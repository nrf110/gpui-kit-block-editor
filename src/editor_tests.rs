use super::collaboration_tests::{select, setup as setup_content};
use super::{BlockEditor, Document, ExtensionRegistry};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Entity, Focusable, ScrollDelta, SharedString,
    TestAppContext, WindowBounds, WindowOptions, point, px, size,
};

fn setup(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<BlockEditor>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(800.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut document = Document::new();
                    document.insert_section(None, "").unwrap();
                    let editor =
                        BlockEditor::new(document, ExtensionRegistry::default(), window, cx);
                    editor.views[&editor.order[0]]
                        .editor
                        .update(cx, |state, cx| state.focus(window, cx));
                    editor
                })
            },
        )
        .unwrap()
    })
}

#[gpui_kit::test]
fn slash_keyboard_accepts_heading_without_inserting_enter(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.input("/heading-2", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(
            editor.completion.as_ref().unwrap().commands[0].id,
            "heading-2"
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.press("enter", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.input("A shared heading", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.markdown().trim(), "## A shared heading");
        assert!(editor.completion.is_none());
    });
}

#[gpui_kit::test]
fn slash_extension_inserts_mermaid_as_an_independent_section(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.input("/mermaid", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.press("enter", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].content.plain_text().trim().is_empty());
        assert_eq!(
            sections[1].content.blocks[0].kind,
            crate::RichKind::CodeBlock
        );
        assert_eq!(
            sections[1].content.blocks[0]
                .attrs
                .get("lang")
                .and_then(serde_json::Value::as_str),
            Some("mermaid")
        );
        assert!(!editor.markdown().contains("/mermaid"));
    });
}

#[gpui_kit::test]
fn dragging_a_section_reorders_without_replacing_its_source(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    let first = editor.read_with(cx, |editor, _| editor.order[0].clone());
    let second = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(None, "Second section", window, cx)
                    .unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(second.clone()), cx);
        let from = window.find(("grip", 1usize)).bounds().center();
        let to = window
            .find(SharedString::from(first.clone()))
            .bounds()
            .center();
        window.drag(from, to, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        assert_eq!(sections[0].id, second);
        assert_eq!(sections[0].content.plain_text(), "Second section");
        assert_eq!(sections[1].id, first);
    });
}

#[gpui_kit::test]
fn section_controls_follow_hover_and_trash_deletes_only_its_section(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    let first = editor.read_with(cx, |editor, _| editor.order[0].clone());
    let second = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(
                        None,
                        "Keep editing<br>A taller block<br>With multiple soft lines",
                        window,
                        cx,
                    )
                    .unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            !window
                .try_find(("block-menu", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("grip", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("remove", 0usize))
                .is_some_and(|control| control.visible())
        );
        let bounds = window.find(SharedString::from(first.clone())).bounds();

        window.hover(SharedString::from(first.clone()), cx);
        assert!(window.find(("grip", 0usize)).visible());
        let plus = window.find(("block-menu", 0usize));
        let grip = window.find(("grip", 0usize));
        assert!(plus.visible());
        assert!(plus.bounds().right() <= grip.bounds().left());
        assert!((plus.bounds().center().y - grip.bounds().center().y).abs() < px(0.5));
        assert!(window.find(("remove", 0usize)).visible());
        assert!(
            (window.find(("grip", 0usize)).bounds().center().y - bounds.center().y).abs() < px(0.5)
        );
        assert!(
            !window
                .try_find(("grip", 1usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("remove", 1usize))
                .is_some_and(|control| control.visible())
        );
        assert_eq!(
            window.find(SharedString::from(first.clone())).bounds(),
            bounds
        );

        window.hover(SharedString::from(second.clone()), cx);
        assert!(
            !window
                .try_find(("block-menu", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("grip", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("remove", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(window.find(("grip", 1usize)).visible());
        assert!(window.find(("block-menu", 1usize)).visible());
        assert!(window.find(("remove", 1usize)).visible());
        let section_bounds = window.find(SharedString::from(second.clone())).bounds();
        assert!(
            (window.find(("grip", 1usize)).bounds().center().y - section_bounds.center().y).abs()
                < px(0.5)
        );
        window.click(("remove", 1usize), cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        assert_eq!(
            sections
                .iter()
                .map(|section| &section.id)
                .collect::<Vec<_>>(),
            vec![&first]
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.hover("add-section", cx);
        assert!(
            !window
                .try_find(("block-menu", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("grip", 0usize))
                .is_some_and(|control| control.visible())
        );
        assert!(
            !window
                .try_find(("remove", 0usize))
                .is_some_and(|control| control.visible())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn diagram_toggle_switches_views_and_preserves_edits(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    let diagram = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(None, "```mermaid\nflowchart LR\n A --> B\n```", window, cx)
                    .unwrap()
            })
        })
        .unwrap();
    cx.run_until_parked();
    let (before, original_svg) = editor.read_with(cx, |editor, _| {
        (
            editor.document.encode_state(),
            editor.views[&diagram]
                .preview
                .as_ref()
                .unwrap()
                .bytes
                .clone(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("section-preview", 1usize)).visible());
        assert!(
            !window
                .try_find(("section-source", 1usize))
                .is_some_and(|view| view.visible())
        );
        assert!(
            !window
                .try_find(("toggle-preview", 1usize))
                .is_some_and(|button| button.visible())
        );
        window.hover(SharedString::from(diagram.clone()), cx);
        let toggle = window.find(("toggle-preview", 1usize)).bounds();
        let remove = window.find(("remove", 1usize)).bounds();
        assert!(toggle.right() <= remove.left());
        window.click(("toggle-preview", 1usize), cx);
        assert!(window.find(("section-source", 1usize)).visible());
        assert!(
            !window
                .try_find(("section-preview", 1usize))
                .is_some_and(|view| view.visible())
        );
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.encode_state(), before)
    });

    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.views[&diagram]
                .editor
                .update(cx, |state, cx| state.select_end(cx).unwrap());
        });
        window.input("\n B --> C", cx);
    })
    .unwrap();
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert!(editor.document.markdown().contains("B --> C"));
        assert_ne!(
            editor.views[&diagram].preview.as_ref().unwrap().bytes,
            original_svg
        );
    });

    // Remote changes refresh the render without changing this replica's chosen view.
    let mut remote = Document::new();
    remote
        .apply_update(&editor.read_with(cx, |editor, _| editor.document.encode_state()))
        .unwrap();
    let vector = remote.state_vector();
    remote
        .set_markdown(&diagram, "```mermaid\nflowchart LR\n A --> Remote\n```")
        .unwrap();
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        window.render_frame(cx);
        assert!(window.find(("section-source", 1usize)).visible());
    })
    .unwrap();
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert!(editor.views[&diagram].show_source);
        assert!(
            String::from_utf8_lossy(&editor.views[&diagram].preview.as_ref().unwrap().bytes)
                .contains("Remote")
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(diagram.clone()), cx);
        window.click(("toggle-preview", 1usize), cx);
        assert!(window.find(("section-preview", 1usize)).visible());
        assert!(
            !window
                .try_find(("section-source", 1usize))
                .is_some_and(|view| view.visible())
        );
        window.hover("add-section", cx);
        assert!(
            !window
                .try_find(("toggle-preview", 1usize))
                .is_some_and(|button| button.visible())
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn invalid_diagram_can_switch_to_source_and_other_code_has_no_toggle(cx: &mut TestAppContext) {
    let (handle, editor) = setup(cx);
    let (diagram, code) = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                let diagram = editor
                    .insert_section(None, "```mermaid\nnot a diagram\n```", window, cx)
                    .unwrap();
                let code = editor
                    .insert_section(None, "```rust\nfn main() {}\n```", window, cx)
                    .unwrap();
                (diagram, code)
            })
        })
        .unwrap();
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert!(editor.views[&diagram].preview_error.is_some())
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(code), cx);
        assert!(
            !window
                .try_find(("toggle-preview", 2usize))
                .is_some_and(|button| button.visible())
        );
        window.hover(SharedString::from(diagram.clone()), cx);
        window.click(("toggle-preview", 1usize), cx);
        assert!(window.find(("section-source", 1usize)).visible());
        assert!(
            !window
                .try_find(("section-preview", 1usize))
                .is_some_and(|view| view.visible())
        );
        window.click(("toggle-preview", 1usize), cx);
        assert!(window.find(("section-preview", 1usize)).visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn plus_menu_preserves_selected_text_and_escape_does_not_edit(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup_content("Hello world", cx);
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 5, window, cx);
        window.render_frame(cx);
        window.hover(SharedString::from(id.clone()), cx);
        let row_bounds = window.find(SharedString::from(id.clone())).bounds();
        let button_bounds = window.find(("block-menu", 0usize)).bounds();
        window.click(("block-menu", 0usize), cx);
        let menu_bounds = window.find("slash-menu").bounds();
        assert!((menu_bounds.left() - button_bounds.left()).abs() < px(1.));
        assert!(menu_bounds.top() >= button_bounds.bottom());
        assert!(menu_bounds.top() - button_bounds.bottom() <= px(8.));
        assert_eq!(
            window.find(SharedString::from(id.clone())).bounds(),
            row_bounds
        );
        assert_eq!(editor.read(cx).document.state_vector(), vector);
        window.press("escape", cx);
        assert!(editor.read(cx).completion.is_none());
        assert_eq!(editor.read(cx).document.state_vector(), vector);
        window.hover(SharedString::from(id.clone()), cx);
        window.click(("block-menu", 0usize), cx);
        let viewport = window.find("slash-menu-items").bounds();
        let command = window.find("bold").bounds();
        window.scroll(
            "slash-menu-items",
            ScrollDelta::Pixels(point(
                px(0.),
                (viewport.bottom() - command.bottom() - px(4.)).min(px(0.)),
            )),
            cx,
        );
        assert!(
            window
                .find("slash-menu-items")
                .bounds()
                .contains(&window.find("bold").bounds().center())
        );
        window.click("bold", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.markdown().trim(), "**Hello** world");
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.markdown().trim(), "Hello world");
    });
}

#[gpui_kit::test]
fn plus_menu_keyboard_command_targets_its_block_not_the_previously_focused_one(
    cx: &mut TestAppContext,
) {
    let (handle, editor, first) = setup_content("First", cx);
    let second = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert_section(None, "Second", window, cx).unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.hover(SharedString::from(second.clone()), cx);
        window.click(("block-menu", 1usize), cx);
        let index = editor
            .read(cx)
            .completion
            .as_ref()
            .unwrap()
            .commands
            .iter()
            .position(|command| command.id == "collapsible")
            .unwrap();
        for _ in 0..index {
            window.press("down", cx);
        }
        window.press("enter", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.views[&first].kind, super::SectionKind::Text);
        assert_eq!(editor.views[&first].content.plain_text(), "First");
        assert!(matches!(
            editor.views[&second].kind,
            super::SectionKind::Collapsible { .. }
        ));
        assert_eq!(editor.views[&second].content.plain_text(), "Second");
        assert_eq!(editor.document.children(Some(&second)).len(), 1);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn command_menu_wheel_scrolling_does_not_scroll_the_document_even_at_its_edges(
    cx: &mut TestAppContext,
) {
    let (handle, editor, first) = setup_content("First", cx);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            for _ in 0..40 {
                editor
                    .document
                    .insert_section(None, "Another block")
                    .unwrap();
            }
            editor.reconcile(window, cx);
        });
        select(&editor, &first, 0, 0, 0, window, cx);
        window.render_frame(cx);
    })
    .unwrap();
    let page_offset = editor.read_with(cx, |editor, _| editor.scroll.offset());
    for via_button in [true, false] {
        cx.update_window(handle, |_, window, cx| {
            if via_button {
                window.hover(SharedString::from(first.clone()), cx);
                window.click(("block-menu", 0usize), cx);
            } else {
                window.input("/", cx);
                window.render_frame(cx);
            }
        })
        .unwrap();
        let menu_scroll = editor.read_with(cx, |editor, _| {
            editor.completion.as_ref().unwrap().scroll.clone()
        });
        cx.update_window(handle, |_, window, cx| {
            for delta in [-120., -10_000., -10_000., 10_000., 10_000.] {
                window.scroll(
                    "slash-menu-items",
                    ScrollDelta::Pixels(point(px(0.), px(delta))),
                    cx,
                );
                assert_eq!(editor.read(cx).scroll.offset(), page_offset);
                if delta < 0. {
                    assert!(menu_scroll.offset().y < px(0.));
                } else {
                    assert_eq!(menu_scroll.offset().y, px(0.));
                }
            }
            window.press("escape", cx);
        })
        .unwrap();
    }
    cx.update_window(handle, |_, window, cx| {
        window.scroll(
            SharedString::from(first),
            ScrollDelta::Pixels(point(px(0.), px(-120.))),
            cx,
        );
        assert!(editor.read(cx).scroll.offset().y < page_offset.y);
    })
    .unwrap();
}

fn click_divider_menu(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, index: usize) {
    window.click(("block-menu", index), cx);
    let viewport = window.find("slash-menu-items").bounds();
    let command = window.find("divider").bounds();
    window.scroll(
        "slash-menu-items",
        ScrollDelta::Pixels(point(px(0.), viewport.bottom() - command.bottom() - px(4.))),
        cx,
    );
    assert!(
        window
            .find("slash-menu-items")
            .bounds()
            .contains(&window.find("divider").bounds().center())
    );
    window.click("divider", cx);
}

#[gpui_kit::test]
fn divider_slash_replaces_an_empty_block_and_continues_typing_with_one_undo(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = setup(cx);
    let id = editor.read_with(cx, |editor, _| editor.order[0].clone());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.input("/divider", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    let inserted = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, id);
        assert_eq!(
            sections[0].content.blocks[0].kind,
            crate::RichKind::HorizontalRule
        );
        assert_eq!(
            sections[1].content.blocks[0].kind,
            crate::RichKind::Paragraph
        );
        assert!(sections[1].content.plain_text().is_empty());
        let restored = Document::import_json(&editor.document.export_json().unwrap()).unwrap();
        assert_eq!(restored.sections(), sections);
        let mut peer = Document::new();
        peer.apply_update(&editor.document.encode_state()).unwrap();
        assert_eq!(peer.sections(), sections);
        assert_eq!(editor.markdown().trim(), "---");
        assert!(editor.error.is_none(), "{:?}", editor.error);
        sections
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let rule = window.find(SharedString::from(id.clone())).bounds();
        let text = window
            .find(SharedString::from(inserted[1].id.clone()))
            .bounds();
        assert!(rule.size.width > px(0.));
        assert!(rule.bottom() <= text.top());
        assert!(
            editor.read(cx).views[&inserted[1].id]
                .editor
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections().len(), 1);
        assert_eq!(editor.views[&id].content.plain_text(), "/divider");
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-shift-z", cx))
        .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.sections()),
        inserted
    );
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &inserted[1].id, 0, 0, 0, window, cx);
        window.input("After the divider", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(
            editor.document.sections()[1].content.plain_text(),
            "After the divider"
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn divider_plus_preserves_text_and_the_divider_can_be_dragged(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup_content("Hello world\n\nTail", cx);
    let original = editor.read_with(cx, |editor, _| editor.document.sections());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 5, window, cx);
        window.render_frame(cx);
        window.hover(SharedString::from(id.clone()), cx);
        click_divider_menu(window, cx, 0);
    })
    .unwrap();
    let inserted = editor.read_with(cx, |editor, _| {
        let sections = editor.document.children(None);
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0], original[0]);
        assert_eq!(
            sections[1].content.blocks[0].kind,
            crate::RichKind::HorizontalRule
        );
        assert_eq!(sections[3], original[1]);
        assert!(editor.error.is_none(), "{:?}", editor.error);
        sections
    });
    cx.update_window(handle, |_, window, cx| {
        window.input("Between", cx);
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.sections()[2]
            .content
            .plain_text()),
        "Between"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(inserted[1].id.clone()), cx);
        let from = window.find(("grip", 1usize)).bounds().center();
        let to = window.find("append-section").bounds().center();
        window.drag(from, to, cx);
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor
            .document
            .children(None)
            .last()
            .unwrap()
            .id
            .clone()),
        inserted[1].id
    );
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.children(None)[1].id.clone()),
        inserted[1].id
    );
}

#[gpui_kit::test]
fn divider_in_list_and_collapsible_titles_preserves_the_title_and_existing_body(
    cx: &mut TestAppContext,
) {
    for source in [
        "- Title\n  - Child\n- Other",
        "<details><summary>Title</summary>\n\nBody\n\n</details>",
    ] {
        let (handle, editor, id) = setup_content(source, cx);
        let original = editor.read_with(cx, |editor, _| editor.document.sections());
        cx.update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 0, 0, window, cx);
            window.render_frame(cx);
            window.hover(SharedString::from(id.clone()), cx);
            click_divider_menu(window, cx, 0);
        })
        .unwrap();
        let continuation = editor.read_with(cx, |editor, _| {
            let sections = editor.document.sections();
            for old in &original {
                assert_eq!(
                    sections.iter().find(|section| section.id == old.id),
                    Some(old)
                );
            }
            let children = editor.document.children(Some(&id));
            assert_eq!(
                children[0].content.blocks[0].kind,
                crate::RichKind::HorizontalRule
            );
            assert_eq!(
                children[1].content.blocks[0].kind,
                crate::RichKind::Paragraph
            );
            assert!(!editor.is_collapsed(&id));
            assert!(editor.error.is_none(), "{:?}", editor.error);
            children[1].id.clone()
        });
        cx.update_window(handle, |_, window, cx| {
            window.input("New body", cx);
        })
        .unwrap();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.views[&continuation]
                .content
                .plain_text()),
            "New body"
        );
    }
}

#[gpui_kit::test]
fn divider_object_selection_clears_on_outside_click_and_focus_changes(cx: &mut TestAppContext) {
    use crate::native_editor::{BlockPosition, BlockSelection};

    for whole_block in [false, true] {
        for exit in 0..3 {
            let (handle, editor, id) = setup_content("---\n\nAfter", cx);
            // Blur observers run only in active platform windows.
            cx.update_window(handle, |_, window, _| window.activate_window())
                .unwrap();
            cx.run_until_parked();
            let original = editor.read_with(cx, |editor, _| editor.document.sections());
            let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
            let after = original[1].id.clone();
            let page = cx
                .update_window(handle, |_, window, cx| {
                    let native = editor.read(cx).views[&id].editor.clone();
                    native.update(cx, |state, cx| {
                        let rule = state
                            .model()
                            .document()
                            .root()
                            .children()
                            .next()
                            .unwrap()
                            .id();
                        let selection = if whole_block {
                            BlockSelection::Blocks {
                                anchor: rule,
                                head: rule,
                            }
                        } else {
                            BlockSelection::Text {
                                anchor: BlockPosition::new(rule, 0),
                                head: BlockPosition::new(rule, 1),
                            }
                        };
                        state.set_selection(selection, cx).unwrap();
                        state.focus(window, cx);
                    });
                    window.render_frame(cx);
                    assert!(!native.read(cx).selection().is_collapsed());
                    let page = editor.read(cx).scroll.offset();
                    match exit {
                        0 => window.click(("section-source", 1usize), cx),
                        1 => {
                            let blank = window.find("append-section").bounds();
                            window.click_at(
                                "append-section",
                                point(blank.size.width - px(4.), blank.size.height - px(4.)),
                                cx,
                            );
                        }
                        _ => select(&editor, &after, 0, 5, 5, window, cx),
                    }
                    page
                })
                .unwrap();
            // GPUI dispatches blur observers after the focus-changing update.
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                let native = editor.read(cx).views[&id].editor.clone();
                window.render_frame(cx);
                assert!(
                    native.read(cx).selection().is_collapsed(),
                    "whole_block={whole_block}, exit={exit}: {:?}",
                    native.read(cx).selection()
                );
                assert!(!native.read(cx).focus_handle(cx).is_focused(window));
                assert_eq!(editor.read(cx).scroll.offset(), page);
                assert_eq!(editor.read(cx).document.sections(), original);
                assert_eq!(editor.read(cx).document.state_vector(), vector);
            })
            .unwrap();
            cx.update_window(handle, |_, window, cx| {
                if exit == 1 {
                    window.press("delete", cx);
                } else {
                    window.input("!", cx);
                }
            })
            .unwrap();
            editor.read_with(cx, |editor, _| {
                assert_eq!(editor.document.sections()[0], original[0]);
                if exit == 1 {
                    assert_eq!(editor.document.sections(), original);
                } else {
                    assert_eq!(editor.views[&after].content.plain_text(), "After!");
                }
                assert!(editor.error.is_none(), "{:?}", editor.error);
            });
        }
    }
}
