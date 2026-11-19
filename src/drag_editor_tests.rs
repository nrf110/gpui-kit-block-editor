use super::collaboration_tests::{replica, select, set_text, setup};
use super::{BlockEditor, SectionKind};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    App, AppContext, ElementId, Entity, Modifiers, SharedString, TestAppContext, Window,
};

fn click_grip(
    editor: &Entity<BlockEditor>,
    id: &str,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    let index = editor
        .read(cx)
        .order
        .iter()
        .position(|candidate| candidate == id)
        .unwrap();
    window.hover(SharedString::from(id.to_owned()), cx);
    window.click_with_modifiers(("grip", index), modifiers, cx);
}

fn drag_grip(
    editor: &Entity<BlockEditor>,
    id: &str,
    target: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
) {
    let index = editor
        .read(cx)
        .order
        .iter()
        .position(|candidate| candidate == id)
        .unwrap();
    window.hover(SharedString::from(id.to_owned()), cx);
    let from = window.find(("grip", index)).bounds().center();
    let to = window.find(target).bounds().center();
    window.drag(from, to, cx);
}

#[gpui_kit::test]
fn dragging_a_task_out_of_its_list_keeps_children_caret_and_undo(cx: &mut TestAppContext) {
    let (handle, editor, one) = setup("- one\n- [x] two\n  - child", cx);
    let (two, child, group) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let two = sections
            .iter()
            .find(|block| block.content.plain_text() == "two")
            .unwrap();
        let child = sections
            .iter()
            .find(|block| block.content.plain_text() == "child")
            .unwrap();
        (
            two.id.clone(),
            child.id.clone(),
            two.parent.clone().unwrap(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &two, 0, 1, 1, window, cx);
        window.render_frame(cx);
        drag_grip(&editor, &two, "append-section", window, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let roots = editor.document.children(None);
        assert!(
            roots
                .iter()
                .map(|block| block.id.as_str())
                .eq([group.as_str(), two.as_str()])
        );
        assert_eq!(editor.views[&two].kind, SectionKind::Text);
        assert_eq!(
            editor.views[&two]
                .editor
                .read(cx)
                .model()
                .caret()
                .unwrap()
                .offset,
            1
        );
        let sections = editor.document.sections();
        let child_group = sections
            .iter()
            .find(|block| Some(&block.id) == editor.views[&child].parent.as_ref())
            .unwrap();
        assert_eq!(child_group.parent.as_deref(), Some(two.as_str()));
        assert_eq!(editor.document.children(Some(&group))[0].id, one);
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.views[&two].parent.as_deref(), Some(group.as_str()));
        assert_eq!(
            editor.views[&two].kind,
            SectionKind::ListItem {
                checked: Some(true)
            }
        );
        assert!(
            editor
                .document
                .sections()
                .iter()
                .any(|block| block.id == child)
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn nested_list_boundary_pulls_an_item_up_with_its_subtree(cx: &mut TestAppContext) {
    let (handle, editor, parent) = setup("- parent\n  - nested\n    - child\n- tail", cx);
    let (nested, child, tail, group, outer) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let nested = sections
            .iter()
            .find(|block| block.content.plain_text() == "nested")
            .unwrap();
        let child = sections
            .iter()
            .find(|block| block.content.plain_text() == "child")
            .unwrap();
        let tail = sections
            .iter()
            .find(|block| block.content.plain_text() == "tail")
            .unwrap();
        (
            nested.id.clone(),
            child.id.clone(),
            tail.id.clone(),
            nested.parent.clone().unwrap(),
            tail.parent.clone().unwrap(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        drag_grip(
            &editor,
            &nested,
            SharedString::from(format!("list-after-{group}")),
            window,
            cx,
        );
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let items = editor.document.children(Some(&outer));
        assert!(items.iter().map(|block| block.id.as_str()).eq([
            parent.as_str(),
            nested.as_str(),
            tail.as_str()
        ]));
        let sections = editor.document.sections();
        assert!(!sections.iter().any(|block| block.id == group));
        let child_group = sections
            .iter()
            .find(|block| Some(&block.id) == editor.views[&child].parent.as_ref())
            .unwrap();
        assert_eq!(child_group.parent.as_deref(), Some(nested.as_str()));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn range_and_toggle_selection_drag_in_document_order_and_undo_once(cx: &mut TestAppContext) {
    let (handle, editor, a) = setup("A\n\nB\n\nC\n\nD\n\nE", cx);
    let (b, c, d, e) = editor.read_with(cx, |editor, _| {
        (
            editor.order[1].clone(),
            editor.order[2].clone(),
            editor.order[3].clone(),
            editor.order[4].clone(),
        )
    });
    let mut peer = replica(&editor, cx);
    let vector = peer.state_vector();
    set_text(&mut peer, &b, "peer B");
    let update = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        click_grip(&editor, &b, Modifiers::default(), window, cx);
        click_grip(
            &editor,
            &d,
            Modifiers {
                shift: true,
                ..Default::default()
            },
            window,
            cx,
        );
        click_grip(
            &editor,
            &c,
            Modifiers {
                platform: true,
                ..Default::default()
            },
            window,
            cx,
        );
        click_grip(
            &editor,
            &e,
            Modifiers {
                control: true,
                ..Default::default()
            },
            window,
            cx,
        );
        click_grip(
            &editor,
            &e,
            Modifiers {
                platform: true,
                ..Default::default()
            },
            window,
            cx,
        );
        drag_grip(&editor, &d, SharedString::from(a.clone()), window, cx);
        window.hover("add-section", cx);
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(
            editor.order.iter().map(String::as_str).eq([
                b.as_str(),
                d.as_str(),
                a.as_str(),
                c.as_str(),
                e.as_str()
            ]),
            "{:?}",
            editor.order
        );
        assert_eq!(editor.views[&b].content.plain_text(), "peer B");
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.order.iter().map(String::as_str).eq([
            a.as_str(),
            b.as_str(),
            c.as_str(),
            d.as_str(),
            e.as_str()
        ]));
        assert_eq!(editor.views[&b].content.plain_text(), "peer B");
    });
    // After a completed drop, Shift-click starts a fresh selection rather
    // than extending from the last dragged block's stale anchor.
    cx.update_window(handle, |_, window, cx| {
        click_grip(
            &editor,
            &e,
            Modifiers {
                shift: true,
                ..Default::default()
            },
            window,
            cx,
        );
        drag_grip(&editor, &e, SharedString::from(a.clone()), window, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.order.iter().map(String::as_str).eq([
            e.as_str(),
            a.as_str(),
            b.as_str(),
            c.as_str(),
            d.as_str()
        ]));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        click_grip(&editor, &b, Modifiers::default(), window, cx);
        click_grip(
            &editor,
            &d,
            Modifiers {
                platform: true,
                ..Default::default()
            },
            window,
            cx,
        );
        window.press("escape", cx);
        drag_grip(&editor, &d, SharedString::from(a.clone()), window, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.order.iter().map(String::as_str).eq([
            d.as_str(),
            a.as_str(),
            b.as_str(),
            c.as_str(),
            e.as_str()
        ]));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn selecting_all_items_drags_the_original_list_container(cx: &mut TestAppContext) {
    let (handle, editor, one) = setup("7. one\n   - child\n8. two\n9. three\n\nTail", cx);
    let (three, tail, group, descendants) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let group = editor.views[&one].parent.clone().unwrap();
        (
            sections
                .iter()
                .find(|block| block.content.plain_text() == "three")
                .unwrap()
                .id
                .clone(),
            sections
                .iter()
                .find(|block| block.content.plain_text() == "Tail")
                .unwrap()
                .id
                .clone(),
            group.clone(),
            sections
                .iter()
                .filter(|block| block.id != group && block.content.plain_text() != "Tail")
                .map(|block| block.id.clone())
                .collect::<Vec<_>>(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        click_grip(&editor, &one, Modifiers::default(), window, cx);
        click_grip(
            &editor,
            &three,
            Modifiers {
                shift: true,
                ..Default::default()
            },
            window,
            cx,
        );
        drag_grip(&editor, &three, "append-section", window, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let roots = editor.document.children(None);
        assert!(
            roots
                .iter()
                .map(|block| block.id.as_str())
                .eq([tail.as_str(), group.as_str()])
        );
        assert_eq!(
            roots[1].kind,
            SectionKind::List {
                ordered: true,
                start: 7
            }
        );
        let sections = editor.document.sections();
        assert!(
            descendants
                .iter()
                .all(|id| sections.iter().any(|block| &block.id == id))
        );
        assert!(
            editor
                .markdown()
                .contains("7. one\n   - child\n8. two\n9. three"),
            "{}",
            editor.markdown()
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn alt_selection_moves_a_single_item_list_before_another_list(cx: &mut TestAppContext) {
    let (handle, editor, only) = setup("- only\n\nTail\n\n- other", cx);
    let (group, other_group, tail) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let other = sections
            .iter()
            .find(|block| block.content.plain_text() == "other")
            .unwrap();
        (
            editor.views[&only].parent.clone().unwrap(),
            other.parent.clone().unwrap(),
            sections
                .iter()
                .find(|block| block.content.plain_text() == "Tail")
                .unwrap()
                .id
                .clone(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        click_grip(
            &editor,
            &only,
            Modifiers {
                alt: true,
                ..Default::default()
            },
            window,
            cx,
        );
        drag_grip(
            &editor,
            &only,
            SharedString::from(format!("list-before-{other_group}")),
            window,
            cx,
        );
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let roots = editor.document.children(None);
        assert!(roots.iter().map(|block| block.id.as_str()).eq([
            tail.as_str(),
            group.as_str(),
            other_group.as_str()
        ]));
        assert_eq!(editor.views[&only].parent.as_deref(), Some(group.as_str()));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn dragging_a_parent_onto_its_descendant_is_a_silent_noop(cx: &mut TestAppContext) {
    let (handle, editor, parent) = setup("- parent\n  - child\n- other", cx);
    let (child, before) = editor.read_with(cx, |editor, _| {
        (
            editor
                .document
                .sections()
                .iter()
                .find(|block| block.content.plain_text() == "child")
                .unwrap()
                .id
                .clone(),
            editor.document.sections(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        drag_grip(
            &editor,
            &parent,
            SharedString::from(child.clone()),
            window,
            cx,
        );
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections(), before);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}
