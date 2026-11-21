use crate::media::{
    FileMediaStore, MediaAttachment, MediaKind, MediaStore, validate_media_atom,
    validate_media_content,
};
use crate::native_editor::BlockEditorModel;
use crate::{BlockSpec, Document, RichAtom, RichDocument, RichKind, RichNode, SectionKind};
use gpui_kit::base::text::MarkdownExtensions;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("gpui-kit-media-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn store(&self) -> FileMediaStore {
        FileMediaStore::new(self.0.join("assets"))
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn attachment() -> MediaAttachment {
    MediaAttachment {
        asset_id: "a6e95781-3fde-4e13-aa1f-5650cd7bb593".into(),
        kind: MediaKind::Image,
        file_name: "雪 [image].png".into(),
        mime_type: "image/png".into(),
        byte_len: 15,
    }
}

fn payload(content: &mut RichDocument) -> &mut Value {
    let Some(RichAtom::Custom { payload, .. }) = &mut content.blocks[0].runs[0].atom else {
        panic!("expected media atom")
    };
    payload
}

fn assert_invalid(content: &RichDocument) {
    assert!(validate_media_content(content).is_err());
    assert!(MediaAttachment::from_content(content).is_err());
}

#[test]
fn media_imports_classify_all_kinds_and_signature_overrides_extension() {
    let directory = TestDirectory::new();
    let store = directory.store();
    let examples: &[(&str, &[u8], MediaKind, &str)] = &[
        (
            "雪.png",
            b"\x89PNG\r\n\x1a\ncontent",
            MediaKind::Image,
            "image/png",
        ),
        (
            "actually-image.txt",
            b"GIF89aimage",
            MediaKind::Image,
            "image/gif",
        ),
        (
            "clip.mp4",
            b"\0\0\0\x18ftypisomvideo",
            MediaKind::Video,
            "video/mp4",
        ),
        (
            "voice.wav",
            b"RIFF\0\0\0\0WAVEaudio",
            MediaKind::Audio,
            "audio/wav",
        ),
        (
            "notes.pdf",
            b"%PDF-1.7\ncontent",
            MediaKind::File,
            "application/pdf",
        ),
        (
            "unknown.bin",
            b"\0\x01\x02unknown",
            MediaKind::File,
            "application/octet-stream",
        ),
        (
            "not-an-image.png",
            b"ordinary bytes",
            MediaKind::File,
            "application/octet-stream",
        ),
        ("notes.txt", b"plain text", MediaKind::File, "text/plain"),
    ];
    for (name, bytes, kind, mime) in examples {
        let source = directory.file(name, bytes);
        let media = store.import_file(&source, None).unwrap();
        assert_eq!(media.kind, *kind);
        assert_eq!(media.mime_type, *mime);
        assert_eq!(media.file_name, *name);
        assert_eq!(media.byte_len, bytes.len() as u64);
        assert_eq!(
            Uuid::parse_str(&media.asset_id).unwrap().to_string(),
            media.asset_id
        );
        let managed = store.local_path(&media).unwrap();
        assert_eq!(managed.extension(), source.extension());
        assert_eq!(fs::read(managed).unwrap(), *bytes);
    }
}

#[test]
fn requested_media_kind_rejects_mismatch_but_file_accepts_any_file() {
    let directory = TestDirectory::new();
    let store = directory.store();
    let source = directory.file("photo.png", b"\x89PNG\r\n\x1a\nimage");
    assert_eq!(
        store
            .import_file(&source, Some(MediaKind::Image))
            .unwrap()
            .kind,
        MediaKind::Image
    );
    assert_eq!(
        store
            .import_file(&source, Some(MediaKind::File))
            .unwrap()
            .kind,
        MediaKind::File
    );
    assert!(store.import_file(&source, Some(MediaKind::Audio)).is_err());
    assert!(store.import_file(&source, Some(MediaKind::Video)).is_err());
    let unknown = directory.file("unknown.bin", b"unknown");
    assert!(store.import_file(&unknown, Some(MediaKind::Image)).is_err());
    assert!(store.import_file(&directory.0, None).is_err());
}

#[test]
fn assets_persist_after_source_deletion_and_export_overwrites_byte_exactly() {
    let directory = TestDirectory::new();
    let bytes: Vec<u8> = (0..100_000).map(|index| (index % 251) as u8).collect();
    let source = directory.file("large.data", &bytes);
    let media = directory.store().import_file(&source, None).unwrap();
    fs::remove_file(&source).unwrap();
    let restarted = directory.store();
    let managed = restarted.local_path(&media).unwrap();
    assert_eq!(fs::read(&managed).unwrap(), bytes);
    let destination = directory.file("saved.data", b"replace me");
    restarted.export_file(&media, &destination).unwrap();
    assert_eq!(fs::read(destination).unwrap(), bytes);
    assert!(restarted.export_file(&media, &managed).is_err());
    assert_eq!(fs::read(&managed).unwrap(), bytes);
    assert_eq!(fs::read_dir(directory.0.join("assets")).unwrap().count(), 1);
    let mut wrong_size = media.clone();
    wrong_size.byte_len += 1;
    assert!(restarted.local_path(&wrong_size).is_err());
}

#[test]
fn peer_install_retains_identity_and_never_overwrites_existing_bytes() {
    let owner = TestDirectory::new();
    let peer = TestDirectory::new();
    let bytes = b"RIFF\0\0\0\0WAVEpeer bytes";
    let source = owner.file("recording.wav", bytes);
    let original = owner.store().import_file(&source, None).unwrap();
    let peer_store = peer.store();
    assert!(peer_store.local_path(&original).is_err());
    peer_store.install_file(&original, &source).unwrap();
    peer_store.install_file(&original, &source).unwrap();
    assert_eq!(
        fs::read(peer_store.local_path(&original).unwrap()).unwrap(),
        bytes
    );
    let duplicate = original.to_content().unwrap();
    assert_eq!(
        MediaAttachment::from_content(&duplicate).unwrap(),
        Some(original.clone())
    );
    let corrupt = peer.file("wrong.wav", b"RIFF\0\0\0\0WAVEdifferent!");
    assert!(peer_store.install_file(&original, &corrupt).is_err());
    let shorter = peer.file("short.wav", b"RIFF\0\0\0\0WAVE");
    assert!(peer_store.install_file(&original, &shorter).is_err());
    assert_eq!(
        fs::read(peer_store.local_path(&original).unwrap()).unwrap(),
        bytes
    );
}

#[test]
fn unsafe_asset_identifiers_and_filenames_are_rejected() {
    let directory = TestDirectory::new();
    let store = directory.store();
    for id in [
        "../outside",
        "/absolute",
        "..\\outside",
        "not-a-uuid",
        "00000000-0000-0000-0000-000000000000",
    ] {
        let mut media = attachment();
        media.asset_id = id.into();
        assert!(media.to_content().is_err());
        assert!(store.local_path(&media).is_err());
        assert!(store.install_file(&media, Path::new("missing")).is_err());
    }
    for name in [
        "../secret.png",
        "..\\secret.png",
        "/secret.png",
        "bad\n.png",
        "CON.png",
        "lpt1.txt",
        "trailing.",
        "",
        "..",
    ] {
        let mut media = attachment();
        media.file_name = name.into();
        assert!(media.to_content().is_err(), "{name:?}");
    }
    // Four UTF-8 bytes must not be sliced at an invalid character boundary.
    let mut unicode = attachment();
    unicode.file_name = "xxé.png".into();
    assert!(unicode.to_content().is_ok());
}

#[cfg(unix)]
#[test]
fn symlinks_are_rejected_and_source_executable_bits_are_not_copied() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = TestDirectory::new();
    let store = directory.store();
    let bytes = b"plain file bytes";
    let source = directory.file("original.txt", bytes);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let media = store.import_file(&source, None).unwrap();
    let managed = store.local_path(&media).unwrap();
    assert_eq!(
        fs::metadata(&managed).unwrap().permissions().mode() & 0o111,
        0
    );
    let destination = directory.0.join("export.txt");
    store.export_file(&media, &destination).unwrap();
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o111,
        0
    );
    let linked_source = directory.0.join("source-link.txt");
    symlink(&source, &linked_source).unwrap();
    assert!(store.import_file(&linked_source, None).is_err());
    let linked_export = directory.0.join("export-link.txt");
    symlink(&source, &linked_export).unwrap();
    assert!(store.export_file(&media, &linked_export).is_err());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    fs::remove_file(&managed).unwrap();
    symlink(&source, &managed).unwrap();
    assert!(store.local_path(&media).is_err());
    assert!(store.install_file(&media, &source).is_err());
    let linked_root = directory.0.join("linked-root");
    symlink(directory.0.join("assets"), &linked_root).unwrap();
    assert!(
        FileMediaStore::new(linked_root)
            .import_file(&source, None)
            .is_err()
    );
}

#[test]
fn media_custom_block_survives_native_model_and_exports_a_portable_asset_link() {
    let media = attachment();
    let content = media.to_content().unwrap();
    content.validate().unwrap();
    validate_media_content(&content).unwrap();
    assert_eq!(content.blocks[0].kind, RichKind::Custom);
    assert_eq!(content.blocks[0].runs[0].text, "\u{FFFC}");
    assert_eq!(
        MediaAttachment::from_content(&content).unwrap(),
        Some(media.clone())
    );
    let model = BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    assert_eq!(model.rich_document().unwrap(), content);
    let markdown = content.to_markdown().unwrap();
    assert!(markdown.contains(&format!("asset://{}", media.asset_id)));
    assert!(markdown.contains("雪 \\[image\\].png"));
    let encoded = serde_json::to_string(&content).unwrap();
    assert!(!encoded.contains("local_path"));
    assert!(!encoded.contains("file_bytes"));
    assert_eq!(
        serde_json::from_str::<RichDocument>(&encoded).unwrap(),
        content
    );
}

#[test]
fn reserved_payloads_reject_invalid_metadata_versions_and_presentation() {
    let original = attachment().to_content().unwrap();
    for (key, value) in [("version", json!(2)), ("attachment", json!(null))] {
        let mut invalid = original.clone();
        payload(&mut invalid)["data"][key] = value;
        assert_invalid(&invalid);
        assert!(validate_media_atom(invalid.blocks[0].runs[0].atom.as_ref().unwrap()).is_err());
    }
    for (key, value) in [
        ("asset_id", json!("../outside")),
        ("file_name", json!("../outside.png")),
        ("kind", json!("document")),
        ("mime_type", json!("video/mp4")),
        ("mime_type", json!("image/png\n")),
        ("byte_len", json!(-1)),
        ("local_path", json!("/private/source.png")),
    ] {
        let mut invalid = original.clone();
        payload(&mut invalid)["data"]["attachment"][key] = value;
        assert_invalid(&invalid);
    }
    let mut invalid = original.clone();
    payload(&mut invalid)["codec"] = json!("string");
    assert_invalid(&invalid);
    let mut invalid = original.clone();
    payload(&mut invalid)["unexpected"] = json!(true);
    assert_invalid(&invalid);
    let mut invalid = original.clone();
    if let Some(RichAtom::Custom { markdown, .. }) = &mut invalid.blocks[0].runs[0].atom {
        *markdown = "[unsafe](file:///private/source.png)".into();
    }
    assert_invalid(&invalid);
    let mut invalid = original.clone();
    invalid.blocks[0].runs[0].marks.bold = true;
    assert_invalid(&invalid);
    let mut invalid = original;
    invalid.blocks[0].kind = RichKind::Paragraph;
    assert_invalid(&invalid);
}

#[test]
fn recursive_validation_preserves_other_extensions_and_non_atomic_documents() {
    let media = attachment().to_content().unwrap();
    let nested = RichDocument {
        blocks: vec![RichNode {
            id: Uuid::new_v4().to_string(),
            kind: RichKind::Quote,
            attrs: Default::default(),
            runs: vec![],
            children: media.blocks.clone(),
        }],
        ..RichDocument::default()
    };
    nested.validate().unwrap();
    validate_media_content(&nested).unwrap();
    assert_eq!(MediaAttachment::from_content(&nested).unwrap(), None);
    let mut invalid = nested;
    if let Some(RichAtom::Custom { payload, .. }) = &mut invalid.blocks[0].children[0].runs[0].atom
    {
        payload["data"]["version"] = json!(99);
    }
    assert_invalid(&invalid);
    let mut unknown = media;
    if let Some(RichAtom::Custom { name, payload, .. }) = &mut unknown.blocks[0].runs[0].atom {
        *name = "another-extension".into();
        *payload = json!({"codec": "json", "data": {"arbitrary": [1, true, "α"]}});
    }
    validate_media_content(&unknown).unwrap();
    assert_eq!(MediaAttachment::from_content(&unknown).unwrap(), None);
    let mut multiple = attachment().to_content().unwrap();
    multiple.blocks.push(RichNode::paragraph("caption"));
    assert_eq!(MediaAttachment::from_content(&multiple).unwrap(), None);
}

#[test]
fn json_crdt_sync_and_undo_keep_original_media_and_node_identities() {
    let media = attachment();
    let content = media.to_content().unwrap();
    let node_id = content.blocks[0].id.clone();
    let mut original = Document::new();
    let section = original
        .insert_block(
            None,
            None,
            &BlockSpec {
                kind: SectionKind::Text,
                content: content.clone(),
                children: vec![],
            },
        )
        .unwrap();
    let json = original.export_json().unwrap();
    let restored = Document::import_json(&json).unwrap();
    assert_eq!(restored.sections(), original.sections());
    let mut peer = Document::new();
    peer.apply_update(&original.encode_state()).unwrap();
    assert_eq!(peer.sections(), original.sections());
    assert!(!peer.can_undo());
    assert_eq!(peer.sections()[0].content.blocks[0].id, node_id);
    assert_eq!(
        MediaAttachment::from_content(&peer.sections()[0].content).unwrap(),
        Some(media.clone())
    );
    original.clear_history();
    let mut updated = media.clone();
    updated.file_name = "renamed.png".into();
    let mut updated_content = updated.to_content().unwrap();
    updated_content.blocks[0].id = node_id.clone();
    original.set_content(&section, &updated_content).unwrap();
    peer.apply_update(&original.encode_update(&peer.state_vector()).unwrap())
        .unwrap();
    assert_eq!(peer.sections(), original.sections());
    assert_eq!(peer.sections()[0].content.blocks[0].id, node_id);
    assert!(original.undo());
    assert_eq!(original.sections()[0].content, content);
    peer.apply_update(&original.encode_update(&peer.state_vector()).unwrap())
        .unwrap();
    assert_eq!(peer.sections(), original.sections());
    assert!(original.redo());
    assert_eq!(original.sections()[0].content, updated_content);
    assert_eq!(original.sections()[0].content.blocks[0].id, node_id);
}

#[test]
fn document_boundaries_reject_invalid_reserved_media_without_mutation() {
    let mut document = Document::new();
    let section = document.insert_section(None, "keep").unwrap();
    document.clear_history();
    let before = document.export_json().unwrap();
    let mut invalid = attachment().to_content().unwrap();
    payload(&mut invalid)["data"]["version"] = json!(55);
    assert!(document.set_content(&section, &invalid).is_err());
    assert!(
        document
            .insert_block(
                None,
                None,
                &BlockSpec {
                    kind: SectionKind::Text,
                    content: invalid.clone(),
                    children: vec![],
                }
            )
            .is_err()
    );
    let mut json: Value = serde_json::from_str(&before).unwrap();
    json["sections"][0]["content"] = serde_json::to_value(invalid).unwrap();
    assert!(Document::import_json(&json.to_string()).is_err());
    assert_eq!(document.export_json().unwrap(), before);
    assert!(!document.can_undo());
}
