use super::collaboration_tests::{mount, replica, select, setup};
use super::{BlockEditor, SectionKind};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Entity, ScrollDelta, SharedString, TestAppContext, point, px,
};

fn create(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<BlockEditor>, String, String) {
    let (handle, editor, title) = setup("", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &title, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/collapsible", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    let body = editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert_eq!(
            editor.views[&title].kind,
            SectionKind::Collapsible {
                default_collapsed: true
            }
        );
        assert_eq!(editor.views[&title].content.plain_text(), "Details");
        editor.document.children(Some(&title))[0].id.clone()
    });
    (handle, editor, title, body)
}

#[gpui_kit::test]
fn slash_collapsible_is_one_undo_group_and_redo_restores_the_subtree(cx: &mut TestAppContext) {
    let (handle, editor, title, body) = create(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(SharedString::from(body.clone())).visible());
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections().len(), 1);
        assert_eq!(editor.views[&title].content.plain_text(), "/collapsible");
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-shift-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(
            editor.views[&title].kind,
            SectionKind::Collapsible {
                default_collapsed: true
            }
        );
        assert_eq!(editor.document.children(Some(&title))[0].id, body);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn folding_hides_the_body_without_changing_shared_content(cx: &mut TestAppContext) {
    let (handle, editor, title, body) = create(cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &body, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("A retained body", cx);
    })
    .unwrap();
    let vector = editor.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(handle, |_, window, cx| {
        window.click(("toggle-collapse", 0usize), cx);
        assert!(window.find(SharedString::from(title.clone())).visible());
        assert!(
            !window
                .try_find(SharedString::from(body.clone()))
                .is_some_and(|row| row.visible())
        );
        assert_eq!(editor.read(cx).document.state_vector(), vector);
        window.click(("toggle-collapse", 0usize), cx);
        assert!(window.find(SharedString::from(body.clone())).visible());
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert_eq!(editor.document.state_vector(), vector);
        assert_eq!(
            editor.views[&body]
                .editor
                .read(cx)
                .model()
                .rich_document()
                .unwrap()
                .plain_text(),
            "A retained body"
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn replicas_fold_independently_and_hidden_bodies_receive_peer_edits(cx: &mut TestAppContext) {
    let (left_handle, left, _title, body) = create(cx);
    let (right_handle, right) = mount(replica(&left, cx), cx);
    cx.update_window(right_handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            !window
                .try_find(SharedString::from(body.clone()))
                .is_some_and(|row| row.visible())
        );
        window.click(("toggle-collapse", 0usize), cx);
        assert!(window.find(SharedString::from(body.clone())).visible());
    })
    .unwrap();
    cx.update_window(left_handle, |_, window, cx| {
        window.click(("toggle-collapse", 0usize), cx);
        assert!(
            !window
                .try_find(SharedString::from(body.clone()))
                .is_some_and(|row| row.visible())
        );
    })
    .unwrap();
    let vector = right.read_with(cx, |editor, _| editor.document.state_vector());
    cx.update_window(right_handle, |_, window, cx| {
        select(&right, &body, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("Peer-edited hidden content", cx);
    })
    .unwrap();
    let update = right.read_with(cx, |editor, _| {
        editor.document.encode_update(&vector).unwrap()
    });
    cx.update_window(left_handle, |_, window, cx| {
        left.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        window.render_frame(cx);
        assert!(
            !window
                .try_find(SharedString::from(body.clone()))
                .is_some_and(|row| row.visible())
        );
        window.click(("toggle-collapse", 0usize), cx);
        assert!(window.find(SharedString::from(body.clone())).visible());
    })
    .unwrap();
    left.read_with(cx, |editor, cx| {
        assert_eq!(
            editor.views[&body]
                .editor
                .read(cx)
                .model()
                .rich_document()
                .unwrap()
                .plain_text(),
            "Peer-edited hidden content"
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    cx.update_window(right_handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(SharedString::from(body)).visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn dragging_a_collapsible_title_moves_children_and_trash_undo_restores_them(
    cx: &mut TestAppContext,
) {
    let (handle, editor, title, body) = create(cx);
    let tail = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert_section(None, "Tail", window, cx).unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(title.clone()), cx);
        let from = window.find(("grip", 0usize)).bounds().center();
        let to = window.find("append-section").bounds().center();
        window.drag(from, to, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(
            editor
                .document
                .children(None)
                .iter()
                .map(|block| block.id.as_str())
                .eq([tail.as_str(), title.as_str()])
        );
        assert_eq!(editor.document.children(Some(&title))[0].id, body);
    });
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(title.clone()), cx);
        window.click(("remove", 1usize), cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(
            !editor
                .document
                .sections()
                .iter()
                .any(|block| block.id == title || block.id == body)
        );
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.children(Some(&title))[0].id, body);
        assert!(matches!(
            editor.views[&title].kind,
            SectionKind::Collapsible { .. }
        ));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn dragging_a_collapsible_into_a_list_does_not_erase_its_container_kind(cx: &mut TestAppContext) {
    let (handle, editor, title, _body) = create(cx);
    let target = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(None, "- item\n- other", window, cx)
                    .unwrap()
            })
        })
        .unwrap();
    let before = editor.read_with(cx, |editor, _| editor.document.sections());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(SharedString::from(title), cx);
        let from = window.find(("grip", 0usize)).bounds().center();
        let to = window.find(SharedString::from(target)).bounds().center();
        window.drag(from, to, cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections(), before);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn mermaid_inserted_inside_a_collapsible_keeps_preview_and_source_switching(
    cx: &mut TestAppContext,
) {
    let (handle, editor, title, body) = create(cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &body, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/mermaid", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    let diagram = editor.read_with(cx, |editor, _| {
        let children = editor.document.children(Some(&title));
        let diagram = children
            .iter()
            .find(|block| {
                block
                    .content
                    .blocks
                    .first()
                    .is_some_and(|node| node.kind == crate::RichKind::CodeBlock)
            })
            .unwrap();
        assert!(editor.views[&diagram.id].previewable);
        diagram.id.clone()
    });
    cx.update_window(handle, |_, window, cx| {
        window.click(("toggle-collapse", 0usize), cx);
        assert!(
            !window
                .try_find(SharedString::from(diagram.clone()))
                .is_some_and(|row| row.visible())
        );
        window.click(("toggle-collapse", 0usize), cx);
        window.hover(SharedString::from(diagram.clone()), cx);
        window.click(("toggle-preview", 2usize), cx);
        assert!(window.find(("section-source", 2usize)).visible());
        window.click(("toggle-preview", 2usize), cx);
        assert!(window.find(("section-preview", 2usize)).visible());
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let node = &editor.views[&diagram].content.blocks[0];
        assert_eq!(
            node.attrs.get("lang").and_then(serde_json::Value::as_str),
            Some("mermaid")
        );
        assert!(node.text().contains("flowchart"));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn enter_in_a_collapsible_title_creates_a_child_without_unwrapping_the_section(
    cx: &mut TestAppContext,
) {
    let (handle, editor, title, _body) = create(cx);
    cx.update_window(handle, |_, window, cx| {
        select(
            &editor,
            &title,
            0,
            "Details".len(),
            "Details".len(),
            window,
            cx,
        );
        window.render_frame(cx);
        window.press("enter", cx);
        window.input("New child", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        let roots = editor.document.children(None);
        assert_eq!(roots[0].id, title);
        assert!(matches!(roots[0].kind, SectionKind::Collapsible { .. }));
        assert_eq!(roots[0].content.plain_text(), "Details");
        assert!(
            editor
                .document
                .children(Some(&title))
                .iter()
                .any(|child| child.content.plain_text() == "New child")
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn bare_slash_menu_scrolls_to_collapsible_and_accepts_a_mouse_click(cx: &mut TestAppContext) {
    let (handle, editor, title) = setup("", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &title, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.scroll(
            "slash-menu-items",
            ScrollDelta::Pixels(point(px(0.), px(-2000.))),
            cx,
        );
        let viewport = window.find("slash-menu-items").bounds();
        let command = window.find("collapsible").bounds();
        assert!(viewport.contains(&command.center()));
        window.click("collapsible", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(matches!(
            editor.views[&title].kind,
            SectionKind::Collapsible { .. }
        ));
        assert_eq!(editor.document.children(Some(&title)).len(), 1);
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn keyboard_navigation_reveals_commands_beyond_the_initial_menu_viewport(cx: &mut TestAppContext) {
    let (handle, editor, title) = setup("", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &title, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/", cx);
    })
    .unwrap();
    let index = editor.read_with(cx, |editor, _| {
        editor
            .completion
            .as_ref()
            .unwrap()
            .commands
            .iter()
            .position(|command| command.id == "collapsible")
            .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        for _ in 0..index {
            window.press("down", cx);
        }
        let viewport = window.find("slash-menu-items").bounds();
        let command = window.find("collapsible").bounds();
        assert!(viewport.contains(&command.center()));
        window.press("enter", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(matches!(
            editor.views[&title].kind,
            SectionKind::Collapsible { .. }
        ));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}
