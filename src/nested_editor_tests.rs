use super::SectionKind;
use super::collaboration_tests::{replica, select, setup};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, SharedString, TestAppContext};

#[gpui_kit::test]
fn enter_splits_a_rich_block_and_undo_restores_it(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("hello **world**", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 6, 6, window, cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    let next = editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let roots = editor.document.children(None);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].id, id);
        assert!(roots[1].markdown.contains("**world**"));
        let state = editor.views[&roots[1].id].editor.read(cx);
        assert_eq!(state.model().caret().unwrap().offset, 0);
        roots[1].id.clone()
    });
    cx.update_window(handle, |_, window, cx| window.input("new ", cx))
        .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert_eq!(
            editor.views[&next]
                .editor
                .read(cx)
                .model()
                .document()
                .leaves()
                .next()
                .unwrap()
                .text(),
            Some("new world")
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-z", cx);
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let roots = editor.document.children(None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, id);
        assert_eq!(editor.markdown(), "hello **world**");
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn tab_nests_a_list_item_with_its_subtree_and_shift_tab_lifts_it(cx: &mut TestAppContext) {
    let (handle, editor, one) = setup("- one\n- two\n  - child", cx);
    let (two, child, group, child_group) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let two = sections
            .iter()
            .find(|section| section.markdown == "two")
            .unwrap();
        let child = sections
            .iter()
            .find(|section| section.markdown == "child")
            .unwrap();
        (
            two.id.clone(),
            child.id.clone(),
            two.parent.clone().unwrap(),
            child.parent.clone().unwrap(),
        )
    });
    let before_x = cx
        .update_window(handle, |_, window, cx| {
            select(&editor, &two, 0, 3, 3, window, cx);
            window.render_frame(cx);
            let before = window.find(("section-source", 1usize)).bounds().origin.x;
            window.press("tab", cx);
            assert!(window.find(("section-source", 1usize)).bounds().origin.x > before);
            before
        })
        .unwrap();
    editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let nested_group = sections
            .iter()
            .find(|section| Some(&section.id) == editor.views[&two].parent.as_ref())
            .unwrap();
        assert_eq!(nested_group.parent.as_deref(), Some(one.as_str()));
        assert_eq!(
            sections
                .iter()
                .find(|section| section.id == child)
                .unwrap()
                .parent
                .as_deref(),
            Some(child_group.as_str())
        );
        assert_eq!(
            sections
                .iter()
                .find(|section| section.id == child_group)
                .unwrap()
                .parent
                .as_deref(),
            Some(two.as_str())
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| {
        window.press("shift-tab", cx);
        assert_eq!(
            window.find(("section-source", 1usize)).bounds().origin.x,
            before_x
        );
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.views[&two].parent.as_deref(), Some(group.as_str()))
    });
}

#[gpui_kit::test]
fn task_enter_creates_an_unchecked_sibling_and_empty_enter_exits(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("- [x] done", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 4, 4, window, cx);
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    let next = editor.read_with(cx, |editor, _| {
        let parent = editor.views[&id].parent.as_deref();
        let items = editor.document.children(parent);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[1].kind,
            SectionKind::ListItem {
                checked: Some(false)
            }
        );
        assert_eq!(items[1].markdown, "");
        items[1].id.clone()
    });
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.views[&next].kind, SectionKind::Text);
        assert!(editor.views[&next].parent.is_none());
        assert!(editor.markdown().contains("[x] done"));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| window.click(("task", 0usize), cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.markdown().contains("[ ] done"))
    });
}

#[gpui_kit::test]
fn dragging_an_item_keeps_children_peer_text_and_undo(cx: &mut TestAppContext) {
    let (handle, editor, one) = setup("- one\n- two\n  - child\n- three", cx);
    let mut peer = replica(&editor, cx);
    let (two, child, parent) = editor.read_with(cx, |editor, _| {
        let sections = editor.document.sections();
        let two = sections
            .iter()
            .find(|section| section.markdown == "two")
            .unwrap();
        let child = sections
            .iter()
            .find(|section| section.markdown == "child")
            .unwrap();
        (
            two.id.clone(),
            child.id.clone(),
            two.parent.clone().unwrap(),
        )
    });
    let vector = peer.state_vector();
    peer.set_markdown(&two, "peer two").unwrap();
    let update = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &two, 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.hover(SharedString::from(two.clone()), cx);
        let from = window.find(("grip", 1usize)).bounds().center();
        let to = window
            .find(SharedString::from(one.clone()))
            .bounds()
            .center();
        window.drag(from, to, cx);
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert_eq!(editor.document.children(Some(&parent))[0].id, two);
        assert!(
            editor
                .document
                .sections()
                .iter()
                .any(|section| section.id == child && section.markdown == "child")
        );
        let state = editor.views[&two].editor.read(cx);
        assert_eq!(state.source(), "peer two");
        assert_eq!(state.model().caret().unwrap().offset, "peer two".len());
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| assert!(editor.undo(window, cx).unwrap()))
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let items = editor.document.children(Some(&parent));
        assert_eq!(items[0].id, one);
        assert_eq!(items[1].id, two);
        assert_eq!(items[1].markdown, "peer two");
    });
}

#[gpui_kit::test]
fn list_end_drop_renumbers_items_and_cross_group_drop_transfers_them(cx: &mut TestAppContext) {
    let (handle, editor, first) = setup("3. three\n4. four\n\nseparator\n\n- other", cx);
    let (parent, other) = editor.read_with(cx, |editor, _| {
        (
            editor.views[&first].parent.clone().unwrap(),
            editor
                .document
                .sections()
                .iter()
                .find(|section| section.markdown == "other")
                .unwrap()
                .id
                .clone(),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(first.clone()), cx);
        let from = window.find(("grip", 0usize)).bounds().center();
        let to = window
            .find(SharedString::from(format!("list-end-{parent}")))
            .bounds()
            .center();
        window.drag(from, to, cx);
    })
    .unwrap();
    let other_parent = editor.read_with(cx, |editor, _| {
        assert!(
            editor.markdown().starts_with("3. four\n4. three"),
            "{}",
            editor.markdown()
        );
        editor.views[&other].parent.clone().unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(first.clone()), cx);
        let from = window.find(("grip", 1usize)).bounds().center();
        let to = window
            .find(SharedString::from(other.clone()))
            .bounds()
            .center();
        window.drag(from, to, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let moved = editor.document.children(Some(&other_parent));
        assert!(
            moved
                .iter()
                .map(|block| block.id.as_str())
                .eq([first.as_str(), other.as_str()])
        );
        let remaining = editor.document.children(Some(&parent));
        assert_eq!(remaining[0].markdown, "four");
        assert_eq!(remaining.len(), 1);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn ordinary_blocks_can_nest_and_newlines_create_siblings(cx: &mut TestAppContext) {
    let (handle, editor, one) = setup("one\n\ntwo", cx);
    let two = editor.read_with(cx, |editor, _| editor.order[1].clone());
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &two, 0, 3, 3, window, cx);
        window.render_frame(cx);
        window.press("tab", cx);
        window.press("shift-enter", cx);
        window.input("continued", cx);
    })
    .unwrap();
    let continued = editor.read_with(cx, |editor, _| {
        assert_eq!(editor.views[&two].parent.as_deref(), Some(one.as_str()));
        let children = editor.document.children(Some(&one));
        assert_eq!(
            children
                .iter()
                .map(|block| block.markdown.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "continued"]
        );
        assert_eq!(children[0].id, two);
        assert!(editor.error.is_none(), "{:?}", editor.error);
        children[1].id.clone()
    });
    cx.update_window(handle, |_, window, cx| window.press("shift-tab", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.views[&continued].parent.is_none());
        assert_eq!(editor.views[&two].parent.as_deref(), Some(one.as_str()));
    });
}

#[gpui_kit::test]
fn code_and_table_newlines_remain_internal(cx: &mut TestAppContext) {
    let (handle, editor, code) = setup("```rust\nlet value = 1;\n```", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &code, 0, 14, 14, window, cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.input("next", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.order.len(), 1);
        assert!(editor.markdown().contains("let value = 1;\nnext"));
    });
    let table = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(None, "| A | B |\n| --- | --- |\n| x | y |", window, cx)
                    .unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &table, 0, 1, 1, window, cx);
        window.render_frame(cx);
        window.press("shift-enter", cx);
        window.input("cell", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.order.len(), 2);
        assert!(editor.markdown().contains("cell"));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn enter_in_an_empty_block_creates_editable_siblings(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("enter", cx);
        window.input("text", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let roots = editor.document.children(None);
        assert_eq!(roots[0].id, id);
        assert_eq!(
            roots
                .iter()
                .map(|block| block.markdown.as_str())
                .collect::<Vec<_>>(),
            vec!["", "", "text"]
        );
    });
}
