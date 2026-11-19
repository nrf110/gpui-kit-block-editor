use super::*;
use crate::document::{AtomData, BlockContent};
use crate::rich::*;
use gpui_kit::base::text::MarkdownNode;
use serde_json::json;

fn restore(document: &RichDocument) -> BlockEditorModel {
    BlockEditorModel::from_rich(document, MarkdownExtensions::default()).unwrap()
}

#[test]
fn direct_restore_does_not_interpret_markdown_or_unicode() {
    let document = RichDocument::paragraph("# literal **bold?** [x](url)\n👩🏽‍💻 e\u{301} 世界");
    let model = restore(&document);
    assert_eq!(model.rich_document().unwrap(), document);
    assert_eq!(
        model.document().leaves().next().unwrap().text(),
        Some(document.blocks[0].text().as_str())
    );
    assert_eq!(document.plain_text(), document.blocks[0].text());
}

#[test]
fn semantic_native_tree_round_trips_without_markdown_intermediate() {
    let source = "# Heading\n\nText **bold** *italic* [link](https://example.com \"title\")\n\n> quote\n\n3. [x] task\n   - child\n\n| left | right |\n| :--- | ---: |\n| α | β |\n\n```rust meta\nlet x = \"**not marks**\";\n```\n\n---";
    let native = model(source);
    let document = native.rich_document().unwrap();
    document.validate().unwrap();
    let restored = restore(&document);
    assert_eq!(restored.rich_document().unwrap(), document);
    assert_eq!(restored.source(), native.source());
    let kinds: Vec<_> = document.blocks.iter().map(|node| node.kind).collect();
    assert!(kinds.contains(&RichKind::Heading));
    assert!(kinds.contains(&RichKind::List));
    assert!(kinds.contains(&RichKind::Table));
    assert!(kinds.contains(&RichKind::CodeBlock));
    assert_invariants(&restored);
}

#[test]
fn image_dimensions_links_colors_and_extra_metadata_are_lossless() {
    let mut document = RichDocument::paragraph("α");
    let node = &mut document.blocks[0];
    node.attrs
        .insert("plugin-metadata".into(), json!({"nested":[1,true,"世界"]}));
    node.runs[0].marks = RichMarks {
        bold: true,
        italic: true,
        strikethrough: true,
        underline: true,
        code: true,
        highlight: Some([0.2, 0.3, 0.4, 0.5]),
        link: Some(RichLink {
            url: "target".into(),
            identifier: Some("id".into()),
            title: Some("title".into()),
        }),
    };
    for (width, height) in [
        (RichLength::Pixels(321.5), RichLength::Rems(3.25)),
        (RichLength::Relative(0.75), RichLength::Pixels(42.0)),
    ] {
        node.runs.push(RichRun {
            text: "\u{FFFC}".into(),
            marks: RichMarks::default(),
            atom: Some(RichAtom::Image {
                url: "https://example.com/a.png".into(),
                link: Some(RichLink {
                    url: "image-link".into(),
                    identifier: None,
                    title: Some("caption".into()),
                }),
                title: Some("image title".into()),
                alt: Some("alt α".into()),
                width: Some(width),
                height: Some(height),
            }),
        });
    }
    let restored = restore(&document);
    assert_eq!(restored.rich_document().unwrap(), document);
    let encoded = serde_json::to_string(&document).unwrap();
    assert_eq!(
        serde_json::from_str::<RichDocument>(&encoded).unwrap(),
        document
    );
}

#[test]
fn stable_ids_survive_marks_headings_split_moves_and_history() {
    let mut native = model("abcdef\n\nsecond");
    let original = native.rich_document().unwrap();
    let first = original.blocks[0].id.clone();
    let native_first = native.block_id(&first).unwrap();
    select(&mut native, 0, 1..4);
    native
        .run(EditorCommand::ToggleMark(MarkKind::Bold))
        .unwrap();
    assert_eq!(native.rich_document().unwrap().blocks[0].id, first);
    native
        .run(EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }))
        .unwrap();
    assert_eq!(native.stable_id(native_first), Some(first.clone()));
    caret_in(&mut native, 0, 3);
    native.split_block().unwrap();
    let split = native.rich_document().unwrap();
    assert_eq!(split.blocks[0].id, first);
    assert_ne!(split.blocks[1].id, first);
    let split_id = split.blocks[1].id.clone();
    assert!(native.undo());
    assert!(native.block_id(&split_id).is_none());
    assert!(native.redo());
    assert_eq!(native.rich_document().unwrap().blocks[1].id, split_id);
    native.move_block_to(native_first, 3).unwrap();
    assert_eq!(native.rich_document().unwrap().blocks[2].id, first);
    assert!(native.undo());
    assert_eq!(native.rich_document().unwrap().blocks[0].id, first);
    assert!(native.redo());
    assert_eq!(native.rich_document().unwrap().blocks[2].id, first);
    assert_invariants(&native);
}

#[test]
fn code_conversion_keeps_stable_identity() {
    let mut native = model("literal");
    let before = native.rich_document().unwrap();
    native
        .run(EditorCommand::SetCodeBlock {
            lang: Some("rust".into()),
        })
        .unwrap();
    assert_eq!(
        native.rich_document().unwrap().blocks[0].id,
        before.blocks[0].id
    );
    assert!(native.undo());
    assert_eq!(native.rich_document().unwrap(), before);
    assert!(native.redo());
    assert_eq!(
        native.rich_document().unwrap().blocks[0].id,
        before.blocks[0].id
    );
}

#[test]
fn same_id_restore_retains_native_identity_and_clears_history() {
    let document = RichDocument::paragraph("before");
    let mut native = restore(&document);
    let block = native.block_id(&document.blocks[0].id).unwrap();
    native.insert_text("typed", EditOrigin::Typing).unwrap();
    assert!(native.can_undo());
    let revision = native.revision();
    let mut changed = document.clone();
    changed.blocks[0].runs[0].text = "after **literal**".into();
    native.set_document(&changed).unwrap();
    assert_eq!(native.revision(), revision + 1);
    assert!(!native.can_undo());
    assert!(!native.can_redo());
    assert_eq!(native.block_id(&changed.blocks[0].id), Some(block));
    assert_eq!(native.rich_document().unwrap(), changed);
    assert_ne!(
        restore(&changed).block_id(&changed.blocks[0].id),
        Some(block)
    );
}

#[test]
fn invalid_restore_is_atomic_and_does_not_invalidate_history() {
    let mut native = restore(&RichDocument::paragraph("hello"));
    native.insert_text("x", EditOrigin::Typing).unwrap();
    let before = native.rich_document().unwrap();
    let revision = native.revision();
    let mut invalid = before.clone();
    invalid.blocks.push(invalid.blocks[0].clone());
    assert!(native.set_document(&invalid).is_err());
    assert_eq!(native.rich_document().unwrap(), before);
    assert_eq!(native.revision(), revision);
    assert!(native.can_undo());
    invalid = before.clone();
    invalid.version = 999;
    assert!(invalid.validate().is_err());
    invalid = before.clone();
    invalid.blocks[0].kind = RichKind::Heading;
    invalid.blocks[0].attrs.insert("level".into(), json!(7));
    assert!(invalid.validate().is_err());
    invalid = before.clone();
    invalid.blocks[0].runs[0].atom = Some(RichAtom::Image {
        url: "a".into(),
        link: None,
        title: None,
        alt: None,
        width: None,
        height: None,
    });
    assert!(invalid.validate().is_err());
}

#[test]
fn empty_section_container_metadata_validates_but_is_not_mounted() {
    for kind in [RichKind::List, RichKind::Quote] {
        let mut document = RichDocument::paragraph("");
        document.blocks[0].kind = kind;
        document.validate().unwrap();
        assert!(BlockEditorModel::from_rich(&document, MarkdownExtensions::default()).is_err());
    }
}

#[test]
fn source_atoms_and_builtin_payload_codecs_round_trip() {
    let native = model(
        "<details>\n<summary>Title</summary>\nbody\n</details>\n\ntext[^note]\n\n[^note]: definition",
    );
    let document = native.rich_document().unwrap();
    assert_eq!(restore(&document).rich_document().unwrap(), document);
    assert_eq!(restore(&document).source(), native.source());
    for atom in [
        MarkdownNode::new("unit", ()),
        MarkdownNode::new("json", json!({"a":[1,2]})),
        MarkdownNode::new("string", String::from("payload")),
    ] {
        let mut native = model("placeholder");
        let id = native.doc.leaves()[0];
        let entry = native.doc.get_mut(id).unwrap();
        entry.kind = crate::kind::BlockKindData::Custom;
        entry.content = BlockContent::Atom(AtomData::Custom(
            atom.text("text")
                .markdown("<custom>")
                .accessibility_label("accessible"),
        ));
        let snapshot = native.rich_document().unwrap();
        assert_eq!(restore(&snapshot).rich_document().unwrap(), snapshot);
    }
}

#[test]
fn custom_registered_payload_is_restored_and_unknown_any_errors() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Payload {
        count: u64,
        label: String,
    }
    register_custom_codec::<Payload>("rich-test-payload").unwrap();
    let mut native = model("placeholder");
    let id = native.doc.leaves()[0];
    let entry = native.doc.get_mut(id).unwrap();
    entry.kind = crate::kind::BlockKindData::Custom;
    entry.content = BlockContent::Atom(AtomData::Custom(
        MarkdownNode::new(
            "rich-test-payload",
            Payload {
                count: 42,
                label: "α".into(),
            },
        )
        .text("text")
        .markdown("source"),
    ));
    let snapshot = native.rich_document().unwrap();
    let restored = restore(&snapshot);
    let id = restored.doc.leaves()[0];
    let BlockContent::Atom(AtomData::Custom(node)) = &restored.doc.get(id).unwrap().content else {
        panic!("custom atom was not restored")
    };
    assert_eq!(
        node.data::<Payload>(),
        Some(&Payload {
            count: 42,
            label: "α".into()
        })
    );
    let id = native.doc.leaves()[0];
    let entry = native.doc.get_mut(id).unwrap();
    entry.content = BlockContent::Atom(AtomData::Custom(MarkdownNode::new("unsupported", 123_u16)));
    assert!(
        native
            .rich_document()
            .unwrap_err()
            .contains("requires a registered rich codec")
    );
}
