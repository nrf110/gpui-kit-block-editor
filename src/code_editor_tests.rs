use super::BlockEditor;
use super::collaboration_tests::{replica, select, set_text, setup};
use crate::native_editor::rich::{RichKind, RichNode};
use crate::native_editor::{BlockEditorModel, BlockSelection, EditorCommand};
use gpui_kit::component::input::{EditorState, Undo};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Entity, EntityInputHandler, Focusable, ScrollDelta,
    TestAppContext, Window, point, px,
};
use serde_json::{Value, json};
use std::time::Duration;

fn finish(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

fn input(editor: &Entity<BlockEditor>, id: &str, cx: &App) -> Entity<EditorState> {
    editor.read(cx).views[id]
        .editor
        .read(cx)
        .code_editor_state()
        .expect("a rendered single code block must mount GPUI's editable EditorState")
}

fn node(editor: &Entity<BlockEditor>, id: &str, cx: &TestAppContext) -> RichNode {
    editor.read_with(cx, |editor, _| {
        editor
            .document
            .sections()
            .into_iter()
            .find(|section| section.id == id)
            .unwrap()
            .content
            .blocks
            .remove(0)
    })
}

fn assert_code(
    editor: &Entity<BlockEditor>,
    id: &str,
    original: &RichNode,
    text: &str,
    lang: Option<&str>,
    cx: &TestAppContext,
) {
    editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let section = editor
            .document
            .sections()
            .into_iter()
            .find(|section| section.id == id)
            .unwrap();
        assert_eq!(section.content.blocks.len(), 1);
        let stored = &section.content.blocks[0];
        let native = editor.views[id]
            .editor
            .read(cx)
            .model()
            .rich_document()
            .unwrap();
        assert_eq!(native.blocks.len(), 1);
        for code in [stored, &native.blocks[0]] {
            assert_eq!(code.id, original.id);
            assert_eq!(code.kind, RichKind::CodeBlock);
            assert_eq!(
                code.runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>(),
                text
            );
            assert_eq!(code.attrs.get("lang").and_then(Value::as_str), lang);
            assert_eq!(code.attrs.get("meta"), original.attrs.get("meta"));
            assert!(code.children.is_empty());
        }
        let exported: Value =
            serde_json::from_str(&editor.document.export_json().unwrap()).unwrap();
        let json_node = &exported["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|section| section["id"] == id)
            .unwrap()["content"]["blocks"][0];
        let native_json = serde_json::to_value(&native.blocks[0]).unwrap();
        for key in ["id", "kind", "runs", "children"] {
            assert_eq!(json_node[key], native_json[key], "native JSON field {key}");
        }
        for key in ["lang", "meta"] {
            assert_eq!(json_node["attrs"][key], native_json["attrs"][key]);
        }
        assert_eq!(
            editor.views[id]
                .editor
                .read(cx)
                .code_editor_state()
                .unwrap()
                .read(cx)
                .value()
                .as_str(),
            text
        );
    });
}

fn focused(editor: &Entity<BlockEditor>, id: &str, window: &Window, cx: &App) {
    assert!(
        editor.read(cx).views[id]
            .editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    );
}

fn click_language(window: &mut Window, label: &str, cx: &mut App) {
    window.click(("code-language", 0usize), cx);
    // Native pickers open scrolled to the checked language, not always item 0.
    if !window.within("popup-menu").find_by_label(label).visible() {
        window.scroll(
            "code-language-menu",
            ScrollDelta::Pixels(point(px(0.), px(1000.))),
            cx,
        );
    }
    let item = window.within("popup-menu").find_by_label(label);
    window
        .within("popup-menu")
        .click(item.path().last().unwrap().clone(), cx);
}

#[gpui_kit::test]
fn code_input_edits_unicode_newlines_tabs_and_selection_without_changing_storage(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("```rust example\nseed\n```", cx);
    let original = node(&editor, &id, cx);
    assert_eq!(original.attrs.get("meta"), Some(&json!("example")));
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 4, 4, window, cx);
        window.render_frame(cx);
        assert!(
            input(&editor, &id, cx)
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        window.input(" 👩‍💻e\u{301}", cx);
        window.press("enter", cx);
        window.press("tab", cx);
        window.input("tail", cx);
    })
    .unwrap();
    finish(handle, cx);
    let text = editor.read_with(cx, |_, cx| {
        input(&editor, &id, cx).read(cx).value().to_string()
    });
    assert!(text.starts_with("seed 👩‍💻e\u{301}\n"), "{text:?}");
    let indent = text
        .strip_prefix("seed 👩‍💻e\u{301}\n")
        .unwrap()
        .strip_suffix("tail")
        .unwrap();
    assert!(
        !indent.is_empty() && indent.chars().all(|c| c == ' ' || c == '\t'),
        "{text:?}"
    );
    assert_code(&editor, &id, &original, &text, Some("rust"), cx);
    cx.update_window(handle, |_, window, cx| {
        let code = input(&editor, &id, cx);
        code.update(cx, |state, cx| {
            state.set_selected_range_with_direction(5.."seed 👩‍💻e\u{301}".len(), true, cx);
        });
        window.render_frame(cx);
        window.input("🦀", cx);
    })
    .unwrap();
    finish(handle, cx);
    let expected = format!("seed 🦀\n{indent}tail");
    assert_code(&editor, &id, &original, &expected, Some("rust"), cx);
    editor.read_with(cx, |editor, cx| {
        let state = editor.views[&id].editor.read(cx);
        assert_eq!(state.model().caret().unwrap().offset, "seed 🦀".len());
    });
}

#[gpui_kit::test]
fn language_menu_preserves_code_identity_metadata_and_standard_fences(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```rust example\nlet text = 1;\n```", cx);
    let original = node(&editor, &id, cx);
    finish(handle, cx);
    for (label, lang) in [
        ("Python", Some("python")),
        ("Plain text", None),
        ("Rust", Some("rust")),
    ] {
        cx.update_window(handle, |_, window, cx| click_language(window, label, cx))
            .unwrap();
        finish(handle, cx);
        assert_code(&editor, &id, &original, "let text = 1;", lang, cx);
        editor.read_with(cx, |editor, _| {
            let markdown = editor.markdown();
            assert!(markdown.starts_with("```"), "{markdown}");
            assert!(markdown.ends_with("```"), "{markdown}");
            let parsed = BlockEditorModel::from_markdown(
                &markdown,
                gpui_kit::base::text::MarkdownExtensions::default(),
            )
            .unwrap()
            .rich_document()
            .unwrap();
            assert_eq!(parsed.blocks[0].kind, RichKind::CodeBlock);
            assert_eq!(parsed.plain_text(), "let text = 1;");
            if let Some(lang) = lang {
                assert_eq!(
                    parsed.blocks[0].attrs.get("lang").and_then(Value::as_str),
                    Some(lang)
                );
            }
        });
        cx.update_window(handle, |_, window, cx| {
            assert!(
                input(&editor, &id, cx)
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn real_code_layout_reserves_a_line_number_gutter_and_moves_the_visible_caret(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("```text\nfirst\nsecond\nthird\n```", cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.render_frame(cx);
        let code = input(&editor, &id, cx);
        let (first, line_height) = code.read(cx).cursor_layout().expect("painted code caret");
        let gutter = code
            .read(cx)
            .text_origin_offset()
            .expect("laid-out editor gutter");
        assert!(gutter > px(0.));
        let text_bounds = code.read(cx).text_bounds().expect("painted text bounds");
        assert!(first.origin.x >= text_bounds.origin.x + gutter);
        window.press("down", cx);
        window.render_frame(cx);
        let (second, _) = code.read(cx).cursor_layout().unwrap();
        assert!((second.origin.y - first.origin.y - line_height).abs() < px(1.));
        assert_eq!(code.read(cx).selected_range(), 6..6);
        focused(&editor, &id, window, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn code_rows_keep_vertical_navigation_until_the_visible_block_edge(cx: &mut TestAppContext) {
    let (handle, editor, _) = setup("Before\n\n```text\nmmmmmm\nmmmmmm\n```\n\nAfter", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    let before = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &order[1], 0, 3, 3, window, cx);
        window.render_frame(cx);
        let code = input(&editor, &order[1], cx);
        window.press("down", cx);
        window.render_frame(cx);
        assert!(code.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(code.read(cx).selected_range(), 10..10);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[2], window, cx);
        assert!(!code.read(cx).focus_handle(cx).is_focused(window));
        window.press("up", cx);
        window.render_frame(cx);
        assert!(code.read(cx).focus_handle(cx).is_focused(window));
        assert!(code.read(cx).selected_range().start >= 7);
        window.press("up", cx);
        window.render_frame(cx);
        assert!(code.read(cx).selected_range().start <= 6);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[0], window, cx);
        window.press("down", cx);
        window.render_frame(cx);
        assert!(code.read(cx).focus_handle(cx).is_focused(window));
        assert!(code.read(cx).selected_range().start <= 6);
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.export_json().unwrap()),
        before
    );
}

#[gpui_kit::test]
fn empty_code_backspace_and_code_reentry_move_focus_to_the_visible_renderer(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("```rust example\n\n```", cx);
    finish(handle, cx);
    let old_input = cx
        .update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 0, 0, window, cx);
            window.render_frame(cx);
            let old = input(&editor, &id, cx);
            window.press("backspace", cx);
            window.render_frame(cx);
            assert!(
                editor.read(cx).views[&id]
                    .editor
                    .read(cx)
                    .code_editor_state()
                    .is_none()
            );
            assert!(!old.read(cx).focus_handle(cx).is_focused(window));
            focused(&editor, &id, window, cx);
            window.input("paragraph", cx);
            old
        })
        .unwrap();
    finish(handle, cx);
    assert_eq!(node(&editor, &id, cx).kind, RichKind::Paragraph);
    cx.update_window(handle, |_, window, cx| {
        let native = editor.read(cx).views[&id].editor.clone();
        native.update(cx, |state, cx| {
            state
                .run(
                    EditorCommand::SetCodeBlock {
                        lang: Some("python".into()),
                    },
                    cx,
                )
                .unwrap();
            state.focus(window, cx);
        });
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        assert!(
            input(&editor, &id, cx)
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        if old_input != input(&editor, &id, cx) {
            assert!(!old_input.read(cx).focus_handle(cx).is_focused(window));
        }
        window.input("!", cx);
    })
    .unwrap();
    finish(handle, cx);
    assert_eq!(
        node(&editor, &id, cx)
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        "paragraph!"
    );
}

#[gpui_kit::test]
fn command_enter_exits_code_without_typing_into_an_invisible_input(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```rust\ncode\n```", cx);
    let original = node(&editor, &id, cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 4, 4, window, cx);
        window.render_frame(cx);
        window.press("cmd-enter", cx);
        window.render_frame(cx);
        window.input("after", cx);
    })
    .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "code", Some("rust"), cx);
    editor.read_with(cx, |editor, _| {
        let sections = editor.document.children(None);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].content.blocks[0].kind, RichKind::Paragraph);
        assert_eq!(sections[1].content.plain_text(), "after");
    });
}

#[gpui_kit::test]
fn peer_edits_preserve_forward_and_reversed_unicode_selection_in_visible_code(
    cx: &mut TestAppContext,
) {
    for reversed in [false, true] {
        let source = "a👩‍💻e\u{301}z";
        let (handle, editor, id) = setup(&format!("```rust example\n{source}\n```"), cx);
        let original = node(&editor, &id, cx);
        let mut remote = replica(&editor, cx);
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 1, 1, window, cx);
            window.render_frame(cx);
            input(&editor, &id, cx).update(cx, |state, cx| {
                state.set_selected_range_with_direction(1..source.len() - 1, reversed, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        finish(handle, cx);
        let vector = remote.state_vector();
        set_text(&mut remote, &id, &format!("peer {source}"));
        let update = remote.encode_update(&vector).unwrap();
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.apply_update(&update, window, cx).unwrap()
            });
        })
        .unwrap();
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            let code = input(&editor, &id, cx);
            assert_eq!(code.read(cx).selected_range(), 6..source.len() + 4);
            assert_eq!(code.read(cx).is_selection_reversed(), reversed);
            let root = editor.read(cx);
            let BlockSelection::Text { anchor, head } = root.views[&id].editor.read(cx).selection()
            else {
                panic!("expected native text selection")
            };
            assert_eq!(
                (anchor.offset, head.offset),
                if reversed {
                    (source.len() + 4, 6)
                } else {
                    (6, source.len() + 4)
                }
            );
            window.input("X", cx);
        })
        .unwrap();
        finish(handle, cx);
        assert_code(&editor, &id, &original, "peer aXz", Some("rust"), cx);
    }
}

#[gpui_kit::test]
fn shared_keyboard_and_input_undo_never_restore_text_over_a_peer_edit(cx: &mut TestAppContext) {
    for input_action in [false, true] {
        let (handle, editor, id) = setup("```rust example\nstart\n```", cx);
        let original = node(&editor, &id, cx);
        let mut remote = replica(&editor, cx);
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 5, 5, window, cx);
            window.render_frame(cx);
            window.input(" local", cx);
        })
        .unwrap();
        finish(handle, cx);
        let vector = remote.state_vector();
        set_text(&mut remote, &id, "peer start");
        let update = remote.encode_update(&vector).unwrap();
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.apply_update(&update, window, cx).unwrap()
            });
        })
        .unwrap();
        finish(handle, cx);
        assert_code(
            &editor,
            &id,
            &original,
            "peer start local",
            Some("rust"),
            cx,
        );
        cx.update_window(handle, |_, window, cx| {
            if input_action {
                // Native menu actions use action dispatch, not the root's key-down hook.
                window.dispatch_action(Box::new(Undo), cx);
            } else {
                window.press("cmd-z", cx);
            }
        })
        .unwrap();
        finish(handle, cx);
        assert_code(&editor, &id, &original, "peer start", Some("rust"), cx);
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_action(Box::new(Undo), cx)
        })
        .unwrap();
        finish(handle, cx);
        assert_code(&editor, &id, &original, "peer start", Some("rust"), cx);
    }
}

#[gpui_kit::test]
fn actual_code_ime_defers_remote_loading_until_commit(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```rust example\nstart\n```", cx);
    let original = node(&editor, &id, cx);
    let mut remote = replica(&editor, cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 5, window, cx);
        window.render_frame(cx);
        input(&editor, &id, cx).update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx);
            assert!(state.marked_text_range(window, cx).is_some());
        });
    })
    .unwrap();
    finish(handle, cx);
    let vector = remote.state_vector();
    set_text(&mut remote, &id, "peer start");
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            assert!(editor.views[&id].editor.read(cx).is_composing());
            editor.apply_update(&update, window, cx).unwrap();
            assert!(!editor.pending_remote.is_empty());
        });
        let code = input(&editor, &id, cx);
        assert_eq!(code.read(cx).value().as_str(), "startに");
        code.update(cx, |state, cx| state.unmark_text(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(32));
    finish(handle, cx);
    assert_code(&editor, &id, &original, "peer startに", Some("rust"), cx);
    editor.read_with(cx, |editor, _| assert!(editor.pending_remote.is_empty()));
}

#[gpui_kit::test]
fn language_menu_contains_scroll_and_returns_to_literal_code_input(cx: &mut TestAppContext) {
    let source = format!("```rust\nseed\n```{}", "\n\nScroll sentinel".repeat(50));
    let (handle, editor, id) = setup(&source, cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 4, 4, window, cx);
        window.render_frame(cx);
        window.click(("code-language", 0usize), cx);
    })
    .unwrap();
    finish(handle, cx);
    let offset = editor.read_with(cx, |editor, _| editor.scroll.offset());
    cx.update_window(handle, |_, window, cx| {
        for delta in [-2000., -2000., 2000., 2000.] {
            window.scroll(
                "code-language-menu",
                ScrollDelta::Pixels(point(px(0.), px(delta))),
                cx,
            );
            assert_eq!(editor.read(cx).scroll.offset(), offset);
        }
        window.press("escape", cx);
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        assert!(window.try_find("code-language-menu").is_none());
        focused(&editor, &id, window, cx);
        window.input(" /", cx);
    })
    .unwrap();
    finish(handle, cx);
    editor.read_with(cx, |root, cx| {
        assert!(root.completion.is_none());
        assert_eq!(input(&editor, &id, cx).read(cx).value().as_str(), "seed /");
        assert_eq!(
            root.document.children(None)[0].content.plain_text(),
            "seed /"
        );
    });
}

#[gpui_kit::test]
fn code_selection_retains_the_existing_floating_toolbar(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```rust\nlet answer = 42;\n```", cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.render_frame(cx);
        input(&editor, &id, cx).update(cx, |state, cx| {
            state.set_selected_range(0..3, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        let native = editor.read(cx).views[&id].editor.clone();
        let selected = native
            .read(cx)
            .selection_bounds()
            .expect("visible code selection");
        let toolbar = window.find("selection-toolbar");
        assert!(toolbar.visible());
        assert!(toolbar.bounds().bottom() <= selected.top());
        input(&editor, &id, cx).update(cx, |state, cx| {
            state.set_selected_range(3..3, cx);
        });
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(
            !window
                .try_find("selection-toolbar")
                .is_some_and(|toolbar| toolbar.visible())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn code_delete_cut_and_paste_actions_reach_the_native_input(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```text\nabcd\n```", cx);
    let original = node(&editor, &id, cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 2, 2, window, cx);
        window.render_frame(cx);
        window.press("backspace", cx);
        window.press("delete", cx);
    })
    .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "ad", Some("text"), cx);
    cx.update_window(handle, |_, window, cx| {
        input(&editor, &id, cx).update(cx, |state, cx| state.set_selected_range(0..1, cx));
        window.render_frame(cx);
        window.press("cmd-x", cx);
    })
    .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "d", Some("text"), cx);
    cx.update_window(handle, |_, window, cx| window.press("cmd-v", cx))
        .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "ad", Some("text"), cx);
    cx.update_window(handle, |_, window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("λ\nnew".into()));
        window.press("cmd-v", cx);
    })
    .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "aλ\nnewd", Some("text"), cx);
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    finish(handle, cx);
    assert_code(&editor, &id, &original, "ad", Some("text"), cx);
}
