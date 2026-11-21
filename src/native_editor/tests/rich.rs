// Modified for gpui-kit-block-editor: incorporated gpui-kit-markdown-editor tests
// (Apache-2.0), relocated into the private native_editor module.

use super::*;
use crate::native_editor::document::{AtomData, BlockContent};
use crate::native_editor::rich::*;
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
        entry.kind = crate::native_editor::kind::BlockKindData::Custom;
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
    entry.kind = crate::native_editor::kind::BlockKindData::Custom;
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

#[test]
fn ragged_tables_keep_real_cells_and_navigate_empty_rows() {
    let mut document = RichDocument::from_markdown(
        "| H0 | H1 | H2 |\n| --- | --- | --- |\n| M0 | M1 | M2 |\n| B0 | B1 | B2 |",
    )
    .unwrap();
    let table = &mut document.blocks[0];
    table
        .attrs
        .insert("aligns".into(), json!(["right", "center", "left", "none"]));
    table.children[0].children.truncate(1);
    table.children[1].children.clear();
    let header = table.children[0].children[0].id.clone();
    let last_row = table.children[2].id.clone();
    let last_cell = table.children[2].children[2].id.clone();
    let mut native = restore(&document);
    assert_eq!(native.rich_document().unwrap(), document);
    let table_id = native.block_id(&document.blocks[0].id).unwrap();
    let projected = crate::native_editor::direct::project_block(&native.doc, table_id).unwrap();
    let gpui_kit::base::text::editor_host::BlockNode::Table(projected) = projected else {
        panic!("table did not project")
    };
    assert_eq!(projected.column_aligns().len(), 3);
    assert_eq!(
        projected.rows().map(|row| row.count()).collect::<Vec<_>>(),
        [1, 0, 3]
    );
    native
        .set_selection(BlockSelection::caret(BlockPosition::new(
            native.block_id(&header).unwrap(),
            0,
        )))
        .unwrap();
    native.table_enter().unwrap();
    assert_eq!(
        native.stable_id(native.caret().unwrap().block),
        Some(document.blocks[0].children[2].children[0].id.clone()),
    );
    native
        .select_table_line(table_id, crate::native_editor::TableLine::Column(2))
        .unwrap();
    assert_eq!(
        native.selection(),
        &BlockSelection::Cells {
            anchor: native.block_id(&last_cell).unwrap(),
            head: native.block_id(&last_cell).unwrap(),
        }
    );
    native
        .run(EditorCommand::EditTable(
            crate::native_editor::TableEdit::RemoveColumn,
        ))
        .unwrap();
    assert!(native.block_id(&last_cell).is_none());
    assert!(native.block_id(&last_row).is_some());
    native
        .run(EditorCommand::EditTable(
            crate::native_editor::TableEdit::RemoveRow,
        ))
        .unwrap();
    native
        .set_selection(BlockSelection::caret(BlockPosition::new(
            native.block_id(&header).unwrap(),
            0,
        )))
        .unwrap();
    native
        .run(EditorCommand::EditTable(
            crate::native_editor::TableEdit::RemoveRow,
        ))
        .unwrap();
    assert_eq!(
        native.selection(),
        &BlockSelection::Blocks {
            anchor: table_id,
            head: table_id
        }
    );
    assert_eq!(native.rich_document().unwrap().blocks[0].children.len(), 1);
    native
        .run(EditorCommand::EditTable(
            crate::native_editor::TableEdit::AddColumnRight,
        ))
        .unwrap();
    assert_eq!(native.document().leaves().count(), 1);
    assert_eq!(native.block_id(&document.blocks[0].id), Some(table_id));
    assert_invariants(&native);
}

#[test]
fn tab_appends_visible_width_and_table_copy_pads_short_header() {
    let mut document =
        RichDocument::from_markdown("| H0 | H1 | H2 |\n| --- | --- | --- |\n| B0 | B1 | B2 |")
            .unwrap();
    document.blocks[0].children[1].children.truncate(1);
    let mut native = restore(&document);
    let last = native
        .block_id(&document.blocks[0].children[1].children[0].id)
        .unwrap();
    native
        .set_selection(BlockSelection::caret(BlockPosition::new(last, 0)))
        .unwrap();
    native.run(EditorCommand::Indent).unwrap();
    assert_eq!(
        native.rich_document().unwrap().blocks[0].children[2]
            .children
            .len(),
        3
    );
    document.blocks[0].children[0].children.truncate(1);
    document.blocks[0].children[1].children.push(RichNode {
        kind: RichKind::TableCell,
        ..RichNode::paragraph("EXTRA")
    });
    let mut native = restore(&document);
    let cells: Vec<_> = native.document().leaves().map(|cell| cell.id()).collect();
    native
        .set_selection(BlockSelection::Cells {
            anchor: cells[0],
            head: cells[2],
        })
        .unwrap();
    let copied = native.cells_markdown().unwrap();
    let restored = RichDocument::from_markdown(&copied).unwrap();
    assert!(restored.plain_text().contains("EXTRA"), "{copied}");
    assert_invariants(&native);
}

#[test]
fn empty_tables_mount_initialize_and_delete_without_repair_cells() {
    for rows in [0, 2] {
        let mut document =
            RichDocument::from_markdown("| H0 | H1 |\n| --- | --- |\n| B0 | B1 |").unwrap();
        document.blocks[0].children.truncate(rows);
        for row in &mut document.blocks[0].children {
            row.children.clear();
        }
        let mut native = restore(&document);
        let table = native.block_id(&document.blocks[0].id).unwrap();
        assert_eq!(
            native.selection(),
            &BlockSelection::Blocks {
                anchor: table,
                head: table
            }
        );
        native.run(EditorCommand::SelectAll).unwrap();
        assert_eq!(
            native.selection(),
            &BlockSelection::Blocks {
                anchor: table,
                head: table
            }
        );
        assert_eq!(native.rich_document().unwrap(), document);
        native
            .run(EditorCommand::EditTable(
                crate::native_editor::TableEdit::AddRowBelow,
            ))
            .unwrap();
        assert_eq!(native.document().leaves().count(), 1);
        assert_eq!(native.block_id(&document.blocks[0].id), Some(table));
        native.set_document(&document).unwrap();
        native.run(EditorCommand::DeleteSelection).unwrap();
        assert_eq!(native.document().leaves().count(), 1);
        assert_eq!(
            native.document().leaves().next().unwrap().kind(),
            crate::native_editor::BlockKind::Paragraph
        );
        assert!(native.caret().is_some());
        assert_invariants(&native);
        native.set_document(&document).unwrap();
        native
            .insert_text("replacement", EditOrigin::Typing)
            .unwrap();
        assert_eq!(native.source(), "replacement");
        assert_invariants(&native);
    }
}

#[test]
fn deleting_a_neighbor_selects_surviving_empty_table_without_invalid_caret() {
    for table_first in [false, true] {
        let mut document = RichDocument::from_markdown("| H0 |\n| --- |\n\nneighbor").unwrap();
        document.blocks[0].children.clear();
        if !table_first {
            document.blocks.swap(0, 1);
        }
        let mut native = restore(&document);
        let paragraph = native.document().leaves().next().unwrap().id();
        native
            .set_selection(BlockSelection::Blocks {
                anchor: paragraph,
                head: paragraph,
            })
            .unwrap();
        native.run(EditorCommand::DeleteSelection).unwrap();
        let table = native
            .block_id(&document.blocks[usize::from(!table_first)].id)
            .unwrap();
        assert_eq!(
            native.selection(),
            &BlockSelection::Blocks {
                anchor: table,
                head: table
            }
        );
        native.set_selection(native.selection().clone()).unwrap();
        assert_eq!(native.document().leaves().count(), 0);
        assert_invariants(&native);
        native
            .run(EditorCommand::EditTable(
                crate::native_editor::TableEdit::AddColumnRight,
            ))
            .unwrap();
        assert_eq!(native.document().leaves().count(), 1);
        assert_invariants(&native);
    }
}

#[test]
fn selection_ranges_and_copy_skip_empty_table_edge_rows() {
    let mut document = RichDocument::from_markdown(
        "before\n\n| H0 | H1 |\n| --- | --- |\n| MID0 | MID1 |\n| LAST0 | LAST1 |\n\nafter",
    )
    .unwrap();
    document.blocks[1].children[0].children.clear();
    document.blocks[1].children[2].children.clear();
    let mut native = restore(&document);
    let table = native.block_id(&document.blocks[1].id).unwrap();
    native
        .set_selection(BlockSelection::Blocks {
            anchor: table,
            head: table,
        })
        .unwrap();
    let range = native.doc.resolve_selection(native.selection()).unwrap();
    assert_eq!(
        native.stable_id(range.start.block),
        Some(document.blocks[1].children[1].children[0].id.clone())
    );
    assert_eq!(
        native.stable_id(range.end.block),
        Some(document.blocks[1].children[1].children[1].id.clone())
    );
    native
        .run(EditorCommand::ToggleMark(MarkKind::Bold))
        .unwrap();
    let range = crate::native_editor::selection::ResolvedSelection {
        start: native.doc.first_caret(),
        end: native.doc.last_caret(),
    };
    let copied = crate::native_editor::slice::selection_markdown(&native.doc, range).unwrap();
    let restored = RichDocument::from_markdown(&copied).unwrap();
    for text in ["before", "MID0", "MID1", "after"] {
        assert!(restored.plain_text().contains(text), "{copied}");
    }
    assert_invariants(&native);
}

#[test]
fn run_validation_is_context_independent_without_weakening_node_rules() {
    let mut run = RichRun {
        text: "\u{FFFC}".into(),
        marks: RichMarks {
            bold: true,
            ..Default::default()
        },
        atom: None,
    };
    run.validate().unwrap();
    let mut document = RichDocument::paragraph("");
    document.blocks[0].runs = vec![run.clone()];
    assert!(
        document.validate().is_err(),
        "ordinary text still requires atom metadata"
    );
    document.blocks[0].kind = RichKind::CodeBlock;
    assert!(
        document.validate().is_err(),
        "direct code imports still reject marks"
    );
    run.atom = Some(RichAtom::Image {
        url: "image".into(),
        link: None,
        title: None,
        alt: None,
        width: Some(RichLength::Pixels(-1.)),
        height: None,
    });
    assert!(run.validate().is_err());
    run.atom = Some(RichAtom::Image {
        url: "image".into(),
        link: None,
        title: None,
        alt: None,
        width: Some(RichLength::Pixels(12.)),
        height: None,
    });
    run.validate().unwrap();
    run.text.push('\u{FFFC}');
    assert!(
        run.validate().is_err(),
        "atom encoding must still be one replacement character"
    );
    run.atom = None;
    run.marks.highlight = Some([2., 0., 0., 1.]);
    assert!(run.validate().is_err());
}
