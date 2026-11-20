use super::*;
use gpui_kit::base::text::MarkdownExtensions;
use markdown_editor::{
    BlockEditorModel,
    rich::{RichAtom, RichMarks, RichRun},
};
use yrs::types::Attrs;

fn content(document: &Document, id: &str) -> RichDocument {
    document
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .unwrap()
        .content
}

fn replica(document: &Document) -> Document {
    let mut peer = Document::new();
    peer.apply_update(&document.encode_state()).unwrap();
    peer
}

fn convert_to_code(document: &mut Document, id: &str) {
    let mut body = content(document, id);
    body.blocks[0].kind = RichKind::CodeBlock;
    body.blocks[0].attrs.insert("lang".into(), "rust".into());
    for run in &mut body.blocks[0].runs {
        run.marks = RichMarks::default();
        run.atom = None;
    }
    document.set_content(id, &body).unwrap();
}

fn concurrent_code_and_formatting(framed: bool) {
    let mut code = Document::new();
    let id = code.insert_section(None, "hello😀").unwrap();
    code.clear_history();
    let node = content(&code, &id).blocks[0].id.clone();
    let cursor = code.relative_position(&id, &node, 5, Assoc::After).unwrap();
    let mut formatting = replica(&code);
    convert_to_code(&mut code, &id);
    let mut bold = content(&formatting, &id);
    bold.blocks[0].runs = vec![
        RichRun {
            text: "hello".into(),
            marks: RichMarks {
                bold: true,
                ..Default::default()
            },
            atom: None,
        },
        RichRun {
            text: "😀".into(),
            ..Default::default()
        },
    ];
    formatting.set_content(&id, &bold).unwrap();
    let to_code = formatting.encode_update(&code.state_vector()).unwrap();
    let to_formatting = code.encode_update(&formatting.state_vector()).unwrap();
    if framed {
        let incoming = Message::Sync(SyncMessage::Update(to_code)).encode_v1();
        let before = code.encode_state();
        code.reply_sync_without_applying(&incoming).unwrap();
        assert_eq!(code.encode_state(), before);
        code.receive_sync(&incoming).unwrap();
        formatting
            .receive_sync(&Message::Sync(SyncMessage::Update(to_formatting)).encode_v1())
            .unwrap();
    } else {
        code.apply_update(&to_code).unwrap();
        formatting.apply_update(&to_formatting).unwrap();
    }
    assert_eq!(code.sections(), formatting.sections());
    let projected = content(&code, &id);
    assert_eq!(projected.blocks[0].id, node);
    assert_eq!(projected.blocks[0].kind, RichKind::CodeBlock);
    assert_eq!(projected.plain_text(), "hello😀");
    assert!(
        projected.blocks[0]
            .runs
            .iter()
            .all(|run| run.marks == RichMarks::default() && run.atom.is_none())
    );
    let native = BlockEditorModel::from_rich(&projected, MarkdownExtensions::default()).unwrap();
    assert_eq!(native.rich_document().unwrap(), projected);
    assert_eq!(code.resolve_position(&cursor), Some((id.clone(), node, 5)));
    let before = code.encode_state();
    code.set_content(&id, &projected).unwrap();
    assert_eq!(
        code.encode_state(),
        before,
        "reading back projected code must not erase peer attributes"
    );
    assert!(code.markdown().contains("hello😀"));
    assert!(code.undo());
    let restored = content(&code, &id);
    assert_eq!(restored.blocks[0].kind, RichKind::Paragraph);
    assert_eq!(restored.plain_text(), "hello😀");
    assert!(
        restored.blocks[0].runs[0].marks.bold,
        "undo must reveal the peer's formatting"
    );
    formatting
        .apply_update(&code.encode_update(&formatting.state_vector()).unwrap())
        .unwrap();
    assert_eq!(code.sections(), formatting.sections());
}

#[test]
fn concurrent_code_conversion_and_formatting_merge_via_raw_updates() {
    concurrent_code_and_formatting(false);
}

#[test]
fn concurrent_code_conversion_and_formatting_merge_via_framed_sync() {
    concurrent_code_and_formatting(true);
}

#[test]
fn concurrent_inline_atom_survives_code_conversion_and_undo() {
    let mut code = Document::new();
    let id = code.insert_section(None, "text").unwrap();
    code.clear_history();
    let mut images = replica(&code);
    convert_to_code(&mut code, &id);
    let mut image_body = content(&images, &id);
    image_body.blocks[0].runs.insert(
        0,
        RichRun {
            text: "\u{FFFC}".into(),
            atom: Some(RichAtom::Image {
                url: "image.png".into(),
                link: None,
                title: None,
                alt: Some("picture".into()),
                width: None,
                height: None,
            }),
            ..Default::default()
        },
    );
    images.set_content(&id, &image_body).unwrap();
    code.apply_update(&images.encode_update(&code.state_vector()).unwrap())
        .unwrap();
    let projected = content(&code, &id);
    assert_eq!(projected.plain_text(), "\u{FFFC}text");
    projected.validate().unwrap();
    assert_eq!(
        BlockEditorModel::from_rich(&projected, MarkdownExtensions::default())
            .unwrap()
            .rich_document()
            .unwrap(),
        projected
    );
    assert!(code.undo());
    assert_eq!(content(&code, &id), image_body);
}

#[test]
fn code_projection_still_rejects_malformed_marks_atomically() {
    let mut code = Document::new();
    let id = code.insert_section(None, "text").unwrap();
    convert_to_code(&mut code, &id);
    let mut malformed = replica(&code);
    let node = content(&code, &id).blocks[0].id.clone();
    let text = {
        let txn = malformed.doc.transact();
        rich_storage::node_text(&malformed.section_map(&txn, &id).unwrap(), &txn, &node).unwrap()
    };
    let attrs: Attrs = [("highlight".into(), Any::String("[-1,0,0,1]".into()))]
        .into_iter()
        .collect();
    text.format(
        &mut malformed.doc.transact_mut_with(LOCAL_ORIGIN),
        0,
        4,
        attrs,
    );
    let incoming = malformed.encode_state();
    let before = code.encode_state();
    assert!(code.apply_update(&incoming).is_err());
    assert_eq!(code.encode_state(), before);
    assert!(
        code.receive_sync(&Message::Sync(SyncMessage::Update(incoming)).encode_v1())
            .is_err()
    );
    assert_eq!(code.encode_state(), before);
    let mut invalid_local = content(&code, &id);
    invalid_local.blocks[0].runs[0].marks.bold = true;
    assert!(code.set_content(&id, &invalid_local).is_err());
    assert_eq!(code.encode_state(), before);
}
