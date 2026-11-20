use super::collaboration_tests::{replica, setup};
use crate::{BlockEditor, RichDocument};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext, Window,
};
use markdown_editor::{BlockSelection, TableLine};

fn open_menu(
    editor: &Entity<BlockEditor>,
    id: &str,
    line: TableLine,
    window: &mut Window,
    cx: &mut App,
) {
    let native = editor.read(cx).views[id].editor.clone();
    native.update(cx, |state, cx| {
        state.focus(window, cx);
        let table = state
            .model()
            .document()
            .root()
            .children()
            .next()
            .unwrap()
            .id();
        state.open_table_menu_for(table, line, cx);
    });
    window.render_frame(cx);
}

fn click_menu_item(handle: AnyWindowHandle, selector: &'static str, cx: &TestAppContext) {
    let mut visual = VisualTestContext::from_window(handle, cx);
    let bounds = visual
        .debug_bounds(selector)
        .expect("visible native menu item");
    visual.simulate_click(bounds.center(), Modifiers::none());
    visual.run_until_parked();
    visual.update(|window, cx| window.render_frame(cx));
}

fn content(editor: &Entity<BlockEditor>, id: &str, cx: &TestAppContext) -> RichDocument {
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        editor
            .document
            .sections()
            .into_iter()
            .find(|section| section.id == id)
            .unwrap()
            .content
    })
}

#[gpui_kit::test]
fn table_menu_edits_persist_sync_and_undo_without_undoing_peer_text(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("| a | b |\n| --- | --- |\n| c | d |", cx);
    let mut peer = replica(&editor, cx);
    let initial = content(&editor, &id, cx);
    let mut peer_content = initial.clone();
    peer_content.blocks[0].children[0].children[0].runs[0].text = "Peer".into();
    let vector = peer.state_vector();
    peer.set_content(&id, &peer_content).unwrap();
    let incoming = peer.encode_update(&vector).unwrap();

    cx.update_window(handle, |_, window, cx| {
        open_menu(&editor, &id, TableLine::Column(1), window, cx);
    })
    .unwrap();
    click_menu_item(handle, "tm-col-right", cx);
    let inserted = content(&editor, &id, cx);
    assert!(
        inserted.blocks[0]
            .children
            .iter()
            .all(|row| row.children.len() == 3)
    );
    for (row, original) in inserted.blocks[0]
        .children
        .iter()
        .zip(&initial.blocks[0].children)
    {
        assert_eq!(&row.children[..2], &original.children);
        assert!(row.children[2].runs.is_empty());
    }

    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&incoming, window, cx).unwrap()
        });
        window.press("cmd-z", cx);
    })
    .unwrap();
    assert_eq!(content(&editor, &id, cx), peer_content);

    cx.update_window(handle, |_, window, cx| {
        open_menu(&editor, &id, TableLine::Row(1), window, cx);
    })
    .unwrap();
    click_menu_item(handle, "tm-row-delete", cx);
    let removed = content(&editor, &id, cx);
    assert_eq!(
        removed.blocks[0].children,
        peer_content.blocks[0].children[..1]
    );
    peer.apply_update(&editor.read_with(cx, |editor, _| editor.document.encode_state()))
        .unwrap();
    assert_eq!(
        peer.sections(),
        editor.read_with(cx, |editor, _| editor.document.sections())
    );

    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    assert_eq!(content(&editor, &id, cx), peer_content);
}

#[gpui_kit::test]
fn clicking_another_block_deselects_the_table_and_keeps_typing_in_that_block(
    cx: &mut TestAppContext,
) {
    let (handle, editor, id) = setup("| a | b |\n| --- | --- |\n| c | d |\n\nAfter", cx);
    let initial = content(&editor, &id, cx);
    cx.update_window(handle, |_, window, cx| {
        let native = editor.read(cx).views[&id].editor.clone();
        native.update(cx, |state, cx| {
            let table = state
                .model()
                .document()
                .root()
                .children()
                .next()
                .unwrap()
                .id();
            state
                .set_selection(
                    BlockSelection::Blocks {
                        anchor: table,
                        head: table,
                    },
                    cx,
                )
                .unwrap();
            state.focus(window, cx);
        });
        window.render_frame(cx);
        window.click(("section-source", 1usize), cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        let table = editor.views[&id].editor.read(cx);
        assert!(table.selection().is_collapsed());
    });
    cx.update_window(handle, |_, window, cx| window.input("!", cx))
        .unwrap();
    assert_eq!(content(&editor, &id, cx), initial);
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections()[1].content.plain_text(), "After!");
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}
