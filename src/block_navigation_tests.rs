use super::collaboration_tests::{select, setup};
use super::{BlockEditor, BlockSpec, RichDocument, SectionKind};
use crate::native_editor::BlockSelection;
use crate::{MediaAttachment, MediaKind};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, AppContext, Entity, EntityInputHandler, Focusable, TestAppContext, Window};

fn focused(editor: &Entity<BlockEditor>, id: &str, window: &Window, cx: &App) {
    let editor = editor.read(cx);
    assert!(editor.error.is_none(), "{:?}", editor.error);
    assert!(
        editor.views[id]
            .editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    );
}

fn caret_offset(editor: &Entity<BlockEditor>, id: &str, cx: &App) -> usize {
    editor.read(cx).views[id]
        .editor
        .read(cx)
        .model()
        .caret()
        .unwrap()
        .offset
}

#[gpui_kit::test]
fn vertical_roundtrip_keeps_the_preferred_column_across_short_sections(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("mmmmmmmmmm\n\nm\n\nmmmmmmmmmm", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 6, 6, window, cx);
        editor.update(cx, |editor, _| {
            editor.block_selection.insert(order[2].clone());
            editor.block_anchor = Some(order[2].clone());
        });
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert_eq!(caret_offset(&editor, &order[1], cx), 1);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[2], window, cx);
        assert_eq!(caret_offset(&editor, &order[2], cx), 6);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert_eq!(caret_offset(&editor, &order[1], cx), 1);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert_eq!(caret_offset(&editor, &first, cx), 6);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.state_vector(), vector);
        assert!(!editor.document.can_undo());
        assert!(editor.block_selection.is_empty());
        assert!(editor.block_anchor.is_none());
        assert!(editor.views.values().all(|view| view.selection.is_some()));
    });
}

#[gpui_kit::test]
fn wrapped_visual_rows_stay_in_their_native_section(cx: &mut TestAppContext) {
    let text = "wrapped words ".repeat(70);
    let (handle, editor, first) = setup(&format!("{text}\n\nAfter"), cx);
    let last = editor.read_with(cx, |editor, _| editor.order[1].clone());
    let len = text.trim_end().len();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert!(caret_offset(&editor, &first, cx) > 3);
        select(&editor, &first, 0, len - 3, len - 3, window, cx);
        window.render_frame(cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert!(caret_offset(&editor, &first, cx) < len - 3);
        select(&editor, &first, 0, len, len, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &last, window, cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert!(caret_offset(&editor, &first, cx) > len / 2);
    })
    .unwrap();
}

#[gpui_kit::test]
fn vertical_navigation_crosses_heading_and_list_sections_in_visible_order(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("Before\n\n# Heading\n\n- First\n- Second\n\nAfter", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    assert_eq!(order.len(), 5);
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 2, 2, window, cx);
        window.render_frame(cx);
        for id in &order[1..] {
            window.press("down", cx);
            window.render_frame(cx);
            focused(&editor, id, window, cx);
        }
        for id in order[..order.len() - 1].iter().rev() {
            window.press("up", cx);
            window.render_frame(cx);
            focused(&editor, id, window, cx);
        }
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.state_vector()),
        vector
    );
}

#[gpui_kit::test]
fn code_boundary_navigation_never_creates_exit_paragraphs(cx: &mut TestAppContext) {
    let (handle, editor, _) = setup("Before\n\n```text\nmmmmmm\nmmmmmm\n```\n\nAfter", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    assert_eq!(order.len(), 3);
    let before = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &order[1], 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert!(caret_offset(&editor, &order[1], cx) > 6);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[2], window, cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert!(caret_offset(&editor, &order[1], cx) >= 7);
        select(&editor, &order[1], 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[0], window, cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert!(caret_offset(&editor, &order[1], cx) <= 6);
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert_eq!(editor.document.export_json().unwrap(), before);
        assert!(!editor.document.can_undo());
        assert_eq!(
            editor.views[&order[1]]
                .editor
                .read(cx)
                .model()
                .document()
                .leaves()
                .count(),
            1
        );
    });
}

#[gpui_kit::test]
fn table_boundary_navigation_keeps_internal_rows_native_without_exit_paragraphs(
    cx: &mut TestAppContext,
) {
    for column in [0, 1] {
        let (handle, editor, _) = setup(
            "Before\n\n| mmmmm | nnnnn |\n| --- | --- |\n| mmmmm | nnnnn |\n\nAfter",
            cx,
        );
        let order = editor.read_with(cx, |editor, _| editor.order.clone());
        assert_eq!(order.len(), 3);
        let before = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
        cx.update_window(handle, |_, window, cx| {
            select(&editor, &order[1], column, 2, 2, window, cx);
            window.render_frame(cx);
            let first_cell = editor.read(cx).views[&order[1]]
                .editor
                .read(cx)
                .model()
                .caret()
                .unwrap()
                .block;
            window.press("down", cx);
            window.render_frame(cx);
            focused(&editor, &order[1], window, cx);
            let last_cell = editor.read(cx).views[&order[1]]
                .editor
                .read(cx)
                .model()
                .caret()
                .unwrap()
                .block;
            assert_ne!(first_cell, last_cell);
            window.press("down", cx);
            window.render_frame(cx);
            focused(&editor, &order[2], window, cx);
            window.press("up", cx);
            window.render_frame(cx);
            focused(&editor, &order[1], window, cx);
            assert_eq!(
                editor.read(cx).views[&order[1]]
                    .editor
                    .read(cx)
                    .model()
                    .caret()
                    .unwrap()
                    .block,
                last_cell
            );
            select(&editor, &order[1], column, 2, 2, window, cx);
            window.render_frame(cx);
            window.press("up", cx);
            window.render_frame(cx);
            focused(&editor, &order[0], window, cx);
            window.press("down", cx);
            window.render_frame(cx);
            focused(&editor, &order[1], window, cx);
            assert_eq!(
                editor.read(cx).views[&order[1]]
                    .editor
                    .read(cx)
                    .model()
                    .caret()
                    .unwrap()
                    .block,
                first_cell
            );
        })
        .unwrap();
        editor.read_with(cx, |editor, cx| {
            assert_eq!(editor.document.export_json().unwrap(), before);
            assert!(!editor.document.can_undo());
            assert_eq!(
                editor.views[&order[1]]
                    .editor
                    .read(cx)
                    .model()
                    .document()
                    .leaves()
                    .count(),
                4
            );
        });
    }
}

#[gpui_kit::test]
fn vertical_navigation_skips_retained_collapsed_descendants(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("Before\n\nAfter", cx);
    let (title, body, last) = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                let last = editor.order[1].clone();
                let title = editor
                    .document
                    .insert_block(
                        None,
                        Some(&last),
                        &BlockSpec {
                            kind: SectionKind::Collapsible {
                                default_collapsed: true,
                            },
                            content: RichDocument::paragraph("Title"),
                            children: vec![BlockSpec {
                                kind: SectionKind::Text,
                                content: RichDocument::paragraph("Hidden body"),
                                children: Vec::new(),
                            }],
                        },
                    )
                    .unwrap();
                let body = editor.document.children(Some(&title))[0].id.clone();
                editor.document.clear_history();
                editor.reconcile(window, cx);
                assert!(editor.views.contains_key(&body));
                assert!(!editor.order.contains(&body));
                (title, body, last)
            })
        })
        .unwrap();
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 2, 2, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &title, window, cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &last, window, cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &title, window, cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.state_vector(), vector);
        assert!(!editor.order.contains(&body));
        assert!(editor.is_collapsed(&title));
    });
}

#[gpui_kit::test]
fn dividers_are_navigable_in_both_directions_without_losing_the_column(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("mmmmmmmm\n\n---\n\nmmmmmmmm", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    assert_eq!(order.len(), 3);
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 4, 4, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        assert!(matches!(editor.read(cx).views[&order[1]].editor.read(cx).selection(), BlockSelection::Blocks { anchor, head } if anchor == head));
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[2], window, cx);
        assert_eq!(caret_offset(&editor, &order[2], cx), 4);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert_eq!(caret_offset(&editor, &first, cx), 4);
    }).unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.state_vector()),
        vector
    );
}

#[gpui_kit::test]
fn attachment_navigation_selects_the_visible_card_instead_of_a_hidden_input(
    cx: &mut TestAppContext,
) {
    let (handle, editor, first) = setup("mmmmmmmm\n\nmmmmmmmm", cx);
    let (card, last) = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                let last = editor.order[1].clone();
                let content = MediaAttachment {
                    asset_id: "3aa24075-42fd-4d87-bb0b-94b532546721".into(),
                    kind: MediaKind::File,
                    file_name: "report.pdf".into(),
                    mime_type: "application/pdf".into(),
                    byte_len: 42,
                }
                .to_content()
                .unwrap();
                let card = editor
                    .document
                    .insert_block(
                        None,
                        Some(&last),
                        &BlockSpec {
                            kind: SectionKind::Text,
                            content,
                            children: Vec::new(),
                        },
                    )
                    .unwrap();
                editor.document.clear_history();
                editor.reconcile(window, cx);
                (card, last)
            })
        })
        .unwrap();
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 4, 4, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        editor.read_with(cx, |editor, cx| {
            assert!(editor.focus_scope.is_focused(window));
            assert!(
                !editor.views[&card]
                    .editor
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(editor.block_selection.len(), 1);
            assert!(editor.block_selection.contains(&card));
        });
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &last, window, cx);
        assert_eq!(caret_offset(&editor, &last, cx), 4);
        window.press("up", cx);
        window.render_frame(cx);
        assert!(editor.read(cx).focus_scope.is_focused(window));
        assert!(editor.read(cx).block_selection.contains(&card));
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &first, window, cx);
        assert_eq!(caret_offset(&editor, &first, cx), 4);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.state_vector(), vector);
        assert!(editor.block_selection.is_empty());
        assert!(!editor.document.can_undo());
    });
}

#[gpui_kit::test]
fn vertical_navigation_leaves_ranges_modifiers_and_document_edges_native(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("Before\n\nAfter", cx);
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 1, 4, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        focused(&editor, &first, window, cx);
        for key in ["shift-down", "alt-down", "ctrl-down", "cmd-down", "up"] {
            select(&editor, &first, 0, 3, 3, window, cx);
            window.render_frame(cx);
            window.press(key, cx);
            focused(&editor, &first, window, cx);
        }
        let last = editor.read(cx).order[1].clone();
        select(&editor, &last, 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        focused(&editor, &last, window, cx);
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.state_vector()),
        vector
    );
}

#[gpui_kit::test]
fn entering_a_diagram_reveals_its_source_before_focusing_it(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup(
        "Before\n\n```mermaid\nflowchart LR\n A --> B\n```\n\nAfter",
        cx,
    );
    let diagram = editor.read_with(cx, |editor, _| editor.order[1].clone());
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        assert!(!editor.read(cx).views[&diagram].show_source);
        select(&editor, &first, 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &diagram, window, cx);
        assert!(editor.read(cx).views[&diagram].show_source);
        assert!(window.find(("section-source", 1usize)).visible());
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.state_vector(), vector);
        assert!(!editor.document.can_undo());
    });
}

#[gpui_kit::test]
fn slash_completion_and_ime_keep_vertical_keys_in_the_active_native_editor(
    cx: &mut TestAppContext,
) {
    let (handle, editor, first) = setup("Before\n\nAfter", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &first, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        assert!(editor.read(cx).completion.is_some());
        window.press("down", cx);
        assert_eq!(editor.read(cx).completion.as_ref().unwrap().selected, 1);
        focused(&editor, &first, window, cx);
        window.press("escape", cx);
        editor.update(cx, |editor, cx| {
            assert!(editor.completion.is_none());
            editor.views[&first].editor.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx);
                assert!(state.is_composing());
            });
        });
        window.render_frame(cx);
        window.press("down", cx);
        focused(&editor, &first, window, cx);
        editor.update(cx, |editor, cx| {
            editor.views[&first].editor.update(cx, |state, cx| {
                state.unmark_text(window, cx);
            });
        });
        focused(&editor, &first, window, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn reference_metadata_is_not_a_navigation_target_and_unicode_entry_is_grapheme_aligned(
    cx: &mut TestAppContext,
) {
    use unicode_segmentation::UnicodeSegmentation;

    let (handle, editor, _) = setup(
        "Before\n\n[👩🏽‍💻日本語][id]\n\nAfter\n\n[id]: https://example.test",
        cx,
    );
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    assert_eq!(order.len(), 3);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &order[2], 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("up", cx);
        window.render_frame(cx);
        focused(&editor, &order[1], window, cx);
        let native = editor.read(cx).views[&order[1]].editor.read(cx);
        let caret = native.model().caret().unwrap();
        let leaf = native.model().document().block(caret.block).unwrap();
        assert_eq!(leaf.kind(), crate::native_editor::BlockKind::Paragraph);
        let text = leaf.text().unwrap();
        assert!(
            text.grapheme_indices(true)
                .map(|(offset, _)| offset)
                .chain(std::iter::once(text.len()))
                .any(|offset| offset == caret.offset)
        );
        window.press("down", cx);
        window.render_frame(cx);
        focused(&editor, &order[2], window, cx);
        assert_eq!(caret_offset(&editor, &order[2], cx), 3);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.state_vector(), vector);
        assert!(!editor.document.can_undo());
    });
}
