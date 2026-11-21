use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use markdown_editor::rich::{RichDocument, RichKind, RichNode};
use markdown_editor::{Affinity, BlockEditorModel, BlockId, BlockPosition, BlockSelection};
use unicode_segmentation::UnicodeSegmentation;
use yrs::Assoc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionKind {
    Text,
    Blocks,
    Cells,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RichPoint {
    pub node: String,
    /// UTF-8 byte offset in semantic text, never in Markdown source.
    pub offset: usize,
    /// Before/after an atomic block; its CRDT text anchor remains offset zero.
    pub after_atom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectionOffsets {
    pub kind: SelectionKind,
    pub anchor: RichPoint,
    pub head: RichPoint,
}

struct Node {
    id: Arc<str>,
    native: BlockId,
    parent: Option<usize>,
    table: Option<usize>,
    kind: RichKind,
    boundaries: Vec<usize>,
    atomic: bool,
}

/// Selection mapping for one native model generation. Stable semantic UUIDs
/// make formatting delimiters, parser spans, and text searches unnecessary.
pub(crate) struct RichMap {
    nodes: Vec<Node>,
    native: HashMap<BlockId, usize>,
    stable: HashMap<Arc<str>, usize>,
    first: Option<BlockPosition>,
    fallback: BlockSelection,
}

impl RichMap {
    pub(crate) fn new(content: &RichDocument, model: &BlockEditorModel) -> Result<Self> {
        let mut map = Self {
            nodes: Vec::new(),
            native: HashMap::new(),
            stable: HashMap::new(),
            first: model
                .document()
                .leaves()
                .next()
                .map(|_| model.document().first_caret()),
            fallback: model.selection().clone(),
        };
        for node in &content.blocks {
            map.add(node, None, None, model)?;
        }
        Ok(map)
    }

    fn add(
        &mut self,
        node: &RichNode,
        parent: Option<usize>,
        table: Option<usize>,
        model: &BlockEditorModel,
    ) -> Result<()> {
        let native = model
            .block_id(&node.id)
            .context("semantic node is absent from the native model")?;
        let block = model
            .document()
            .block(native)
            .context("native block is absent")?;
        let atomic = block.text().is_none()
            && matches!(
                node.kind,
                RichKind::HorizontalRule
                    | RichKind::Break
                    | RichKind::Custom
                    | RichKind::Definition
                    | RichKind::Unknown
            );
        let mut boundaries = Vec::new();
        if let Some(text) = block.text() {
            let mut at = 0;
            for run in &node.runs {
                let end = at + run.text.len();
                ensure!(
                    text.get(at..end) == Some(run.text.as_str()),
                    "semantic/native text mismatch for {}",
                    node.id
                );
                at = end;
            }
            ensure!(
                at == text.len(),
                "semantic/native text length mismatch for {}",
                node.id
            );
            boundaries.extend(text.grapheme_indices(true).map(|(offset, _)| offset));
            boundaries.push(text.len());
        } else {
            ensure!(
                node.runs.is_empty()
                    || (atomic
                        && node.kind == RichKind::Custom
                        && matches!(node.runs.as_slice(), [run] if run.text == "\u{FFFC}"
                            && matches!(run.atom, Some(markdown_editor::rich::RichAtom::Custom { .. })))),
                "non-text native node has semantic text"
            );
            boundaries.push(0);
        }
        let id: Arc<str> = Arc::from(node.id.as_str());
        let index = self.nodes.len();
        ensure!(
            self.stable.insert(id.clone(), index).is_none(),
            "duplicate semantic node UUID"
        );
        self.native.insert(native, index);
        self.nodes.push(Node {
            id,
            native,
            parent,
            table,
            kind: node.kind,
            boundaries,
            atomic,
        });
        let table = if node.kind == RichKind::Table {
            Some(index)
        } else {
            table
        };
        for child in &node.children {
            self.add(child, Some(index), table, model)?;
        }
        Ok(())
    }

    pub(crate) fn point(&self, position: BlockPosition) -> Option<RichPoint> {
        let node = &self.nodes[*self.native.get(&position.block)?];
        if node.atomic {
            if position.offset > 1 {
                return None;
            }
            return Some(RichPoint {
                node: node.id.to_string(),
                offset: 0,
                after_atom: position.offset == 1,
            });
        }
        node.boundaries.binary_search(&position.offset).ok()?;
        Some(RichPoint {
            node: node.id.to_string(),
            offset: position.offset,
            after_atom: false,
        })
    }

    pub(crate) fn position(&self, point: &RichPoint, assoc: Assoc) -> Option<BlockPosition> {
        let Some(index) = self.stable.get(point.node.as_str()) else {
            return self.first;
        };
        let node = &self.nodes[*index];
        if matches!(
            node.kind,
            RichKind::List
                | RichKind::ListItem
                | RichKind::Quote
                | RichKind::Table
                | RichKind::TableRow
        ) {
            return self.first;
        }
        if node.atomic {
            return Some(BlockPosition {
                block: node.native,
                offset: usize::from(point.after_atom),
                affinity: match assoc {
                    Assoc::Before => Affinity::Upstream,
                    Assoc::After => Affinity::Downstream,
                },
            });
        }
        let boundary = match node.boundaries.binary_search(&point.offset) {
            Ok(index) => index,
            Err(index) => match assoc {
                Assoc::Before => index.saturating_sub(1),
                Assoc::After => index.min(node.boundaries.len().checked_sub(1)?),
            },
        };
        Some(BlockPosition {
            block: node.native,
            offset: node.boundaries[boundary],
            affinity: match assoc {
                Assoc::Before => Affinity::Upstream,
                Assoc::After => Affinity::Downstream,
            },
        })
    }

    fn node_point(&self, native: BlockId) -> Option<RichPoint> {
        let node = &self.nodes[*self.native.get(&native)?];
        Some(RichPoint {
            node: node.id.to_string(),
            offset: 0,
            after_atom: false,
        })
    }

    pub(crate) fn selection_offsets(&self, selection: &BlockSelection) -> Option<SelectionOffsets> {
        let (kind, anchor, head) = match selection {
            BlockSelection::Text { anchor, head } => (
                SelectionKind::Text,
                self.point(*anchor)?,
                self.point(*head)?,
            ),
            BlockSelection::Blocks { anchor, head } => {
                let a = &self.nodes[*self.native.get(anchor)?];
                let b = &self.nodes[*self.native.get(head)?];
                if a.parent != b.parent {
                    return None;
                }
                (
                    SelectionKind::Blocks,
                    self.node_point(*anchor)?,
                    self.node_point(*head)?,
                )
            }
            BlockSelection::Cells { anchor, head } => {
                let a = &self.nodes[*self.native.get(anchor)?];
                let b = &self.nodes[*self.native.get(head)?];
                if a.kind != RichKind::TableCell
                    || b.kind != RichKind::TableCell
                    || a.table.is_none()
                    || a.table != b.table
                {
                    return None;
                }
                (
                    SelectionKind::Cells,
                    self.node_point(*anchor)?,
                    self.node_point(*head)?,
                )
            }
        };
        Some(SelectionOffsets { kind, anchor, head })
    }

    pub(crate) fn restore_selection(&self, selection: SelectionOffsets) -> Option<BlockSelection> {
        let anchor = self.stable.get(selection.anchor.node.as_str()).copied();
        let head = self.stable.get(selection.head.node.as_str()).copied();
        if let (Some(a), Some(b)) = (anchor, head) {
            let a = &self.nodes[a];
            let b = &self.nodes[b];
            match selection.kind {
                SelectionKind::Blocks if a.parent == b.parent => {
                    return Some(BlockSelection::Blocks {
                        anchor: a.native,
                        head: b.native,
                    });
                }
                SelectionKind::Cells
                    if a.kind == RichKind::TableCell
                        && b.kind == RichKind::TableCell
                        && a.table.is_some()
                        && a.table == b.table =>
                {
                    return Some(BlockSelection::Cells {
                        anchor: a.native,
                        head: b.native,
                    });
                }
                _ => {}
            }
        }
        if self.first.is_none() {
            return Some(self.fallback.clone());
        }
        let reversed = match (anchor, head) {
            (Some(a), Some(b)) => {
                (a, selection.anchor.offset, selection.anchor.after_atom)
                    > (b, selection.head.offset, selection.head.after_atom)
            }
            _ => false,
        };
        let anchor = self.position(
            &selection.anchor,
            if reversed {
                Assoc::Before
            } else {
                Assoc::After
            },
        )?;
        let head = if selection.anchor == selection.head {
            anchor
        } else {
            self.position(
                &selection.head,
                if reversed {
                    Assoc::After
                } else {
                    Assoc::Before
                },
            )?
        };
        Some(BlockSelection::Text { anchor, head })
    }

    pub(crate) fn first_position(&self) -> Option<BlockPosition> {
        self.first
    }
}

#[cfg(test)]
#[path = "rich_map_tests.rs"]
mod tests;
