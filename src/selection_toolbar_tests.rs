use super::collaboration_tests::{replica, select, set_text, setup};
use super::{BlockEditor, SectionKind};
use crate::BlockSpec;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    App, AppContext, Entity, ScrollDelta, SharedString, TestAppContext, Window, point, px,
};
use markdown_editor::BlockSelection;
use markdown_editor::rich::{RichDocument, RichKind, RichMarks};

fn click_menu_item(window: &mut Window, label: &str, cx: &mut App) {
    let item = window.within("popup-menu").find_by_label(label);
    let viewport = window.find("selection-dropdown-menu").bounds();
    let bounds = item.bounds();
    let delta = if bounds.top() < viewport.top() {
        viewport.top() - bounds.top() + px(4.)
    } else if bounds.bottom() > viewport.bottom() {
        viewport.bottom() - bounds.bottom() - px(4.)
    } else {
        px(0.)
    };
    if delta != px(0.) {
        window.scroll(
            "selection-dropdown-menu",
            ScrollDelta::Pixels(point(px(0.), delta)),
            cx,
        );
    }
    let item = window.within("popup-menu").find_by_label(label);
    assert!(viewport.contains(&item.bounds().center()));
    window
        .within("popup-menu")
        .click(item.path().last().unwrap().clone(), cx);
}

fn assert_selection(editor: &BlockEditor, id: &str, anchor: usize, head: usize, cx: &App) {
    let BlockSelection::Text {
        anchor: actual_anchor,
        head: actual_head,
    } = editor.views[id].editor.read(cx).selection()
    else {
        panic!("expected retained text selection")
    };
    assert_eq!(actual_anchor.block, actual_head.block);
    assert_eq!(actual_anchor.offset, anchor);
    assert_eq!(actual_head.offset, head);
}

fn content(editor: &BlockEditor, id: &str) -> RichDocument {
    editor
        .document
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .unwrap()
        .content
}

// Compare marks on each character, rather than depending on run coalescing.
fn assert_marks(content: &RichDocument, selected: std::ops::Range<usize>, marks: &RichMarks) {
    let mut offset = 0;
    let unmarked = RichMarks::default();
    for run in &content.blocks[0].runs {
        for character in run.text.chars() {
            let expected = if selected.contains(&offset) {
                marks
            } else {
                &unmarked
            };
            assert_eq!(&run.marks, expected, "marks at byte {offset}");
            offset += character.len_utf8();
        }
    }
    assert_eq!(offset, content.plain_text().len());
}

fn assert_link(editor: &Entity<BlockEditor>, id: &str, url: Option<&str>, cx: &TestAppContext) {
    editor.read_with(cx, |editor, _| {
        let content = content(editor, id);
        assert_eq!(content.plain_text(), "Hello world");
        for run in &content.blocks[0].runs {
            assert_eq!(run.marks.link.as_ref().map(|link| link.url.as_str()), url);
        }
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn selection_toolbar_tracks_visible_glyphs_without_moving_content_and_hides_at_caret(
    cx: &mut TestAppContext,
) {
    let source = format!(
        "{}Selected text{}",
        "Before\n\n".repeat(6),
        "\n\nAfter".repeat(40)
    );
    let (handle, editor, _) = setup(&source, cx);
    let id = editor.read_with(cx, |editor, _| editor.document.sections()[6].id.clone());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let row_before = window.find(SharedString::from(id.clone())).bounds();
        select(&editor, &id, 0, 0, 8, window, cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(SharedString::from(id.clone())).bounds(),
            row_before
        );
        assert!(window.find("selection-toolbar").visible());
        let selected_before = editor.read(cx).views[&id]
            .editor
            .read(cx)
            .selection_bounds()
            .expect("visible selected glyphs");
        let toolbar_before = window.find("selection-toolbar").bounds();
        assert!(toolbar_before.bottom() <= selected_before.top());
        assert!(selected_before.top() - toolbar_before.bottom() <= px(16.));
        assert!(toolbar_before.left() >= px(0.));
        assert!(toolbar_before.right() <= px(900.));
        window.scroll(
            SharedString::from(id.clone()),
            ScrollDelta::Pixels(point(px(0.), px(-60.))),
            cx,
        );
        window.render_frame(cx);
        let selected_after = editor.read(cx).views[&id]
            .editor
            .read(cx)
            .selection_bounds()
            .expect("selection stays visible after scrolling");
        let toolbar_after = window.find("selection-toolbar").bounds();
        assert!(selected_after.top() < selected_before.top());
        assert!(toolbar_after.bottom() <= selected_after.top());
        assert!(
            ((toolbar_after.top() - toolbar_before.top())
                - (selected_after.top() - selected_before.top()))
            .abs()
                < px(1.)
        );
        window.press("right", cx);
        window.render_frame(cx);
        assert!(
            !window
                .try_find("selection-toolbar")
                .is_some_and(|toolbar| toolbar.visible())
        );
        assert!(
            editor.read(cx).views[&id]
                .editor
                .read(cx)
                .selection_bounds()
                .is_none()
        );
        assert_selection(editor.read(cx), &id, 8, 8, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn successive_toolbar_marks_keep_the_selected_range_and_undo_independently(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("Hello world", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 0, window, cx);
        window.render_frame(cx);
        let mut marks = RichMarks::default();
        for button in ["selection-bold", "selection-italic", "selection-underline"] {
            window.click(button, cx);
            match button {
                "selection-bold" => marks.bold = true,
                "selection-italic" => marks.italic = true,
                _ => marks.underline = true,
            }
            let editor = editor.read(cx);
            assert_selection(editor, &id, 5, 0, cx);
            let content = content(editor, &id);
            assert_eq!(content.plain_text(), "Hello world");
            assert_marks(&content, 0..5, &marks);
            assert!(editor.error.is_none(), "{:?}", editor.error);
            assert!(window.find("selection-toolbar").visible());
        }
        for marks in [
            RichMarks {
                bold: true,
                italic: true,
                ..Default::default()
            },
            RichMarks {
                bold: true,
                ..Default::default()
            },
            RichMarks::default(),
        ] {
            window.press("cmd-z", cx);
            let editor = editor.read(cx);
            assert_selection(editor, &id, 5, 0, cx);
            let content = content(editor, &id);
            assert_eq!(content.plain_text(), "Hello world");
            assert_marks(&content, 0..5, &marks);
        }
        assert!(!editor.read(cx).document.can_undo());
    })
    .unwrap();
}

#[gpui_kit::test]
fn toolbar_heading_styles_change_the_whole_paragraph_not_the_selected_fragment(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("Hello **world**", cx);
    let original = editor.read_with(cx, |editor, _| content(editor, &id));
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 6, 11, window, cx);
        window.render_frame(cx);
        window.click("selection-style", cx);
        click_menu_item(window, "Heading 2", cx);
        let heading = content(editor.read(cx), &id);
        assert_eq!(heading.blocks.len(), 1);
        assert_eq!(heading.blocks[0].kind, RichKind::Heading);
        assert_eq!(heading.blocks[0].attrs["level"].as_u64(), Some(2));
        assert_eq!(heading.blocks[0].runs, original.blocks[0].runs);
        assert_selection(editor.read(cx), &id, 6, 11, cx);
        window.click("selection-style", cx);
        click_menu_item(window, "Paragraph", cx);
        assert_eq!(content(editor.read(cx), &id), original);
        window.press("cmd-z", cx);
        assert_eq!(content(editor.read(cx), &id), heading);
        assert_selection(editor.read(cx), &id, 6, 11, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn link_field_apply_cancel_and_remove_never_replace_the_selected_text(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("Hello world", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 11, window, cx);
        window.render_frame(cx);
        window.click("selection-link", cx);
        assert!(window.find("selection-link-url").visible());
        window.input("https://example.com/first", cx);
        window.click("selection-link-apply", cx);
        assert_selection(editor.read(cx), &id, 0, 11, cx);
    })
    .unwrap();
    assert_link(&editor, &id, Some("https://example.com/first"), cx);
    let linked_state = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        for cancel in ["selection-link-cancel", "escape"] {
            window.click("selection-link", cx);
            window.press("cmd-a", cx);
            window.input("https://example.com/discarded", cx);
            if cancel == "escape" {
                window.press("escape", cx);
            } else {
                window.click(cancel, cx);
            }
            assert_eq!(editor.read(cx).document.state_vector(), linked_state);
            assert_selection(editor.read(cx), &id, 0, 11, cx);
        }
        window.click("selection-link", cx);
        window.press("cmd-a", cx);
        window.input("#destination", cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert_link(&editor, &id, Some("#destination"), cx);
    cx.update_window(handle, |_, window, cx| {
        window.click("selection-link", cx);
        window.click("selection-link-remove", cx);
    })
    .unwrap();
    assert_link(&editor, &id, None, cx);
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    assert_link(&editor, &id, Some("#destination"), cx);
    cx.update_window(handle, |_, window, cx| {
        window.click("selection-link", cx);
        window.press("cmd-a", cx);
        window.press("backspace", cx);
        window.click("selection-link-apply", cx);
        assert_selection(editor.read(cx), &id, 0, 11, cx);
    })
    .unwrap();
    assert_link(&editor, &id, None, cx);
}

#[gpui_kit::test]
fn unsafe_link_cannot_change_shared_content(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("Hello world", cx);
    let original = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 11, window, cx);
        window.render_frame(cx);
        window.click("selection-link", cx);
        window.input("javascript:alert(1)", cx);
        window.click("selection-link-apply", cx);
        assert_eq!(editor.read(cx).document.state_vector(), original);
        let content = content(editor.read(cx), &id);
        assert_eq!(content.plain_text(), "Hello world");
        assert!(
            content.blocks[0]
                .runs
                .iter()
                .all(|run| run.marks.link.is_none())
        );
        window.press("escape", cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn remote_prefix_rebases_the_link_target_while_url_input_keeps_keyboard_focus(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("Hello world", cx);
    let mut remote = replica(&editor, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 6, 11, window, cx);
        window.render_frame(cx);
        window.click("selection-link", cx);
        window.input("https://example.com/", cx);
    })
    .unwrap();
    let vector = remote.state_vector();
    set_text(&mut remote, &id, "peer Hello world");
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        window.render_frame(cx);
        window.input("world", cx);
        window.press("enter", cx);
        assert_selection(editor.read(cx), &id, 11, 16, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let content = content(editor, &id);
        assert_eq!(content.plain_text(), "peer Hello world");
        let mut offset = 0;
        for run in &content.blocks[0].runs {
            for character in run.text.chars() {
                let expected = (11..16)
                    .contains(&offset)
                    .then_some("https://example.com/world");
                assert_eq!(
                    run.marks.link.as_ref().map(|link| link.url.as_str()),
                    expected
                );
                offset += character.len_utf8();
            }
        }
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        let content = content(editor, &id);
        assert_eq!(content.plain_text(), "peer Hello world");
        assert!(
            content.blocks[0]
                .runs
                .iter()
                .all(|run| run.marks.link.is_none())
        );
    });
}

#[gpui_kit::test]
fn structural_toolbar_actions_retain_the_whole_block_and_children_in_one_undo(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("Whole **block** tail", cx);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor
                .document
                .insert_block(
                    Some(&id),
                    None,
                    &BlockSpec {
                        kind: SectionKind::Text,
                        content: RichDocument::from_markdown("Retained *child*").unwrap(),
                        children: vec![],
                    },
                )
                .unwrap();
            editor.document.clear_history();
            editor.reconcile(window, cx);
        });
    })
    .unwrap();
    let original = editor.read_with(cx, |editor, _| editor.document.sections());
    let child = original[1].clone();
    for action in [
        "selection-quote",
        "selection-bulleted-list",
        "selection-ordered-list",
        "selection-task-list",
        "selection-collapsible",
    ] {
        cx.update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 6, 11, window, cx);
            window.render_frame(cx);
            if action.ends_with("-list") {
                window.click("selection-list", cx);
                click_menu_item(
                    window,
                    match action {
                        "selection-bulleted-list" => "Bulleted list",
                        "selection-ordered-list" => "Numbered list",
                        _ => "Task list",
                    },
                    cx,
                );
            } else {
                window.click(action, cx);
            }
            let sections = editor.read(cx).document.sections();
            assert_eq!(sections.len(), original.len() + 1);
            let block = sections.iter().find(|section| section.id == id).unwrap();
            assert_eq!(block.content, original[0].content);
            let retained_child = sections
                .iter()
                .find(|section| section.id == child.id)
                .unwrap();
            assert_eq!(retained_child.content, child.content);
            assert_eq!(retained_child.parent, child.parent);
            assert_eq!(retained_child.kind, child.kind);
            assert_eq!(retained_child.depth, block.depth + 1);
            if action == "selection-collapsible" {
                assert!(matches!(block.kind, SectionKind::Collapsible { .. }));
                assert!(block.parent.is_none());
            } else {
                let container = sections
                    .iter()
                    .find(|section| Some(&section.id) == block.parent.as_ref())
                    .unwrap();
                if action == "selection-quote" {
                    assert_eq!(container.kind, SectionKind::Quote);
                    assert_eq!(block.kind, SectionKind::Text);
                } else {
                    assert_eq!(
                        container.kind,
                        SectionKind::List {
                            ordered: action == "selection-ordered-list",
                            start: 1,
                        }
                    );
                    assert_eq!(
                        block.kind,
                        SectionKind::ListItem {
                            checked: (action == "selection-task-list").then_some(false),
                        }
                    );
                }
            }
            assert_selection(editor.read(cx), &id, 6, 11, cx);
            assert!(
                editor.read(cx).error.is_none(),
                "{:?}",
                editor.read(cx).error
            );
            window.press("cmd-z", cx);
            assert_eq!(editor.read(cx).document.sections(), original);
            assert!(!editor.read(cx).document.can_undo());
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn toolbar_dropdowns_float_without_reflow_and_contain_wheel_scrolling(cx: &mut TestAppContext) {
    let source = format!(
        "{}Selected text{}",
        "Before\n\n".repeat(6),
        "\n\nAfter".repeat(40)
    );
    let (handle, editor, _) = setup(&source, cx);
    let id = editor.read_with(cx, |editor, _| editor.document.sections()[6].id.clone());
    cx.update_window(handle, |_, window, cx| {
        window.set_rem_size(px(24.));
        select(&editor, &id, 0, 0, 8, window, cx);
        window.render_frame(cx);
    })
    .unwrap();
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    let (toolbar, row, page) = cx
        .update_window(handle, |_, window, cx| {
            (
                window.find("selection-toolbar").bounds(),
                window.find(SharedString::from(id.clone())).bounds(),
                editor.read(cx).scroll.offset(),
            )
        })
        .unwrap();
    for trigger in ["selection-style", "selection-list"] {
        cx.update_window(handle, |_, window, cx| {
            let trigger_bounds = window.find(trigger).bounds();
            window.click(trigger, cx);
            assert_eq!(window.find("selection-toolbar").bounds(), toolbar);
            assert_eq!(window.find(SharedString::from(id.clone())).bounds(), row);
            let menu = window.find("selection-dropdown-menu").bounds();
            assert!(menu.top() >= trigger_bounds.bottom() || menu.bottom() <= trigger_bounds.top());
            assert!((menu.left() - trigger_bounds.left()).abs() < px(8.));
            if trigger == "selection-style" {
                let last = window
                    .within("popup-menu")
                    .find_by_label("Heading 6")
                    .bounds();
                assert!(
                    !menu.contains(&last.center()),
                    "menu {menu:?}, last item {last:?}"
                );
            }
            for delta in [-10_000., -10_000., 10_000., 10_000.] {
                window.scroll(
                    "selection-dropdown-menu",
                    ScrollDelta::Pixels(point(px(0.), px(delta))),
                    cx,
                );
                assert_eq!(editor.read(cx).scroll.offset(), page);
                if trigger == "selection-style" && delta < 0. {
                    let last = window
                        .within("popup-menu")
                        .find_by_label("Heading 6")
                        .bounds();
                    assert!(
                        menu.contains(&last.center()),
                        "menu {menu:?}, last item {last:?}"
                    );
                }
            }
            window.press("escape", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                !window
                    .try_find("selection-dropdown-menu")
                    .is_some_and(|menu| menu.visible())
            );
            assert_selection(editor.read(cx), &id, 0, 8, cx);
            assert_eq!(editor.read(cx).document.state_vector(), vector);
        })
        .unwrap();
    }
    cx.update_window(handle, |_, window, cx| {
        window.scroll(
            SharedString::from(id),
            ScrollDelta::Pixels(point(px(0.), px(-60.))),
            cx,
        );
        assert!(editor.read(cx).scroll.offset().y < page.y);
    })
    .unwrap();
}
