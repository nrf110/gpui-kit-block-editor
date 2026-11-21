use super::*;
use crate::native_editor::{
    BlockEditorModel, BlockPosition, BlockSelection, EditorCommand, TableEdit,
};
use crate::{RichNode, RichRun};
use gpui_kit::base::text::MarkdownExtensions;
use std::collections::BTreeMap;

const TABLE: &str = "| H0 | H1 |\n| --- | ---: |\n| B0 | B1 |";

#[derive(Clone, Copy, Debug)]
struct Conflict {
    left: (usize, usize, TableEdit),
    right: (usize, usize, TableEdit),
    left_followup: Option<(usize, usize, TableEdit)>,
}

fn conflicts() -> [Conflict; 8] {
    [
        Conflict {
            left: (1, 0, TableEdit::AddRowBelow),
            right: (0, 1, TableEdit::AddColumnRight),
            left_followup: None,
        },
        Conflict {
            left: (1, 0, TableEdit::AddRowBelow),
            right: (0, 1, TableEdit::RemoveColumn),
            left_followup: None,
        },
        Conflict {
            left: (1, 0, TableEdit::RemoveRow),
            right: (0, 1, TableEdit::AddColumnRight),
            left_followup: None,
        },
        Conflict {
            left: (0, 0, TableEdit::RemoveRow),
            right: (0, 1, TableEdit::RemoveColumn),
            left_followup: None,
        },
        Conflict {
            left: (0, 0, TableEdit::RemoveColumn),
            right: (0, 1, TableEdit::RemoveColumn),
            left_followup: None,
        },
        Conflict {
            left: (0, 0, TableEdit::RemoveRow),
            right: (1, 0, TableEdit::RemoveRow),
            left_followup: None,
        },
        Conflict {
            left: (0, 0, TableEdit::AddColumnLeft),
            right: (0, 1, TableEdit::AddColumnRight),
            left_followup: None,
        },
        Conflict {
            left: (1, 0, TableEdit::AddRowBelow),
            right: (0, 1, TableEdit::RemoveColumn),
            left_followup: Some((0, 0, TableEdit::RemoveColumn)),
        },
    ]
}

fn replica(document: &Document) -> Document {
    let mut peer = Document::new();
    peer.apply_update(&document.encode_state()).unwrap();
    peer
}

fn content(document: &Document, section: &str) -> RichDocument {
    document
        .sections()
        .into_iter()
        .find(|entry| entry.id == section)
        .unwrap()
        .content
}

fn cells(content: &RichDocument) -> BTreeMap<String, (String, String)> {
    content.blocks[0]
        .children
        .iter()
        .flat_map(|row| {
            row.children
                .iter()
                .map(|cell| (cell.id.clone(), (row.id.clone(), cell.text())))
        })
        .collect()
}

fn native_edit(
    document: &mut Document,
    section: &str,
    edit: (usize, usize, TableEdit),
    label: &str,
) {
    let before = content(document, section);
    let existing = cells(&before);
    let cell = &before.blocks[0].children[edit.0].children[edit.1].id;
    let mut native = BlockEditorModel::from_rich(&before, MarkdownExtensions::default()).unwrap();
    native
        .set_selection(BlockSelection::caret(BlockPosition::new(
            native.block_id(cell).unwrap(),
            0,
        )))
        .unwrap();
    native.run(EditorCommand::EditTable(edit.2)).unwrap();
    let mut after = native.rich_document().unwrap();
    let mut next = 0;
    for cell in after.blocks[0]
        .children
        .iter_mut()
        .flat_map(|row| &mut row.children)
    {
        if !existing.contains_key(&cell.id) {
            cell.runs = vec![RichRun {
                text: format!("{label}{next}"),
                ..Default::default()
            }];
            next += 1;
        }
    }
    document.set_content(section, &after).unwrap();
}

fn expected_cells(
    base: &RichDocument,
    left: &RichDocument,
    right: &RichDocument,
) -> BTreeMap<String, (String, String)> {
    let base_cells = cells(base);
    let left_cells = cells(left);
    let right_cells = cells(right);
    let deleted_rows: HashSet<_> = base.blocks[0]
        .children
        .iter()
        .filter(|row| {
            !left.blocks[0]
                .children
                .iter()
                .any(|candidate| candidate.id == row.id)
                || !right.blocks[0]
                    .children
                    .iter()
                    .any(|candidate| candidate.id == row.id)
        })
        .map(|row| row.id.as_str())
        .collect();
    left_cells
        .iter()
        .chain(&right_cells)
        .filter(|(id, (row, _))| {
            !deleted_rows.contains(row.as_str())
                && (!base_cells.contains_key(*id)
                    || (left_cells.contains_key(*id) && right_cells.contains_key(*id)))
        })
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect()
}

fn apply(document: &mut Document, update: &[u8], framed: bool) {
    if framed {
        let message = Message::Sync(SyncMessage::Update(update.to_vec())).encode_v1();
        assert!(
            document.receive_sync(&message).unwrap().is_empty(),
            "remote updates must not echo"
        );
    } else {
        document.apply_update(update).unwrap();
    }
}

fn assert_complete_export(content: &RichDocument) {
    let markdown = content.to_markdown().unwrap();
    let exported = RichDocument::from_markdown(&markdown).unwrap();
    let mut expected: Vec<_> = cells(content)
        .into_values()
        .map(|(_, text)| text)
        .filter(|text| !text.is_empty())
        .collect();
    let mut actual: Vec<_> = exported
        .blocks
        .iter()
        .filter(|node| node.kind == RichKind::Table)
        .flat_map(|table| &table.children)
        .flat_map(|row| &row.children)
        .map(RichNode::text)
        .filter(|text| !text.is_empty())
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(
        actual, expected,
        "Markdown dropped or duplicated real cells: {markdown}"
    );
}

#[test]
fn concurrent_table_commands_merge_in_both_orders_raw_and_framed_without_loss() {
    for conflict in conflicts() {
        let mut seed = Document::new();
        let section = seed.insert_section(None, TABLE).unwrap();
        let base = content(&seed, &section);
        let anchors: Vec<_> = cells(&base)
            .keys()
            .map(|cell| {
                seed.relative_position(&section, cell, 1, Assoc::After)
                    .unwrap()
            })
            .collect();
        let mut left = replica(&seed);
        let mut right = replica(&seed);
        native_edit(&mut left, &section, conflict.left, "LEFT");
        if let Some(edit) = conflict.left_followup {
            native_edit(&mut left, &section, edit, "FOLLOWUP");
        }
        native_edit(&mut right, &section, conflict.right, "RIGHT");
        let left_content = content(&left, &section);
        let right_content = content(&right, &section);
        let expected = expected_cells(&base, &left_content, &right_content);
        let expected_rows: HashSet<_> = left_content.blocks[0]
            .children
            .iter()
            .chain(&right_content.blocks[0].children)
            .filter(|row| {
                !base.blocks[0]
                    .children
                    .iter()
                    .any(|original| original.id == row.id)
                    || (left_content.blocks[0]
                        .children
                        .iter()
                        .any(|candidate| candidate.id == row.id)
                        && right_content.blocks[0]
                            .children
                            .iter()
                            .any(|candidate| candidate.id == row.id))
            })
            .map(|row| row.id.clone())
            .collect();
        let updates = [
            left.encode_update(&seed.state_vector()).unwrap(),
            right.encode_update(&seed.state_vector()).unwrap(),
        ];
        let mut canonical = None;
        for framed in [false, true] {
            for reverse in [false, true] {
                let mut merged = replica(&seed);
                let order = if reverse { [1, 0] } else { [0, 1] };
                for index in order {
                    apply(&mut merged, &updates[index], framed);
                }
                let snapshot = content(&merged, &section);
                assert_eq!(cells(&snapshot), expected, "{conflict:?}");
                let actual_rows: HashSet<_> = snapshot.blocks[0]
                    .children
                    .iter()
                    .map(|row| row.id.clone())
                    .collect();
                assert_eq!(
                    actual_rows, expected_rows,
                    "real row identities changed: {conflict:?}"
                );
                assert_eq!(snapshot.blocks[0].id, base.blocks[0].id);
                if let Some(canonical) = &canonical {
                    assert_eq!(&snapshot, canonical, "{conflict:?}");
                } else {
                    canonical = Some(snapshot.clone());
                }
                for anchor in &anchors {
                    let expected_position = expected
                        .contains_key(anchor.node())
                        .then(|| (section.clone(), anchor.node().to_owned(), 1));
                    assert_eq!(
                        merged.resolve_position(anchor),
                        expected_position,
                        "{conflict:?}"
                    );
                }
                let vector = merged.state_vector();
                let mut native =
                    BlockEditorModel::from_rich(&snapshot, MarkdownExtensions::default()).unwrap();
                assert_eq!(
                    native.rich_document().unwrap(),
                    snapshot,
                    "mounting must not fabricate cells"
                );
                assert_complete_export(&snapshot);
                assert_eq!(
                    merged.state_vector(),
                    vector,
                    "projection/export must not write repairs"
                );
                let table = native.block_id(&snapshot.blocks[0].id).unwrap();
                let old_ids: BTreeMap<_, _> = expected
                    .keys()
                    .chain(&expected_rows)
                    .map(|id| (id.clone(), native.block_id(id).unwrap()))
                    .collect();
                let edit = if expected.is_empty() {
                    TableEdit::AddColumnRight
                } else {
                    TableEdit::AddRowBelow
                };
                native.run(EditorCommand::EditTable(edit)).unwrap();
                for (stable, id) in old_ids {
                    assert_eq!(native.block_id(&stable), Some(id));
                }
                assert_eq!(native.block_id(&snapshot.blocks[0].id), Some(table));
                let edited = native.rich_document().unwrap();
                assert!(cells(&edited).len() > expected.len());
                let mut observer = replica(&merged);
                merged.set_content(&section, &edited).unwrap();
                observer
                    .apply_update(&merged.encode_update(&observer.state_vector()).unwrap())
                    .unwrap();
                assert_eq!(observer.sections(), merged.sections());
                assert_complete_export(&content(&merged, &section));
            }
        }
    }
}

#[test]
fn malformed_table_children_and_alignment_values_still_reject_atomically() {
    let mut seed = Document::new();
    let section = seed.insert_section(None, TABLE).unwrap();
    let base = content(&seed, &section);
    for corruption in 0..3 {
        let mut peer = replica(&seed);
        native_edit(&mut peer, &section, (1, 0, TableEdit::AddRowBelow), "VALID");
        {
            let mut txn = peer.doc.transact_mut_with(REMOTE_ORIGIN);
            let section_map = peer.section_map(&txn, &section).unwrap();
            let body = rich_storage::body(&section_map, &txn).unwrap();
            let Some(Out::YMap(nodes)) = body.get(&txn, "nodes") else {
                panic!("missing nodes")
            };
            let id = match corruption {
                0 => &base.blocks[0].children[0].children[0].id,
                1 => &base.blocks[0].children[0].id,
                _ => &base.blocks[0].id,
            };
            let Some(Out::YMap(node)) = nodes.get(&txn, id.as_str()) else {
                panic!("missing node")
            };
            if corruption == 2 {
                let Some(Out::YMap(attrs)) = node.get(&txn, "attrs") else {
                    panic!("missing attrs")
                };
                attrs.insert(&mut txn, "aligns", "not an array");
            } else {
                let kind = if corruption == 0 {
                    RichKind::Paragraph
                } else {
                    RichKind::TableCell
                };
                node.insert(&mut txn, "kind", serde_json::to_string(&kind).unwrap());
            }
        }
        let update = peer.encode_update(&seed.state_vector()).unwrap();
        for framed in [false, true] {
            let mut target = replica(&seed);
            let before = target.encode_state();
            if framed {
                let frame = Message::Sync(SyncMessage::Update(update.clone())).encode_v1();
                assert!(target.receive_sync(&frame).is_err());
            } else {
                assert!(target.apply_update(&update).is_err());
            }
            assert_eq!(
                target.encode_state(),
                before,
                "malformed update partially committed"
            );
            assert_eq!(content(&target, &section), base);
        }
    }
}

struct NativeTableRoot(gpui_kit::Entity<crate::native_editor::BlockEditorState>);

impl gpui_kit::Render for NativeTableRoot {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div()
            .w(gpui_kit::px(700.))
            .h(gpui_kit::px(600.))
            .child(crate::native_editor::BlockEditor::new(&self.0))
    }
}

#[gpui_kit::test]
fn merged_tables_render_every_surviving_cell_and_remain_natively_editable(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::AppContext;
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let (root, cx) = cx.add_window_view(|window, cx| {
        NativeTableRoot(cx.new(|cx| crate::native_editor::BlockEditorState::new(window, cx)))
    });
    let editor = root.read_with(cx, |root, _| root.0.clone());
    for conflict in conflicts() {
        let mut seed = Document::new();
        let section = seed.insert_section(None, TABLE).unwrap();
        let mut left = replica(&seed);
        let mut right = replica(&seed);
        native_edit(&mut left, &section, conflict.left, "LEFT");
        if let Some(edit) = conflict.left_followup {
            native_edit(&mut left, &section, edit, "FOLLOWUP");
        }
        native_edit(&mut right, &section, conflict.right, "RIGHT");
        left.apply_update(&right.encode_state()).unwrap();
        let snapshot = content(&left, &section);
        editor.update(cx, |state, cx| state.set_document(&snapshot, cx).unwrap());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let expected = cells(&snapshot);
        for (stable, (_, text)) in &expected {
            editor.update(cx, |state, cx| {
                let block = state.model().block_id(stable).unwrap();
                state
                    .set_selection(
                        BlockSelection::Text {
                            anchor: BlockPosition::new(block, 0),
                            head: BlockPosition::new(block, text.len()),
                        },
                        cx,
                    )
                    .unwrap();
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            editor.read_with(cx, |state, _| {
                assert!(
                    state.selection_bounds().is_some(),
                    "real cell is not visible: {text}; {conflict:?}"
                )
            });
        }
        editor.update(cx, |state, cx| {
            let table = state.model().block_id(&snapshot.blocks[0].id).unwrap();
            state
                .run(EditorCommand::EditTable(TableEdit::AddColumnRight), cx)
                .unwrap();
            let edited = state.model().rich_document().unwrap();
            assert_eq!(state.model().block_id(&snapshot.blocks[0].id), Some(table));
            for (id, value) in &expected {
                assert_eq!(cells(&edited).get(id), Some(value));
            }
            assert!(cells(&edited).len() > expected.len());
            assert_complete_export(&edited);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}
