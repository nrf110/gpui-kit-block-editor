use super::collaboration_tests::{replica, select, setup};
use crate::{CommandAction, RichKind, RichNode};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use markdown_editor::{
    BlockKind as NativeBlockKind, BlockSelection, EditorCommand, MarkKind, TableEdit,
};

#[gpui_kit::test]
fn a_remote_mark_on_converted_code_keeps_the_native_editor_usable_without_rewriting_peer_state(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("hello😀", cx);
    let mut peer = replica(&editor, cx);
    let vector = peer.state_vector();
    let mut marked = peer.sections()[0].content.clone();
    marked.blocks[0].runs[0].marks.bold = true;
    peer.set_content(&id, &marked).unwrap();
    let incoming = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        editor.update(cx, |editor, cx| {
            editor.execute_command(
                &id,
                CommandAction::Editor(EditorCommand::SetCodeBlock {
                    lang: Some("rust".into()),
                }),
                None,
                window,
                cx,
            )
        });
        editor.update(cx, |editor, cx| {
            editor.apply_update(&incoming, window, cx).unwrap()
        });
        window.render_frame(cx);
    })
    .unwrap();
    let merged = editor.read_with(cx, |editor, _| editor.document.encode_state());
    cx.update_window(handle, |_, window, cx| {
        window.press("right", cx);
        window.render_frame(cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert_eq!(editor.document.encode_state(), merged);
        let state = editor.views[&id].editor.read(cx);
        assert_eq!(
            state.block_kind_at_caret(),
            Some(NativeBlockKind::CodeBlock)
        );
        assert_eq!(
            state.model().rich_document().unwrap().plain_text(),
            "hello😀"
        );
        assert_eq!(
            state.model().mark_state(MarkKind::Bold),
            markdown_editor::MarkState::Off
        );
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        let body = &editor.views[&id].content;
        assert_eq!(body.blocks[0].kind, RichKind::Paragraph);
        assert!(body.blocks[0].runs[0].marks.bold);
        assert_eq!(body.plain_text(), "hello😀");
    });
}

#[gpui_kit::test]
fn a_concurrent_row_and_column_merge_retains_editable_native_cells(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("| a | b |\n| --- | --- |\n| c | d |", cx);
    let mut peer = replica(&editor, cx);
    let vector = peer.state_vector();
    let mut body = peer.sections()[0].content.clone();
    let mut first = RichNode::paragraph("Peer");
    first.kind = RichKind::TableCell;
    let cell_id = first.id.clone();
    let mut second = RichNode::paragraph("row");
    second.kind = RichKind::TableCell;
    let mut row = RichNode::paragraph("");
    row.kind = RichKind::TableRow;
    row.children = vec![first, second];
    body.blocks[0].children.push(row);
    peer.set_content(&id, &body).unwrap();
    let incoming = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        editor.update(cx, |editor, cx| {
            editor.execute_command(
                &id,
                CommandAction::Editor(EditorCommand::EditTable(TableEdit::AddColumnRight)),
                None,
                window,
                cx,
            )
        });
        editor.update(cx, |editor, cx| {
            editor.apply_update(&incoming, window, cx).unwrap()
        });
        window.render_frame(cx);
    })
    .unwrap();
    let leaf = editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let state = editor.views[&id].editor.read(cx);
        let native_id = state.model().block_id(&cell_id).unwrap();
        state
            .model()
            .document()
            .leaves()
            .position(|leaf| leaf.id() == native_id)
            .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, leaf, 4, 4, window, cx);
        window.render_frame(cx);
        window.input("!", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let table = &editor.views[&id].content.blocks[0];
        let cell = table
            .children
            .iter()
            .flat_map(|row| &row.children)
            .find(|cell| cell.id == cell_id)
            .unwrap();
        assert_eq!(cell.text(), "Peer!");
        assert!(editor.markdown().contains("Peer!"));
        assert!(editor.markdown().contains("row"));
    });
}

#[gpui_kit::test]
fn concurrent_row_removals_leave_a_focusable_table_object_that_can_be_deleted(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("| header |\n| --- |\n| body |", cx);
    let mut peer = replica(&editor, cx);
    let vector = peer.state_vector();
    let mut body = peer.sections()[0].content.clone();
    body.blocks[0].children.pop();
    peer.set_content(&id, &body).unwrap();
    let incoming = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        editor.update(cx, |editor, cx| {
            editor.execute_command(
                &id,
                CommandAction::Editor(EditorCommand::EditTable(TableEdit::RemoveRow)),
                None,
                window,
                cx,
            )
        });
        editor.update(cx, |editor, cx| {
            editor.apply_update(&incoming, window, cx).unwrap()
        });
        window.render_frame(cx);
    })
    .unwrap();
    let empty = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.error.is_none(), "{:?}", editor.error);
                assert!(editor.views[&id].content.blocks[0].children.is_empty());
                editor.insert_section(None, "", window, cx).unwrap()
            })
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &empty, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.press("backspace", cx);
        assert!(!editor.read(cx).views.contains_key(&empty));
        assert!(matches!(
            editor.read(cx).views[&id].editor.read(cx).selection(),
            BlockSelection::Blocks { .. }
        ));
        assert!(
            editor.read(cx).error.is_none(),
            "{:?}",
            editor.read(cx).error
        );
        window.press("backspace", cx);
        window.input("Recovered", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert_eq!(editor.views[&id].content.plain_text(), "Recovered");
        assert_eq!(
            editor.views[&id].content.blocks[0].kind,
            RichKind::Paragraph
        );
    });
}
