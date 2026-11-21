// Modified for gpui-kit-block-editor: integrated from gpui-kit-markdown-editor
// as a private native-editor module; original implementation retained.
//! Per-frame geometry, collected through gpui-kit's editor host API.
//!
//! During the editor's prepaint every keyed text leaf reports a
//! [`LeafGeometry`] to `collect_leaf_geometry`, which also paints the
//! selection ranges handed to it. After prepaint the geometries become the
//! editor's [`FrameLayout`], which handlers hit-test against until the next
//! frame. Everything here maps between the model's offsets and the leaf
//! indices gpui-kit reports.

use std::{collections::HashMap, ops::Range, rc::Rc};

use gpui::{Bounds, Pixels, Point, point, px, size};
use gpui_kit::base::text::editor_host::{Caret, ColumnAlign, LeafGeometry};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation as _};

use super::{
    document::EditorDocument, id::BlockId, position::BlockPosition, projection::LeafInfo,
    selection::ResolvedSelection,
};

/// Builds the frame from what `collect_leaf_geometry` returned.
pub(crate) fn into_frame(
    mut geometries: HashMap<u64, LeafGeometry>,
    leaves: Rc<Vec<LeafInfo>>,
    tables: &[(BlockId, Vec<BlockId>)],
) -> FrameLayout {
    let mut frame = FrameLayout::default();
    for (table, cells) in tables {
        let bounds = cells
            .iter()
            .filter_map(|cell| geometries.get(&cell.as_u64()).map(|g| g.bounds()))
            .reduce(|a, b| a.union(&b));
        if let Some(bounds) = bounds {
            frame.tables.push((*table, bounds));
        }
    }
    for info in leaves.iter() {
        if let Some(geometry) = geometries.remove(&info.block.as_u64()) {
            frame.index.insert(info.block, frame.leaves.len());
            frame.leaves.push(LeafGeom {
                info: *info,
                geometry,
                align: ColumnAlign::None,
            });
        }
    }
    frame
}

pub(crate) struct LeafGeom {
    pub(crate) info: LeafInfo,
    geometry: LeafGeometry,
    /// A table cell's column alignment. Since gpui-kit 0.10 an aligned
    /// cell's text element is itself placed (centred / right) inside the
    /// cell, so `LeafGeometry` positions are where the text is painted and
    /// need no correction; the alignment only feeds `layout_tables`.
    pub(crate) align: ColumnAlign,
}

impl LeafGeom {
    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        self.geometry.bounds()
    }

    pub(crate) fn line_height(&self) -> Pixels {
        self.geometry.line_height()
    }

    /// Height of the caret: the font's ascent + descent (like a text
    /// input's caret), not the whole line box, which includes leading.
    pub(crate) fn caret_height(&self) -> Pixels {
        self.geometry.caret_height()
    }

    /// Rendered index for a model offset.
    fn index(&self, offset: usize) -> usize {
        let len = self.geometry.len();
        if self.info.atomic {
            if offset == 0 { 0 } else { len }
        } else {
            offset.min(self.info.len).min(len)
        }
    }

    /// Top of the caret at `offset`, window coordinates.
    pub(crate) fn caret_point(&self, offset: usize) -> Point<Pixels> {
        self.geometry.position_for_index(self.index(offset))
    }

    /// Closest model offset for a window point.
    pub(crate) fn offset_at(&self, at: Point<Pixels>) -> usize {
        let index = self.geometry.index_for_position(at);
        if self.info.atomic {
            usize::from(index * 2 > self.geometry.len().max(1))
        } else {
            index.min(self.info.len)
        }
    }

    /// The inline atom drawn under `at`, as its leaf byte range.
    pub(crate) fn atom_at(&self, at: Point<Pixels>) -> Option<Range<usize>> {
        self.geometry.atom_at(at)
    }

    /// The public host geometry exposes affinity-aware carets, not range
    /// rectangles. Measure each selected grapheme on its own visual row:
    /// endpoint-only bounds miss the wider rows between wrapped endpoints.
    fn selection_bounds(
        &self,
        text: &str,
        range: Range<usize>,
        viewport: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        if self.info.atomic || !self.bounds().intersects(&viewport) {
            return None;
        }
        let selected = text.get(range.clone())?;
        let line_height = self.line_height();
        let rendered_len = self.geometry.len();
        selected
            .grapheme_indices(true)
            .filter(|(_, grapheme)| !matches!(*grapheme, "\n" | "\r" | "\r\n" | "\u{fffc}"))
            .filter_map(|(offset, grapheme)| {
                let start = range.start + offset;
                let end = start + grapheme.len();
                if end > rendered_len {
                    return None;
                }
                // A wrap offset belongs to the next row at the start and
                // the previous row at the end of a selected grapheme.
                let a = self.geometry.position_for_caret(Caret::new(start));
                if a.y >= viewport.bottom() || a.y + line_height <= viewport.top() {
                    return None;
                }
                let b = self.geometry.position_for_caret(Caret::upstream(end));
                let bounds = Bounds::from_corners(
                    point(a.x.min(b.x), a.y),
                    point(a.x.max(b.x), a.y + line_height),
                )
                .intersect(&viewport);
                (bounds.size.width > px(0.) && bounds.size.height > px(0.)).then_some(bounds)
            })
            .reduce(|a, b| a.union(&b))
    }
}

/// The last complete frame's leaf geometry, in document order.
#[derive(Default)]
pub(crate) struct FrameLayout {
    pub(crate) leaves: Vec<LeafGeom>,
    pub(crate) index: HashMap<BlockId, usize>,
    /// Table bounds (union of its cells), for node selection by click.
    pub(crate) tables: Vec<(BlockId, Bounds<Pixels>)>,
    /// Reconstructed table grids (column / row boxes), see `layout_tables`.
    pub(crate) grids: Vec<TableGeom>,
    /// Cell boxes by cell id, for hit-testing between cells.
    pub(crate) cell_boxes: HashMap<BlockId, Bounds<Pixels>>,
    /// The editor's content area: tables span its width, while their cells'
    /// text bounds only cover the text.
    pub(crate) content: Option<Bounds<Pixels>>,
    /// The clipping mask in window coordinates, captured with this frame.
    pub(crate) viewport: Option<Bounds<Pixels>>,
}

impl FrameLayout {
    pub(crate) fn leaf(&self, block: BlockId) -> Option<&LeafGeom> {
        self.index.get(&block).map(|ix| &self.leaves[*ix])
    }

    /// Whole-block selection paint bounds, including table padding and slack.
    pub(crate) fn selected_block_bounds(
        &self,
        doc: &EditorDocument,
        parent: BlockId,
        range: Range<usize>,
    ) -> Option<Bounds<Pixels>> {
        fn subtree_bounds(
            frame: &FrameLayout,
            doc: &EditorDocument,
            block: BlockId,
        ) -> Option<Bounds<Pixels>> {
            if let Some(grid) = frame.grids.iter().find(|grid| grid.table == block) {
                return Some(grid.bounds);
            }
            // Without a reconstructed grid, retain the laid-out descendant
            // leaves as the fallback, just as for ordinary text containers.
            frame.leaf(block).map(LeafGeom::bounds).or_else(|| {
                doc.children(block)
                    .iter()
                    .filter_map(|child| subtree_bounds(frame, doc, *child))
                    .reduce(|a, b| a.union(&b))
            })
        }

        doc.children(parent)
            .get(range)?
            .iter()
            .filter_map(|block| subtree_bounds(self, doc, *block))
            .reduce(|a, b| a.union(&b))
    }

    pub(crate) fn caret_bounds(&self, pos: BlockPosition, width: Pixels) -> Option<Bounds<Pixels>> {
        let leaf = self.leaf(pos.block)?;
        let (line, h) = (leaf.line_height(), leaf.caret_height());
        let mut origin = leaf.caret_point(pos.offset);
        // GPUI centres the glyphs in the line box: so does the caret.
        origin.y += (line - h) / 2.;
        Some(Bounds::new(origin, size(width, h)))
    }

    /// Whether this caret is on the leaf's first/last visual row.
    pub(crate) fn at_vertical_edge(&self, pos: BlockPosition, down: bool) -> Option<bool> {
        let leaf = self.leaf(pos.block)?;
        let y = leaf.caret_point(pos.offset).y;
        let edge = leaf.caret_point(if down { leaf.info.len } else { 0 }).y;
        Some(if down { y >= edge } else { y <= edge })
    }

    /// Enter one model row (one leaf, or all cells of a table row).
    /// Restrict candidates to that row even when surrounding geometry is culled.
    pub(crate) fn vertical_entry(
        &self,
        doc: &EditorDocument,
        blocks: &[BlockId],
        x: Pixels,
        down: bool,
    ) -> Option<BlockPosition> {
        let leaf = blocks
            .iter()
            .filter_map(|block| self.leaf(*block))
            .min_by(|a, b| {
                self.horizontal_distance(a, x)
                    .partial_cmp(&self.horizontal_distance(b, x))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;
        let edge = if down { 0 } else { leaf.info.len };
        let row = leaf.caret_point(edge).y;
        let mut offset = leaf.offset_at(point(x, row + leaf.line_height() / 2.));
        if let Some(text) = doc.leaf_text(leaf.info.block) {
            offset = offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            let on_row =
                |offset| (leaf.caret_point(offset).y - row).abs() < leaf.line_height() / 2.;
            let mut cursor = GraphemeCursor::new(offset, text.len(), true);
            if !cursor.is_boundary(text, 0).unwrap_or(false) || !on_row(offset) {
                // A shaped index can fall inside a grapheme, or at the next
                // row's downstream wrap offset. Pick the closest valid caret
                // that still belongs to the entry row.
                let before = cursor.prev_boundary(text, 0).ok().flatten();
                let after = GraphemeCursor::new(offset, text.len(), true)
                    .next_boundary(text, 0)
                    .ok()
                    .flatten();
                offset = [before, after]
                    .into_iter()
                    .flatten()
                    .filter(|offset| on_row(*offset))
                    .min_by(|a, b| {
                        (leaf.caret_point(*a).x - x)
                            .abs()
                            .partial_cmp(&(leaf.caret_point(*b).x - x).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap_or(edge);
            }
        }
        Some(BlockPosition::new(leaf.info.block, offset))
    }

    fn horizontal_distance(&self, leaf: &LeafGeom, x: Pixels) -> Pixels {
        let bounds = self
            .cell_boxes
            .get(&leaf.info.block)
            .copied()
            .unwrap_or_else(|| leaf.bounds());
        if x < bounds.left() {
            bounds.left() - x
        } else if x > bounds.right() {
            x - bounds.right()
        } else {
            px(0.)
        }
    }

    pub(crate) fn selection_bounds(
        &self,
        doc: &EditorDocument,
        range: ResolvedSelection,
    ) -> Option<Bounds<Pixels>> {
        let viewport = self.viewport?;
        let start = doc.order.rank(range.start.block)?;
        let end = doc.order.rank(range.end.block)?;
        self.leaves
            .iter()
            .filter_map(|leaf| {
                let id = leaf.info.block;
                let rank = doc.order.rank(id)?;
                if rank < start || rank > end {
                    return None;
                }
                let text = doc.leaf_text(id)?;
                let lo = if id == range.start.block {
                    range.start.offset
                } else {
                    0
                };
                let hi = if id == range.end.block {
                    range.end.offset
                } else {
                    text.len()
                };
                leaf.selection_bounds(text, lo..hi, viewport)
            })
            .reduce(|a, b| a.union(&b))
    }

    /// The table under a window point.
    /// Kept for the block handle (not built yet); clicks in a table place the caret.
    #[allow(dead_code)]
    pub(crate) fn table_at(&self, at: Point<Pixels>) -> Option<BlockId> {
        self.tables
            .iter()
            .find(|(_, bounds)| bounds.contains(&at))
            .map(|(id, _)| *id)
    }

    /// Nearest caret position to a window point.
    pub(crate) fn hit_test(&self, at: Point<Pixels>) -> Option<BlockPosition> {
        // Inside a table cell's box (padding and slack included): that
        // cell. Leaf bottoms are not monotonic across a table row whose
        // cells wrap to different heights, so the search below can miss.
        if let Some((cell, _)) = self.cell_boxes.iter().find(|(_, b)| b.contains(&at))
            && let Some(leaf) = self.leaf(*cell)
        {
            return Some(BlockPosition::new(*cell, leaf.offset_at(at)));
        }
        let last = self.leaves.len().checked_sub(1)?;
        let ix = self
            .leaves
            .partition_point(|leaf| leaf.bounds().bottom() <= at.y)
            .min(last);
        let ix = self.nearest_in_band(ix, at.x);
        let leaf = &self.leaves[ix];
        Some(BlockPosition::new(leaf.info.block, leaf.offset_at(at)))
    }

    /// Leaves side by side with `ix` (table cells in one row share a
    /// vertical band): the one horizontally nearest to `x`.
    fn nearest_in_band(&self, ix: usize, x: Pixels) -> usize {
        let band = self.leaves[ix].bounds();
        let overlaps = |i: usize| {
            let b = self.leaves[i].bounds();
            b.top() < band.bottom() && b.bottom() > band.top()
        };
        let mut lo = ix;
        while lo > 0 && overlaps(lo - 1) {
            lo -= 1;
        }
        let mut hi = ix;
        while hi + 1 < self.leaves.len() && overlaps(hi + 1) {
            hi += 1;
        }
        let distance = |i: usize| self.horizontal_distance(&self.leaves[i], x);
        (lo..=hi)
            .min_by(|a, b| {
                distance(*a)
                    .partial_cmp(&distance(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(ix)
    }

    /// One visual line above/below `pos`, keeping `x`.
    pub(crate) fn vertical(
        &self,
        pos: BlockPosition,
        x: Pixels,
        down: bool,
    ) -> Option<BlockPosition> {
        let ix = *self.index.get(&pos.block)?;
        let leaf = &self.leaves[ix];
        let caret = leaf.caret_point(pos.offset);
        let lh = leaf.line_height();
        let y = if down {
            caret.y + lh * 1.5
        } else {
            caret.y - lh * 0.5
        };
        let bounds = leaf.bounds();
        if y >= bounds.top() && y < bounds.bottom() {
            return Some(BlockPosition::new(
                leaf.info.block,
                leaf.offset_at(point(x, y)),
            ));
        }
        // The next leaf above/below that is not side by side with this one
        // (skips the rest of a table row), then the nearest in its band.
        let side_by_side = |i: usize| {
            let b = self.leaves[i].bounds();
            b.top() < bounds.bottom() && b.bottom() > bounds.top()
        };
        let mut j = ix;
        loop {
            j = if down {
                Some(j + 1).filter(|j| *j < self.leaves.len())?
            } else {
                j.checked_sub(1)?
            };
            if !side_by_side(j) {
                break;
            }
        }
        let next = &self.leaves[self.nearest_in_band(j, x)];
        let b = next.bounds();
        let y = if down {
            b.top() + next.line_height() / 2.
        } else {
            b.bottom() - next.line_height() / 2.
        };
        Some(BlockPosition::new(
            next.info.block,
            next.offset_at(point(x, y)),
        ))
    }

    /// Start or end of the visual (soft-wrapped) line holding `pos`.
    pub(crate) fn line_edge(&self, pos: BlockPosition, end: bool) -> Option<BlockPosition> {
        let leaf = self.leaf(pos.block)?;
        let caret = leaf.caret_point(pos.offset);
        let b = leaf.bounds();
        let x = if end {
            b.right() + px(10000.)
        } else {
            b.left() - px(1.)
        };
        let y = caret.y + leaf.line_height() / 2.;
        Some(BlockPosition::new(pos.block, leaf.offset_at(point(x, y))))
    }
}

/// One table's laid-out grid, rebuilt from its cells' text geometry.
#[derive(Clone, Debug)]
pub(crate) struct TableGeom {
    pub(crate) table: BlockId,
    /// Column `(left, right)` edges, slack and divider included.
    pub(crate) cols: Vec<(Pixels, Pixels)>,
    /// Row `(top, bottom)` edges, padding included.
    pub(crate) rows: Vec<(Pixels, Pixels)>,
    /// The table's frame.
    pub(crate) bounds: Bounds<Pixels>,
}

/// A table as the layout pass needs it: cell ids by row and column aligns.
pub(crate) struct TableShape {
    pub(crate) table: BlockId,
    pub(crate) grid: Vec<Vec<BlockId>>,
    pub(crate) aligns: Vec<ColumnAlign>,
}

/// Cell box padding and rule widths of gpui-kit's table (`px_3`, `py(7)`,
/// `border_r_1` between columns, `border_1` frame).
const CELL_MIN_WIDTH: f32 = 48.;
const RULE: f32 = 1.;

impl FrameLayout {
    /// Rebuild each table's column and row boxes from where its cells'
    /// text was laid out (D20 follow-up). gpui-kit lays a column out as
    /// `[lead slack][cell: px_3 + text + px_3][trail slack + divider]`, the
    /// slack split by the column alignment (Left: after, Centre: both
    /// sides, Right: before) and sized in proportion to the cell width.
    /// In a Left / None column the text element of every cell is the
    /// cell's content box. In a Centre / Right column (gpui-kit 0.10+) each
    /// text element shrinks to its text and is placed inside the content
    /// box. Account for the renderer's 48px minimum (divider included),
    /// device-pixel rounding, and any alignment space inside that box before
    /// inferring the proportional slack between columns. Otherwise short
    /// centred/right-aligned text gives incorrect column edges.
    pub(crate) fn layout_tables(
        &mut self,
        shapes: &[TableShape],
        padding: gpui::Edges<Pixels>,
        content_right: Option<Pixels>,
        scale_factor: f32,
    ) {
        self.grids.clear();
        self.cell_boxes.clear();
        for shape in shapes {
            for row in &shape.grid {
                for (c, cell) in row.iter().enumerate() {
                    let align = shape.aligns.get(c).copied().unwrap_or_default();
                    if let Some(ix) = self.index.get(cell) {
                        self.leaves[*ix].align = align;
                    }
                }
            }
            let cols = shape.grid.iter().map(Vec::len).max().unwrap_or(0);
            // Widest text (left, width), padded cell width, and alignment
            // space inside that cell. The renderer rounds up to device pixels.
            let mut elems: Vec<Option<(Pixels, Pixels, Pixels, Pixels)>> = vec![None; cols];
            for row in &shape.grid {
                for (c, cell) in row.iter().enumerate() {
                    if let Some(leaf) = self.leaf(*cell) {
                        let b = leaf.bounds();
                        if elems[c].is_none_or(|(_, w, _, _)| b.size.width > w) {
                            let divider = if c + 1 < cols { px(RULE) } else { px(0.) };
                            let width = (b.size.width + padding.left + padding.right)
                                .max(px(CELL_MIN_WIDTH) - divider);
                            let width = px((f32::from(width) * scale_factor).ceil() / scale_factor);
                            let inner = width - b.size.width - padding.left - padding.right;
                            elems[c] = Some((b.left(), b.size.width, width, inner));
                        }
                    }
                }
            }
            let Some(elems) = elems.into_iter().collect::<Option<Vec<_>>>() else {
                continue;
            };
            if elems.is_empty() {
                continue;
            }
            let share = |c: usize| match shape.aligns.get(c).copied().unwrap_or_default() {
                ColumnAlign::Center => (0.5, 0.5),
                ColumnAlign::Right => (1., 0.),
                _ => (0., 1.),
            };
            let cell_w = |c: usize| elems[c].2;
            let rule = px(RULE);
            // Slack per pixel of cell width, from the first informative gap.
            let mut k: Option<f32> = None;
            for c in 0..cols.saturating_sub(1) {
                let gap = (elems[c + 1].0 - padding.left)
                    - (elems[c].0 + elems[c].1 + padding.right)
                    - rule
                    - elems[c].3 * share(c).1
                    - elems[c + 1].3 * share(c + 1).0;
                let denom =
                    share(c).1 * f32::from(cell_w(c)) + share(c + 1).0 * f32::from(cell_w(c + 1));
                if denom > 0. {
                    k = Some((f32::from(gap) / denom).max(0.));
                    break;
                }
            }
            if k.is_none()
                && let Some(right) = content_right
            {
                let last = cols - 1;
                let gap = (right - rule)
                    - (elems[last].0 + elems[last].1 + padding.right)
                    - elems[last].3 * share(last).1;
                let denom = share(last).1 * f32::from(cell_w(last));
                if denom > 0. {
                    k = Some((f32::from(gap) / denom).max(0.));
                }
            }
            let k = k.unwrap_or(0.);
            let mut col_spans: Vec<(Pixels, Pixels)> = (0..cols)
                .map(|c| {
                    let (lead, trail) = share(c);
                    let slack = cell_w(c) * k + elems[c].3;
                    let left = elems[c].0 - padding.left - slack * lead;
                    let right = elems[c].0 + elems[c].1 + padding.right + slack * trail;
                    let right = if c + 1 < cols { right + rule } else { right };
                    (left, right)
                })
                .collect();
            // A column whose cells are all empty or short and aligned has no
            // text element as wide as its box; meet its neighbours halfway so
            // the columns still tile the table.
            for c in 0..cols.saturating_sub(1) {
                let (right, next_left) = (col_spans[c].1, col_spans[c + 1].0);
                if (right - next_left).abs() > px(0.5) {
                    let mid = (right + next_left) / 2.;
                    col_spans[c].1 = mid;
                    col_spans[c + 1].0 = mid;
                }
            }
            let row_spans: Vec<Option<(Pixels, Pixels)>> = shape
                .grid
                .iter()
                .enumerate()
                .map(|(r, row)| {
                    let bounds: Vec<Bounds<Pixels>> = row
                        .iter()
                        .filter_map(|c| self.leaf(*c).map(|l| l.bounds()))
                        .collect();
                    let top = bounds.iter().map(|b| b.top()).reduce(|a, b| a.min(b))?;
                    let bottom = bounds.iter().map(|b| b.bottom()).reduce(|a, b| a.max(b))?;
                    let last = r + 1 == shape.grid.len();
                    Some((
                        top - padding.top,
                        bottom + padding.bottom + if last { px(0.) } else { rule },
                    ))
                })
                .collect();
            if shape
                .grid
                .iter()
                .zip(&row_spans)
                .any(|(row, span)| !row.is_empty() && span.is_none())
            {
                continue;
            }
            for (r, row) in shape.grid.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    let (l, rt) = col_spans[c];
                    let Some((t, b)) = row_spans[r] else {
                        continue;
                    };
                    self.cell_boxes
                        .insert(*cell, Bounds::from_corners(point(l, t), point(rt, b)));
                }
            }
            let (l, r) = (col_spans[0].0, col_spans[cols - 1].1);
            let Some((t, b)) = row_spans
                .iter()
                .flatten()
                .copied()
                .reduce(|(top, _), (_, bottom)| (top, bottom))
            else {
                continue;
            };
            self.grids.push(TableGeom {
                table: shape.table,
                cols: col_spans,
                rows: row_spans
                    .into_iter()
                    .map(|span| span.unwrap_or((t, t)))
                    .collect(),
                bounds: Bounds::from_corners(point(l - rule, t - rule), point(r + rule, b + rule)),
            });
        }
    }
}
