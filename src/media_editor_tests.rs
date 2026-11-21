use super::BlockEditor;
use super::collaboration_tests::{replica, select, set_text, setup};
use crate::{
    CommandAction, ExtensionRegistry, FileMediaStore, MediaAttachment, MediaKind, MediaStore,
    SectionKind,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Entity, ExternalPaths, FileDropEvent, SharedString,
    TestAppContext, VisualTestContext,
};
use std::{path::PathBuf, sync::Arc};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("media-editor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }
    fn store(&self) -> Arc<FileMediaStore> {
        Arc::new(FileMediaStore::new(self.0.join("assets")))
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn configure(editor: &Entity<BlockEditor>, store: Arc<FileMediaStore>, cx: &mut TestAppContext) {
    editor.update(cx, |editor, _| {
        editor.extensions = Arc::new(ExtensionRegistry::default().with_media_store(store));
    });
}

fn finish(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn image_menu_cancellation_preserves_text_and_success_renders_a_portable_image(
    cx: &mut TestAppContext,
) {
    let files = Files::new();
    let store = files.store();
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"16\"><rect width=\"32\" height=\"16\" fill=\"blue\"/></svg>";
    let path = files.file("picture.svg", svg);
    let (handle, editor, id) = setup("", cx);
    configure(&editor, store.clone(), cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 0, window, cx);
        window.input("/image", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    let before = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories);
        None
    });
    finish(handle, cx);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.export_json().unwrap()),
        before
    );
    assert!(!files.0.join("assets").exists());

    // The + menu uses the same file chooser without inserting a slash token.
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(id.clone()), cx);
        window.click(("block-menu", 0usize), cx);
        let viewport = window.find("slash-menu-items").bounds();
        let command = window.find("image").bounds();
        window.scroll(
            "slash-menu-items",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                viewport.bottom() - command.bottom() - gpui_kit::px(4.),
            )),
            cx,
        );
        window.click("image", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![path]));
    finish(handle, cx);
    let section = editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let sections = editor.document.sections();
        sections
            .iter()
            .find(|section| {
                MediaAttachment::from_content(&section.content)
                    .unwrap()
                    .is_some()
            })
            .unwrap()
            .clone()
    });
    let attachment = MediaAttachment::from_content(&section.content)
        .unwrap()
        .unwrap();
    assert_eq!(attachment.kind, MediaKind::Image);
    assert_eq!(
        std::fs::read(store.local_path(&attachment).unwrap()).unwrap(),
        svg
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    let image = visual
        .debug_bounds("media-image")
        .expect("native image preview is laid out");
    assert!((f32::from(image.size.width) / f32::from(image.size.height) - 2.).abs() < 0.05);
    cx.update_window(handle, |_, window, cx| {
        window
            .within(SharedString::from(section.id.clone()))
            .click("media-save", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    let saved = files.0.join("saved.svg");
    cx.simulate_new_path_selection(|_| Some(saved.clone()));
    finish(handle, cx);
    assert_eq!(std::fs::read(saved).unwrap(), svg);
}

fn drop_files(
    handle: AnyWindowHandle,
    element: SharedString,
    paths: Vec<PathBuf>,
    cx: &mut TestAppContext,
) {
    let position = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find(element).bounds().center()
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_event(FileDropEvent::Entered {
        position,
        paths: ExternalPaths(paths.into_iter().collect()),
    });
    visual.update(|window, cx| window.render_frame(cx));
    visual.simulate_event(FileDropEvent::Submit { position });
    visual.simulate_event(FileDropEvent::Ended);
    finish(handle, cx);
}

#[gpui_kit::test]
fn filesystem_drop_imports_ordered_media_into_a_list_as_one_collaborative_undo(
    cx: &mut TestAppContext,
) {
    let files = Files::new();
    let store = files.store();
    let video = files.file("clip.mp4", b"\0\0\0\x18ftypisomvideo");
    let audio = files.file("sound.wav", b"RIFF\0\0\0\0WAVEaudio");
    let pdf = files.file("report.pdf", b"%PDF-1.7\nattachment");
    let word = files.file("report.docx", b"PK\x03\x04document");
    let (handle, editor, item) = setup("- Keep\n\nAfter", cx);
    configure(&editor, store.clone(), cx);
    let before = editor.read_with(cx, |editor, _| editor.document.sections());
    let group = before
        .iter()
        .find(|section| matches!(section.kind, SectionKind::List { .. }))
        .unwrap()
        .id
        .clone();
    drop_files(
        handle,
        item.clone().into(),
        vec![video, audio, pdf, word],
        cx,
    );
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let children = editor.document.children(Some(&group));
        let kinds = children
            .iter()
            .filter_map(|section| {
                MediaAttachment::from_content(&section.content)
                    .unwrap()
                    .map(|attachment| {
                        assert!(matches!(section.kind, SectionKind::ListItem { .. }));
                        assert!(store.local_path(&attachment).unwrap().is_file());
                        attachment.kind
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                MediaKind::Video,
                MediaKind::Audio,
                MediaKind::File,
                MediaKind::File
            ]
        );
        assert_eq!(children.last().unwrap().id, item);
        let mut peer = crate::Document::new();
        peer.apply_update(&editor.document.encode_state()).unwrap();
        assert_eq!(peer.sections(), editor.document.sections());
    });
    let imported = editor.read_with(cx, |editor, _| editor.document.sections());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| editor.undo(window, cx).unwrap());
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.sections()),
        before
    );
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| editor.redo(window, cx).unwrap());
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.sections()),
        imported
    );
}

#[gpui_kit::test]
fn a_peer_edit_during_the_file_picker_preserves_text_and_undo_targets_only_the_import(
    cx: &mut TestAppContext,
) {
    let files = Files::new();
    let path = files.file("report.pdf", b"%PDF-1.7\nattachment");
    let (handle, editor, id) = setup("Before", cx);
    configure(&editor, files.store(), cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 6, 6, window, cx);
        window.input(" /file", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    let mut peer = replica(&editor, cx);
    let vector = peer.state_vector();
    set_text(&mut peer, &id, "Remote Before /file");
    let incoming = peer.encode_update(&vector).unwrap();
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(&incoming, window, cx).unwrap()
        });
    })
    .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![path]));
    finish(handle, cx);
    editor.read_with(cx, |editor, _| {
        assert!(editor.error.is_none(), "{:?}", editor.error);
        let sections = editor.document.sections();
        assert_eq!(sections[0].content.plain_text(), "Remote Before ");
        assert_eq!(
            MediaAttachment::from_content(&sections[1].content)
                .unwrap()
                .unwrap()
                .kind,
            MediaKind::File
        );
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| editor.undo(window, cx).unwrap());
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(
            editor.document.sections()[0].content.plain_text(),
            "Remote Before /file"
        );
        assert!(editor.document.sections().iter().all(|section| {
            MediaAttachment::from_content(&section.content)
                .unwrap()
                .is_none()
        }));
    });
}

#[gpui_kit::test]
fn failed_drop_and_deleted_picker_target_do_not_mutate_the_document(cx: &mut TestAppContext) {
    let files = Files::new();
    let (handle, editor, id) = setup("Keep", cx);
    configure(&editor, files.store(), cx);
    let before = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
    drop_files(handle, id.clone().into(), vec![files.0.clone()], cx);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.export_json().unwrap()),
        before
    );
    assert!(editor.read_with(cx, |editor, _| editor.error.is_some()));
    let path = files.file("file.pdf", b"%PDF-1.7\nattachment");
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.execute_command(
                &id,
                CommandAction::InsertMedia(MediaKind::File),
                None,
                window,
                cx,
            )
        });
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.remove_section(&id, window, cx).unwrap()
        });
    })
    .unwrap();
    let removed = editor.read_with(cx, |editor, _| editor.document.export_json().unwrap());
    cx.simulate_path_prompt_response(|_| Some(vec![path]));
    finish(handle, cx);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.export_json().unwrap()),
        removed
    );
}
