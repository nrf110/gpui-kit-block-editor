use super::*;
use crate::native_editor::BlockEditorModel;
use crate::native_editor::rich::{RichAtom, RichMarks, RichRun};
use gpui_kit::base::text::MarkdownExtensions;
use yrs::types::Attrs;

fn custom_atom() -> RichAtom {
    RichAtom::Custom {
        name: "atom-storage-test".into(),
        text: "same atom".into(),
        markdown: "<same-atom>".into(),
        accessibility_label: "same atom".into(),
        payload: serde_json::json!({"codec": "json", "data": {"value": [1, "α"]}}),
    }
}

fn assert_atom_content(document: &Document, content: &RichDocument, markdown: &str) {
    let sections = document.sections();
    assert_eq!(sections.len(), 1);
    let actual = &sections[0].content;
    assert_eq!(actual, content);
    actual.validate().unwrap();
    assert_eq!(document.markdown(), markdown);
    let mounted = BlockEditorModel::from_rich(actual, MarkdownExtensions::default()).unwrap();
    assert_eq!(mounted.rich_document().unwrap(), *content);
}

fn repeated_atom_boundaries(atom: RichAtom, markdown_token: &str) {
    let mut content = RichDocument::paragraph("before ");
    let run = RichRun {
        text: "\u{FFFC}".into(),
        marks: RichMarks {
            bold: true,
            italic: true,
            ..RichMarks::default()
        },
        atom: Some(atom),
    };
    content.blocks[0].runs.extend([
        run.clone(),
        run,
        RichRun {
            text: " after".into(),
            ..RichRun::default()
        },
    ]);
    let markdown = content.to_markdown().unwrap();
    assert_eq!(markdown.matches(markdown_token).count(), 2);

    let mut document = Document::new();
    let id = document.insert_section(None, "placeholder").unwrap();
    document.set_content(&id, &content).unwrap();
    // Exercise an actual coalesced Y.Text span, not merely equal-looking runs.
    {
        let txn = document.doc.transact();
        let section = document.section_map(&txn, &id).unwrap();
        let text = rich_storage::node_text(&section, &txn, &content.blocks[0].id).unwrap();
        let atoms: Vec<_> = text
            .diff(&txn, |_| ())
            .into_iter()
            .filter(|part| {
                part.attributes
                    .as_ref()
                    .is_some_and(|attrs| attrs.contains_key("atom"))
            })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert!(matches!(
            &atoms[0].insert,
            Out::Any(Any::String(value)) if value.as_ref() == "\u{FFFC}\u{FFFC}"
        ));
    }
    assert_atom_content(&document, &content, &markdown);

    let json = document.export_json().unwrap();
    let restored = Document::import_json(&json).unwrap();
    assert_eq!(restored.sections(), document.sections());
    assert_atom_content(&restored, &content, &markdown);
    assert!(!restored.can_undo());

    let mut raw = Document::new();
    let update = document.encode_update(&raw.state_vector()).unwrap();
    raw.apply_update(&update).unwrap();
    assert_atom_content(&raw, &content, &markdown);
    assert!(!raw.can_undo());
    assert!(!raw.can_redo());

    let mut framed = Document::new();
    let frame = Message::Sync(SyncMessage::Update(document.encode_state())).encode_v1();
    assert!(framed.receive_sync(&frame).unwrap().is_empty());
    assert_atom_content(&framed, &content, &markdown);
    assert!(!framed.can_undo());
    assert!(!framed.can_redo());
}

#[test]
fn adjacent_identical_images_preserve_atom_boundaries_across_storage_and_sync() {
    repeated_atom_boundaries(
        RichAtom::Image {
            // Native model mounting never needs an external image fetch.
            url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5X0AAAAASUVORK5CYII=".into(),
            link: None,
            title: Some("same title".into()),
            alt: Some("same image".into()),
            width: None,
            height: None,
        },
        "![same image]",
    );
}

#[test]
fn adjacent_identical_custom_atoms_preserve_boundaries_across_storage_and_sync() {
    repeated_atom_boundaries(custom_atom(), "<same-atom>");
}

#[test]
fn atom_metadata_on_non_placeholder_text_rejects_raw_and_framed_updates_atomically() {
    let mut document = Document::new();
    let id = document.insert_section(None, "keep").unwrap();
    let node = document.sections()[0].content.blocks[0].id.clone();
    document.clear_history();
    let before = document.encode_state();
    let vector = document.state_vector();
    for invalid in ["plain text", "\u{FFFC}x", "x\u{FFFC}", "\u{FFFC}x\u{FFFC}"] {
        let mut peer = Document::new();
        peer.apply_update(&before).unwrap();
        {
            let mut txn = peer.doc.transact_mut();
            let section = peer.section_map(&txn, &id).unwrap();
            let body = rich_storage::body(&section, &txn).unwrap();
            let text = rich_storage::node_text(&section, &txn, &node).unwrap();
            let length = text.len(&txn);
            text.remove_range(&mut txn, 0, length);
            let attrs = Attrs::from([(
                "atom".into(),
                Any::String(serde_json::to_string(&custom_atom()).unwrap().into()),
            )]);
            text.insert_with_attributes(&mut txn, 0, invalid, attrs);
            assert!(
                rich_storage::read_content(&body, &txn).is_err(),
                "{invalid:?}"
            );
        }
        let update = peer.encode_update(&vector).unwrap();
        assert!(document.apply_update(&update).is_err(), "{invalid:?}");
        assert_eq!(document.encode_state(), before);
        let frame = Message::Sync(SyncMessage::Update(update)).encode_v1();
        assert!(document.reply_sync_without_applying(&frame).is_err());
        assert!(document.receive_sync(&frame).is_err(), "{invalid:?}");
        assert_eq!(document.encode_state(), before);
        assert_eq!(document.markdown(), "keep");
        assert!(!document.can_undo());
        assert!(!document.can_redo());
    }
}
