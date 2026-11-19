use super::{BlockEditor, Document, DocumentEvent, ExtensionRegistry};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Entity, EntityInputHandler, SharedString,
    TestAppContext, Window, WindowBounds, WindowOptions, point, px, size,
};
use markdown_editor::{BlockPosition, BlockSelection};
use std::{cell::RefCell, rc::Rc, time::Duration};
use yrs::{
    sync::{Message, SyncMessage},
    updates::encoder::Encode,
};

pub(super) fn setup(
    source: &str,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<BlockEditor>, String) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut document = Document::new();
    let id = document.insert_section(None, source).unwrap();
    document.clear_history();
    let (window, editor) = mount(document, cx);
    (window, editor, id)
}

pub(super) fn mount(
    document: Document,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<BlockEditor>) {
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(900.), px(900.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| BlockEditor::new(document, ExtensionRegistry::default(), window, cx))
            },
        )
        .unwrap()
    })
}

pub(super) fn set_text(document: &mut Document, id: &str, text: &str) {
    let mut content = document
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .unwrap()
        .content;
    content.blocks[0].runs = if text.is_empty() {
        Vec::new()
    } else {
        vec![crate::RichRun {
            text: text.to_owned(),
            ..Default::default()
        }]
    };
    document.set_content(id, &content).unwrap();
}

pub(super) fn select(
    editor: &Entity<BlockEditor>,
    id: &str,
    leaf: usize,
    anchor: usize,
    head: usize,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| {
        editor.views[id].editor.update(cx, |state, cx| {
            let block = state.model().document().leaves().nth(leaf).unwrap().id();
            state
                .set_selection(
                    BlockSelection::Text {
                        anchor: BlockPosition::new(block, anchor),
                        head: BlockPosition::new(block, head),
                    },
                    cx,
                )
                .unwrap();
            state.focus(window, cx);
        });
    });
}

pub(super) fn replica(editor: &Entity<BlockEditor>, cx: &TestAppContext) -> Document {
    let mut remote = Document::new();
    remote
        .apply_update(&editor.read_with(cx, |editor, _| editor.document.encode_state()))
        .unwrap();
    remote
}

#[gpui_kit::test]
fn remote_structure_and_marks_keep_the_caret_in_the_same_content(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("same **bold** 👋", cx);
    let mut remote = replica(&editor, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 5, window, cx)
    })
    .unwrap();
    let vector = remote.state_vector();
    remote.insert_section(Some(&id), "Inserted").unwrap();
    let mut content = remote
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .unwrap()
        .content;
    content.blocks[0]
        .runs
        .iter_mut()
        .find(|run| run.marks.bold)
        .unwrap()
        .marks
        .italic = true;
    content.blocks[0].runs.last_mut().unwrap().text.push('!');
    remote.set_content(&id, &content).unwrap();
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        editor.read_with(cx, |editor, cx| {
            assert!(editor.error.is_none(), "{:?}", editor.error);
            let state = editor.views[&id].editor.read(cx);
            let caret = state.model().caret().unwrap();
            assert_eq!(caret.offset, 5);
            assert_eq!(
                state.model().document().block(caret.block).unwrap().text(),
                Some("same bold 👋!")
            );
        });
        window.render_frame(cx);
        window.input("LOCAL ", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        let state = editor.views[&id].editor.read(cx);
        assert!(
            state
                .model()
                .document()
                .leaves()
                .any(|leaf| leaf.text() == Some("same LOCAL bold 👋!"))
        );
        assert!(editor.document.markdown().contains("Inserted"));
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn framed_sync_preserves_a_reversed_unicode_selection(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("one 👋 two", cx);
    let mut remote = replica(&editor, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 12, 9, window, cx)
    })
    .unwrap();
    set_text(&mut remote, &id, "peer one 👋 two");
    let message = Message::Sync(SyncMessage::Update(remote.encode_state())).encode_v1();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.receive_sync(&message, window, cx).unwrap()
        });
    })
    .unwrap();
    editor.read_with(cx, |editor, cx| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let BlockSelection::Text { anchor, head } = editor.views[&id].editor.read(cx).selection()
        else {
            panic!("expected text selection")
        };
        assert_eq!(anchor.offset, 17);
        assert_eq!(head.offset, 14);
        assert_eq!(anchor.block, head.block);
    });
}

#[gpui_kit::test]
fn keyboard_undo_and_redo_keep_peer_changes_and_emit_sync_updates(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("start", cx);
    let mut remote = replica(&editor, cx);
    let updates = Rc::new(RefCell::new(Vec::new()));
    let sink = updates.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&editor, move |_, event: &DocumentEvent, _| {
            let DocumentEvent::Changed { update } = event;
            sink.borrow_mut().push(update.clone());
        })
    });
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 5, window, cx);
        window.render_frame(cx);
        window.input(" local", cx);
    })
    .unwrap();
    let vector = remote.state_vector();
    set_text(&mut remote, &id, "start peer");
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        window.render_frame(cx);
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.markdown(), "start peer");
        assert!(editor.document.can_redo());
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
    let vector = remote.state_vector();
    set_text(&mut remote, &id, "start peer!");
    let update = remote.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap()
        });
        window.render_frame(cx);
        window.press("cmd-shift-z", cx);
    })
    .unwrap();
    let source = editor.read_with(cx, |editor, _| editor.markdown());
    assert!(source.contains(" local"), "{source}");
    assert!(source.contains(" peer!"), "{source}");
    for update in updates.borrow().iter() {
        remote.apply_update(update).unwrap();
    }
    assert_eq!(remote.markdown(), source);
}

#[gpui_kit::test]
fn deleting_the_focused_section_can_be_undone_from_the_keyboard(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("A recoverable section", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.hover(SharedString::from(id.clone()), cx);
        window.click(("remove", 0usize), cx);
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections()[0].id, id);
        assert_eq!(editor.markdown(), "A recoverable section");
    });
}

#[gpui_kit::test]
fn slash_insertion_is_one_collaborative_undo_group(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.render_frame(cx);
        window.input("/mermaid", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.sections().len()),
        2
    );
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections().len(), 1);
        assert_eq!(editor.markdown(), "/mermaid");
    });
    cx.update_window(handle, |_, window, cx| window.press("cmd-shift-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections().len(), 2);
        assert!(editor.markdown().contains("```mermaid"));
        assert!(!editor.markdown().contains("/mermaid"));
    });
}

#[gpui_kit::test]
fn remote_updates_wait_for_ime_commit_without_losing_composed_text(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("start", cx);
    let mut remote = replica(&editor, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 5, window, cx);
        editor.update(cx, |editor, cx| {
            editor.views[&id].editor.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx);
            })
        });
    })
    .unwrap();
    let vector = remote.state_vector();
    set_text(&mut remote, &id, "peer start");
    let update = remote.encode_update(&vector).unwrap();
    let message = Message::Sync(SyncMessage::Update(remote.encode_state())).encode_v1();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&update, window, cx).unwrap();
            editor.receive_sync(&message, window, cx).unwrap();
            assert!(editor.views[&id].editor.read(cx).is_composing());
            assert_eq!(
                editor.views[&id]
                    .editor
                    .read(cx)
                    .model()
                    .rich_document()
                    .unwrap()
                    .plain_text(),
                "startに"
            );
            editor.views[&id]
                .editor
                .update(cx, |state, cx| state.unmark_text(window, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(32));
    cx.run_until_parked();
    editor.read_with(cx, |editor, cx| {
        assert!(editor.pending_remote.is_empty());
        assert_eq!(editor.markdown(), "peer startに");
        assert_eq!(
            editor.views[&id]
                .editor
                .read(cx)
                .model()
                .rich_document()
                .unwrap()
                .plain_text(),
            "peer startに"
        );
        assert!(editor.error.is_none(), "{:?}", editor.error);
    });
}

#[gpui_kit::test]
fn typing_after_a_silent_ime_commit_starts_a_new_undo_group(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("start", cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 5, 5, window, cx);
        window.render_frame(cx);
        editor.update(cx, |editor, cx| {
            editor.views[&id].editor.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "に", Some(1..1), window, cx);
            })
        });
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.views[&id]
                .editor
                .update(cx, |state, cx| state.unmark_text(window, cx))
        });
        window.input(" later", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| assert_eq!(editor.markdown(), "startに"));
    cx.update_window(handle, |_, window, cx| window.press("cmd-z", cx))
        .unwrap();
    editor.read_with(cx, |editor, _| assert_eq!(editor.markdown(), "start"));
}
