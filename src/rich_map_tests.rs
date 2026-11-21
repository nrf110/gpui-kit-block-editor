use super::*;
use crate::native_editor::{BlockKind, EditorCommand, MarkKind};
use gpui_kit::base::text::MarkdownExtensions;

fn parsed(source: &str) -> BlockEditorModel {
    BlockEditorModel::from_markdown(source, MarkdownExtensions::default()).unwrap()
}

#[test]
fn literal_markdown_characters_are_semantic_text_not_syntax() {
    let content = RichDocument::paragraph("**literal** <b> &amp;");
    let model = BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let map = RichMap::new(&content, &model).unwrap();
    let leaf = model.document().leaves().next().unwrap();
    assert_eq!(leaf.text(), Some("**literal** <b> &amp;"));
    let point = map.point(BlockPosition::new(leaf.id(), 2)).unwrap();
    assert_eq!(point.offset, 2);
    assert_eq!(map.position(&point, Assoc::After).unwrap().offset, 2);
}

#[test]
fn formatting_changes_and_native_recreation_keep_reversed_unicode_selection() {
    let mut model = parsed("same **café** and same 🦀");
    let leaf = model.document().leaves().next().unwrap().id();
    model
        .set_selection(BlockSelection::Text {
            anchor: BlockPosition::new(leaf, "same café and same 🦀".len()),
            head: BlockPosition::new(leaf, "same ".len()),
        })
        .unwrap();
    let before = model.rich_document().unwrap();
    let offsets = RichMap::new(&before, &model)
        .unwrap()
        .selection_offsets(model.selection())
        .unwrap();
    model
        .run(EditorCommand::ToggleMark(MarkKind::Italic))
        .unwrap();
    let after = model.rich_document().unwrap();
    assert_eq!(before.blocks[0].id, after.blocks[0].id);
    let mut restored = BlockEditorModel::from_rich(&after, MarkdownExtensions::default()).unwrap();
    let selection = RichMap::new(&after, &restored)
        .unwrap()
        .restore_selection(offsets.clone())
        .unwrap();
    restored.set_selection(selection).unwrap();
    let actual = RichMap::new(&after, &restored)
        .unwrap()
        .selection_offsets(restored.selection())
        .unwrap();
    assert_eq!(actual, offsets);
}

#[test]
fn positions_clamp_to_graphemes_without_markdown_offsets() {
    let content = RichDocument::paragraph("a👩‍💻e\u{301}b");
    let model = BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let map = RichMap::new(&content, &model).unwrap();
    let point = RichPoint {
        node: content.blocks[0].id.clone(),
        offset: 2,
        after_atom: false,
    };
    assert_eq!(map.position(&point, Assoc::Before).unwrap().offset, 1);
    assert_eq!(
        map.position(&point, Assoc::After).unwrap().offset,
        "a👩‍💻".len()
    );
    let at_end = RichPoint {
        offset: usize::MAX,
        ..point
    };
    assert_eq!(
        map.position(&at_end, Assoc::After).unwrap().offset,
        "a👩‍💻e\u{301}b".len()
    );
}

#[test]
fn cell_ranges_restore_against_stable_cell_ids() {
    let model = parsed("| A | B |\n| --- | --- |\n| same | same |");
    let content = model.rich_document().unwrap();
    let cells: Vec<_> = model
        .document()
        .leaves()
        .filter(|leaf| leaf.kind() == BlockKind::TableCell)
        .map(|leaf| leaf.id())
        .collect();
    let original = BlockSelection::Cells {
        anchor: cells[3],
        head: cells[0],
    };
    let offsets = RichMap::new(&content, &model)
        .unwrap()
        .selection_offsets(&original)
        .unwrap();
    let mut restored =
        BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let map = RichMap::new(&content, &restored).unwrap();
    restored
        .set_selection(map.restore_selection(offsets.clone()).unwrap())
        .unwrap();
    assert_eq!(
        map.selection_offsets(restored.selection()).unwrap(),
        offsets
    );
}

#[test]
fn block_ranges_restore_after_rebuilding_native_ids() {
    let model = parsed("first\n\nsecond");
    let content = model.rich_document().unwrap();
    let roots: Vec<_> = model
        .document()
        .root()
        .children()
        .map(|node| node.id())
        .collect();
    let original = BlockSelection::Blocks {
        anchor: roots[1],
        head: roots[0],
    };
    let offsets = RichMap::new(&content, &model)
        .unwrap()
        .selection_offsets(&original)
        .unwrap();
    let mut restored =
        BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let map = RichMap::new(&content, &restored).unwrap();
    restored
        .set_selection(map.restore_selection(offsets.clone()).unwrap())
        .unwrap();
    assert_eq!(
        map.selection_offsets(restored.selection()).unwrap(),
        offsets
    );
}

#[test]
fn deleted_nodes_collapse_to_a_surviving_native_boundary() {
    let model = parsed("first\n\nsecond");
    let mut content = model.rich_document().unwrap();
    let deleted = RichPoint {
        node: content.blocks[0].id.clone(),
        offset: 3,
        after_atom: false,
    };
    content.blocks.remove(0);
    let mut survivor =
        BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let map = RichMap::new(&content, &survivor).unwrap();
    let position = map.position(&deleted, Assoc::After).unwrap();
    survivor
        .set_selection(BlockSelection::caret(position))
        .unwrap();
    assert_eq!(
        survivor.document().block(position.block).unwrap().text(),
        Some("second")
    );
    assert_eq!(position.offset, 0);
}

#[test]
fn atomic_after_carets_keep_their_side_without_fictitious_text_offsets() {
    let model = parsed("---");
    let content = model.rich_document().unwrap();
    let atomic = model
        .document()
        .leaves()
        .find(|node| node.kind() == BlockKind::HorizontalRule)
        .unwrap()
        .id();
    let map = RichMap::new(&content, &model).unwrap();
    let point = map.point(BlockPosition::new(atomic, 1)).unwrap();
    assert_eq!(point.offset, 0);
    assert!(point.after_atom);
    let mut restored =
        BlockEditorModel::from_rich(&content, MarkdownExtensions::default()).unwrap();
    let position = RichMap::new(&content, &restored)
        .unwrap()
        .position(&point, Assoc::After)
        .unwrap();
    restored
        .set_selection(BlockSelection::caret(position))
        .unwrap();
    assert_eq!(position.offset, 1);
    assert_eq!(
        restored.document().block(position.block).unwrap().kind(),
        BlockKind::HorizontalRule
    );
}
