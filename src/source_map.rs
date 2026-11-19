use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result, bail, ensure};
use markdown::mdast::Node;
use markdown_editor::{
    Affinity, BlockEditorModel, BlockId, BlockKind, BlockPosition, BlockSelection,
};
use unicode_segmentation::UnicodeSegmentation;
use yrs::Assoc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionKind {
    Text,
    Blocks,
    Cells,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionOffsets {
    pub kind: SelectionKind,
    pub anchor: usize,
    pub head: usize,
}

/// An immutable index for one source/model generation. AST spans, never a search
/// for rendered text, determine which occurrence of a character owns a caret.
pub(crate) struct SourceMap {
    len: usize,
    leaves: HashMap<BlockId, Leaf>,
    boundaries: Vec<(usize, BlockPosition)>,
    blocks: HashMap<BlockId, Block>,
}

struct Leaf {
    boundaries: Vec<Boundary>,
}

struct Boundary {
    offset: usize,
    upstream: usize,
    downstream: usize,
}

struct Block {
    span: Range<usize>,
    parent: Option<BlockId>,
    table: Option<BlockId>,
    cell: bool,
}

#[derive(Clone)]
struct Unit {
    text: String,
    span: Range<usize>,
    literal: bool,
}

struct ParsedLeaf {
    span: Range<usize>,
    units: Option<Vec<Unit>>,
    kind: BlockKind,
    empty: usize,
}

#[derive(Clone, Copy)]
enum Container {
    Quote,
    Item(usize),
    Cell,
}

impl SourceMap {
    pub(crate) fn new(source: &str, model: &BlockEditorModel) -> Result<Self> {
        let mut options = markdown::ParseOptions::gfm();
        options.constructs.math_text = true;
        options.constructs.math_flow = true;
        let root = markdown::to_mdast(source, &options)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let mut parsed = Vec::new();
        collect(source, &root, &[], &mut parsed)?;
        let exterior_start = root
            .children()
            .and_then(|children| children.last())
            .and_then(Node::position)
            .map_or(0, |position| position.end.offset);
        if parsed.is_empty() {
            parsed.push(empty_leaf(0));
        }
        let exterior = &source[exterior_start..];
        let trimmed = exterior.trim_end_matches([' ', '\t']);
        let stripped = trimmed.trim_end_matches(['\r', '\n']);
        let endings = &trimmed[stripped.len()..];
        let ends: Vec<_> = endings
            .char_indices()
            .filter(|(_, ch)| *ch == '\n' || (*ch == '\r' && !endings.contains('\n')))
            .map(|(offset, ch)| exterior_start + stripped.len() + offset + ch.len_utf8())
            .collect();
        for pair in ends.chunks(2) {
            parsed.push(empty_leaf(*pair.last().unwrap()));
        }
        // The importer retains terminal typing whitespace in the last inline
        // leaf, including after an empty heading/list marker.
        let tail_start = source.trim_end_matches([' ', '\t']).len();
        let actual: Vec<_> = model.document().leaves().collect();
        // Markdown ASTs omit empty paragraphs before/between content. Native
        // editing retains those boundaries until the block tree is normalized.
        if parsed.len() < actual.len() {
            let mut pending = parsed.into_iter().peekable();
            let mut aligned: Vec<ParsedLeaf> = Vec::with_capacity(actual.len());
            for native in &actual {
                let omitted_empty = native.kind() == BlockKind::Paragraph
                    && native.text() == Some("")
                    && pending.peek().is_none_or(|leaf| {
                        leaf.kind != BlockKind::Paragraph
                            || leaf.units.as_ref().is_none_or(|units| !units.is_empty())
                    });
                if omitted_empty {
                    let previous_end = aligned.last().map_or(0, |leaf| leaf.span.end);
                    let next_start = pending.peek().map_or(source.len(), |leaf| leaf.span.start);
                    let at = if aligned.is_empty() {
                        0
                    } else {
                        previous_end.saturating_add(2).min(next_start)
                    };
                    ensure!(
                        source
                            .get(previous_end..at)
                            .is_some_and(|gap| gap.chars().all(char::is_whitespace)),
                        "Empty native paragraph has no Markdown separator"
                    );
                    aligned.push(empty_leaf(at));
                } else if let Some(leaf) = pending.next() {
                    aligned.push(leaf);
                }
            }
            ensure!(pending.next().is_none(), "Unmapped Markdown leaf");
            parsed = aligned;
        }
        ensure!(
            parsed.len() == actual.len(),
            "Markdown source map has {} leaves but the native document has {}",
            parsed.len(),
            actual.len()
        );
        if tail_start < source.len() {
            if let (Some(last), Some(native)) = (parsed.last_mut(), actual.last()) {
                if let (Some(units), Some(text)) = (&mut last.units, native.text()) {
                    let rendered: String = units.iter().map(|unit| unit.text.as_str()).collect();
                    if text.starts_with(&rendered) && text.len() > rendered.len() {
                        let suffix = &text[rendered.len()..];
                        ensure!(
                            source[tail_start..].ends_with(suffix),
                            "Unmapped terminal whitespace"
                        );
                        let start = source.len() - suffix.len();
                        units.extend(literal_units(source, start..source.len()));
                    }
                }
            }
        }
        let mut map = Self {
            len: source.len(),
            leaves: HashMap::with_capacity(actual.len()),
            boundaries: Vec::new(),
            blocks: HashMap::new(),
        };
        for (parsed, native) in parsed.into_iter().zip(actual) {
            let id = native.id();
            let mut boundaries = Vec::new();
            if let Some(text) = native.text() {
                ensure!(
                    parsed.kind == native.kind(),
                    "Markdown/native block kind mismatch at {:?}",
                    parsed.span
                );
                let mut units = parsed
                    .units
                    .context("Native text block corresponds to an opaque source node")?;
                let mut rendered: String = units.iter().map(|unit| unit.text.as_str()).collect();
                if text.starts_with(&rendered) && text.len() > rendered.len() {
                    let suffix = &text[rendered.len()..];
                    if suffix.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
                        && parsed.span.end >= suffix.len()
                        && source.get(parsed.span.end - suffix.len()..parsed.span.end)
                            == Some(suffix)
                    {
                        units.extend(literal_units(
                            source,
                            parsed.span.end - suffix.len()..parsed.span.end,
                        ));
                        rendered.push_str(suffix);
                    }
                }
                ensure!(
                    rendered == text,
                    "Markdown/native text mismatch at {:?}: {:?} != {:?}",
                    parsed.span,
                    rendered,
                    text
                );
                let graphemes: Vec<_> = text
                    .grapheme_indices(true)
                    .map(|(offset, _)| offset)
                    .chain(std::iter::once(text.len()))
                    .collect();
                let mut offset = 0;
                let mut previous_end = parsed.empty;
                let mut points = Vec::with_capacity(units.len() + 1);
                for unit in &units {
                    points.push(Boundary {
                        offset,
                        upstream: if offset == 0 {
                            unit.span.start
                        } else {
                            previous_end
                        },
                        downstream: unit.span.start,
                    });
                    offset += unit.text.len();
                    previous_end = unit.span.end;
                }
                points.push(Boundary {
                    offset,
                    upstream: previous_end,
                    downstream: previous_end,
                });
                for point in points {
                    if graphemes.binary_search(&point.offset).is_ok() {
                        boundaries.push(point);
                    }
                }
            } else {
                // Registered block plugins (e.g. Mermaid) consume an entire AST
                // block. Their fallback text is not an editable coordinate space.
                boundaries.push(Boundary {
                    offset: 0,
                    upstream: parsed.span.start,
                    downstream: parsed.span.start,
                });
                boundaries.push(Boundary {
                    offset: 1,
                    upstream: parsed.span.end,
                    downstream: parsed.span.end,
                });
            }
            for boundary in &boundaries {
                map.boundaries.push((
                    boundary.upstream,
                    BlockPosition {
                        block: id,
                        offset: boundary.offset,
                        affinity: Affinity::Upstream,
                    },
                ));
                map.boundaries
                    .push((boundary.downstream, BlockPosition::new(id, boundary.offset)));
            }
            map.leaves.insert(id, Leaf { boundaries });
            map.blocks.insert(
                id,
                Block {
                    span: parsed.span,
                    parent: native.parent().map(|block| block.id()),
                    table: table_ancestor(native),
                    cell: native.kind() == BlockKind::TableCell,
                },
            );
        }
        map.add_containers(model.document().root());
        // Stable sorting retains document order for coincident empty boundaries.
        map.boundaries.sort_by_key(|(source, _)| *source);
        Ok(map)
    }

    fn add_containers(&mut self, block: markdown_editor::BlockRef<'_>) -> Option<Range<usize>> {
        if let Some(entry) = self.blocks.get(&block.id()) {
            return Some(entry.span.clone());
        }
        let mut span: Option<Range<usize>> = None;
        for child in block.children() {
            if let Some(child) = self.add_containers(child) {
                span = Some(match span {
                    Some(span) => span.start.min(child.start)..span.end.max(child.end),
                    None => child,
                });
            }
        }
        if let Some(span) = &span {
            self.blocks.insert(
                block.id(),
                Block {
                    span: span.clone(),
                    parent: block.parent().map(|parent| parent.id()),
                    table: table_ancestor(block),
                    cell: false,
                },
            );
        }
        span
    }

    pub(crate) fn source_offset(&self, position: BlockPosition) -> Option<usize> {
        let boundaries = &self.leaves.get(&position.block)?.boundaries;
        let point = &boundaries[boundaries
            .binary_search_by_key(&position.offset, |point| point.offset)
            .ok()?];
        Some(match position.affinity {
            Affinity::Upstream => point.upstream,
            Affinity::Downstream => point.downstream,
        })
    }

    pub(crate) fn position(
        &self,
        source_byte_offset: usize,
        assoc: Assoc,
    ) -> Option<BlockPosition> {
        if source_byte_offset > self.len {
            return None;
        }
        let index = match assoc {
            Assoc::Before => self
                .boundaries
                .partition_point(|(offset, _)| *offset <= source_byte_offset)
                .checked_sub(1)
                .unwrap_or(0),
            Assoc::After => self
                .boundaries
                .partition_point(|(offset, _)| *offset < source_byte_offset)
                .min(self.boundaries.len().checked_sub(1)?),
        };
        self.boundaries.get(index).map(|(_, position)| *position)
    }

    pub(crate) fn selection_offsets(&self, selection: &BlockSelection) -> Option<SelectionOffsets> {
        let (kind, anchor, head) = match selection {
            BlockSelection::Text { anchor, head } => (
                SelectionKind::Text,
                self.source_offset(*anchor)?,
                self.source_offset(*head)?,
            ),
            BlockSelection::Blocks { anchor, head } => {
                let a = self.blocks.get(anchor)?;
                let b = self.blocks.get(head)?;
                if a.parent.is_none() || a.parent != b.parent {
                    return None;
                }
                if a.span.start <= b.span.start {
                    (SelectionKind::Blocks, a.span.start, b.span.end)
                } else {
                    (SelectionKind::Blocks, a.span.end, b.span.start)
                }
            }
            BlockSelection::Cells { anchor, head } => {
                let a = self.blocks.get(anchor)?;
                let b = self.blocks.get(head)?;
                if !a.cell || !b.cell || a.table.is_none() || a.table != b.table {
                    return None;
                }
                (SelectionKind::Cells, a.span.start, b.span.start)
            }
        };
        Some(SelectionOffsets { kind, anchor, head })
    }

    pub(crate) fn restore_selection(&self, offsets: SelectionOffsets) -> Option<BlockSelection> {
        let reversed = offsets.anchor > offsets.head;
        let cells = offsets.kind == SelectionKind::Cells;
        let anchor = self.position(
            offsets.anchor,
            if reversed && !cells {
                Assoc::Before
            } else {
                Assoc::After
            },
        )?;
        let head = if offsets.anchor == offsets.head {
            anchor
        } else {
            self.position(
                offsets.head,
                if reversed || cells {
                    Assoc::After
                } else {
                    Assoc::Before
                },
            )?
        };
        match offsets.kind {
            SelectionKind::Cells => {
                let a = self.blocks.get(&anchor.block)?;
                let b = self.blocks.get(&head.block)?;
                if a.cell && b.cell && a.table.is_some() && a.table == b.table {
                    return Some(BlockSelection::Cells {
                        anchor: anchor.block,
                        head: head.block,
                    });
                }
            }
            SelectionKind::Blocks => {
                if let Some((anchor, head)) = self.sibling_blocks(anchor.block, head.block) {
                    return Some(BlockSelection::Blocks { anchor, head });
                }
            }
            SelectionKind::Text => {}
        }
        Some(BlockSelection::Text { anchor, head })
    }

    fn sibling_blocks(&self, anchor: BlockId, head: BlockId) -> Option<(BlockId, BlockId)> {
        let mut a = anchor;
        loop {
            let parent = self.blocks.get(&a)?.parent?;
            let mut b = head;
            while let Some(b_parent) = self.blocks.get(&b)?.parent {
                if parent == b_parent {
                    return Some((a, b));
                }
                b = b_parent;
            }
            a = parent;
        }
    }
}

fn table_ancestor(mut block: markdown_editor::BlockRef<'_>) -> Option<BlockId> {
    loop {
        if block.kind() == BlockKind::Table {
            return Some(block.id());
        }
        block = block.parent()?;
    }
}

fn span(node: &Node) -> Result<Range<usize>> {
    let position = node
        .position()
        .context("Markdown AST node has no source position")?;
    Ok(position.start.offset..position.end.offset)
}

fn empty_leaf(at: usize) -> ParsedLeaf {
    ParsedLeaf {
        span: at..at,
        units: Some(Vec::new()),
        kind: BlockKind::Paragraph,
        empty: at,
    }
}

fn collect(
    source: &str,
    node: &Node,
    containers: &[Container],
    leaves: &mut Vec<ParsedLeaf>,
) -> Result<()> {
    let range = span(node)?;
    match node {
        Node::Root(_) | Node::List(_) | Node::TableRow(_) => {
            for child in node.children().unwrap() {
                collect(source, child, containers, leaves)?;
            }
        }
        Node::Table(table) => {
            for row in &table.children {
                let Node::TableRow(row) = row else {
                    bail!("Invalid table row");
                };
                for cell in row.children.iter().take(table.align.len()) {
                    collect(source, cell, containers, leaves)?;
                }
            }
        }
        Node::Blockquote(_) | Node::ListItem(_) => {
            let mut nested = containers.to_vec();
            nested.push(if matches!(node, Node::Blockquote(_)) {
                Container::Quote
            } else {
                Container::Item(item_indent(&source[range.clone()]))
            });
            let before = leaves.len();
            if matches!(node, Node::ListItem(_))
                && node
                    .children()
                    .and_then(|children| children.first())
                    .is_some_and(|child| {
                        matches!(child, Node::List(_) | Node::Blockquote(_) | Node::Table(_))
                    })
            {
                // Native normalization gives a container-leading item its own
                // editable paragraph before that first nested container.
                leaves.push(empty_leaf(
                    (range.start + item_indent(&source[range.clone()])).min(range.end),
                ));
            }
            for child in node.children().unwrap() {
                collect(source, child, &nested, leaves)?;
            }
            if before == leaves.len() && matches!(node, Node::ListItem(_)) {
                leaves.push(empty_leaf(range.end));
            }
        }
        Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_) => {
            let mut units = Vec::new();
            let mut inline_containers = containers.to_vec();
            if matches!(node, Node::TableCell(_)) {
                inline_containers.push(Container::Cell);
            }
            for child in node.children().unwrap() {
                inline(source, child, &inline_containers, &mut units)?;
            }
            normalize(&mut units);
            let kind = match node {
                Node::Heading(heading) => BlockKind::Heading {
                    level: heading.depth,
                },
                Node::TableCell(_) => BlockKind::TableCell,
                _ => BlockKind::Paragraph,
            };
            let empty = if matches!(node, Node::Heading(_)) {
                range.end
            } else {
                range.start
            };
            leaves.push(ParsedLeaf {
                span: range,
                units: Some(units),
                kind,
                empty,
            });
        }
        Node::Code(code) => {
            let units = code_units(source, range.clone(), &code.value, containers)?;
            let empty = units.first().map_or_else(
                || {
                    source[range.clone()]
                        .find('\n')
                        .map_or(range.end, |i| range.start + i + 1)
                },
                |unit| unit.span.start,
            );
            leaves.push(ParsedLeaf {
                span: range,
                units: Some(units),
                kind: BlockKind::CodeBlock,
                empty,
            });
        }
        Node::Math(math) => {
            let units = code_units(source, range.clone(), &math.value, containers)?;
            let empty = units.first().map_or(range.start, |unit| unit.span.start);
            leaves.push(ParsedLeaf {
                span: range,
                units: Some(units),
                kind: BlockKind::CodeBlock,
                empty,
            });
        }
        Node::Html(html) if known_html(&html.value) => {
            let units = html_units(source, range.clone(), containers)?;
            let kind = if units.iter().all(|unit| unit.text == "\n") {
                BlockKind::Break
            } else {
                BlockKind::Paragraph
            };
            leaves.push(ParsedLeaf {
                span: range.clone(),
                units: if kind == BlockKind::Break {
                    None
                } else {
                    Some(units)
                },
                kind,
                empty: range.start,
            });
        }
        Node::Definition(_)
        | Node::ThematicBreak(_)
        | Node::FootnoteDefinition(_)
        | Node::Html(_) => {
            leaves.push(ParsedLeaf {
                span: range.clone(),
                units: None,
                kind: BlockKind::Custom,
                empty: range.start,
            });
        }
        _ => bail!("Unsupported Markdown block in source map: {node:?}"),
    }
    Ok(())
}

fn inline(
    source: &str,
    node: &Node,
    containers: &[Container],
    units: &mut Vec<Unit>,
) -> Result<()> {
    let range = span(node)?;
    match node {
        Node::Text(text) => {
            let mut decoded = decode_units(source, range.clone(), containers, true)?;
            let expected = folded(&text.value);
            // Escaped punctuation positions can begin after the escape itself.
            if range.start > 0 && source.as_bytes()[range.start - 1] == b'\\' {
                if let Some(first) = decoded.first_mut() {
                    if first
                        .text
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_punctuation())
                    {
                        first.span.start -= 1;
                    }
                }
            }
            fold_units(&mut decoded);
            let actual: String = decoded.iter().map(|unit| unit.text.as_str()).collect();
            ensure!(
                actual == expected,
                "Decoded Markdown text does not match AST at {range:?}: {actual:?} != {expected:?}"
            );
            units.extend(decoded);
        }
        Node::InlineCode(code) => {
            let raw = &source[range.clone()];
            let ticks = raw.bytes().take_while(|byte| *byte == b'`').count();
            ensure!(
                ticks > 0 && raw.len() >= ticks * 2,
                "Invalid inline code span"
            );
            let mut decoded = decode_units(
                source,
                range.start + ticks..range.end - ticks,
                containers,
                false,
            )?;
            if containers
                .iter()
                .any(|container| matches!(container, Container::Cell))
            {
                let mut unescaped: Vec<Unit> = Vec::with_capacity(decoded.len());
                for mut unit in decoded {
                    if unit.text == "|"
                        && unescaped
                            .last()
                            .is_some_and(|previous| previous.text == "\\")
                    {
                        unit.span.start = unescaped.pop().unwrap().span.start;
                    }
                    unescaped.push(unit);
                }
                decoded = unescaped;
            }
            for unit in &mut decoded {
                if unit.text == "\n" || unit.text == "\r" || unit.text == "\r\n" {
                    unit.text = " ".into();
                }
                unit.literal = true;
            }
            let raw_text: String = decoded.iter().map(|unit| unit.text.as_str()).collect();
            if raw_text.starts_with(' ')
                && raw_text.ends_with(' ')
                && raw_text.chars().any(|ch| ch != ' ')
            {
                decoded.remove(0);
                decoded.pop();
            }
            ensure!(
                decoded
                    .iter()
                    .map(|unit| unit.text.as_str())
                    .collect::<String>()
                    == code.value,
                "Inline code/source mismatch at {range:?}"
            );
            units.extend(decoded);
        }
        Node::InlineMath(_) => {
            let raw = &source[range.clone()];
            let ast = markdown::to_mdast(raw, &markdown::ParseOptions::gfm())
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if let Some(Node::Paragraph(paragraph)) =
                ast.children().and_then(|children| children.first())
                && paragraph
                    .children
                    .iter()
                    .any(|child| !matches!(child, Node::Text(_)))
            {
                // Native parsing reinterprets unclaimed math containing prose
                // markup, but keeps a plain dollar expression verbatim.
                for child in &paragraph.children {
                    let mut child = child.clone();
                    shift_offsets(&mut child, range.start);
                    inline(source, &child, containers, units)?;
                }
            } else {
                units.extend(literal_units(source, range));
            }
        }
        Node::Image(_) | Node::ImageReference(_) | Node::FootnoteReference(_) => {
            units.push(Unit {
                text: "\u{fffc}".into(),
                span: range,
                literal: true,
            });
        }
        Node::Break(_) => units.push(Unit {
            text: "\n".into(),
            span: range,
            literal: true,
        }),
        Node::Html(html) => {
            if known_html(&html.value) {
                if html.value.trim().to_ascii_lowercase().starts_with("<br") {
                    units.push(Unit {
                        text: "\n".into(),
                        span: range,
                        literal: true,
                    });
                }
            } else {
                units.push(Unit {
                    text: "\u{fffc}".into(),
                    span: range,
                    literal: true,
                });
            }
        }
        _ if node.children().is_some() => {
            for child in node.children().unwrap() {
                inline(source, child, containers, units)?;
            }
        }
        _ => bail!("Unsupported Markdown inline node in source map: {node:?}"),
    }
    Ok(())
}

fn shift_offsets(node: &mut Node, offset: usize) {
    if let Some(position) = node.position_mut() {
        position.start.offset += offset;
        position.end.offset += offset;
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            shift_offsets(child, offset);
        }
    }
}

fn item_indent(raw: &str) -> usize {
    let marker = if raw.starts_with(['-', '+', '*']) {
        1
    } else {
        raw.bytes().take_while(u8::is_ascii_digit).count() + 1
    };
    let spaces = raw
        .as_bytes()
        .get(marker..)
        .unwrap_or_default()
        .iter()
        .take_while(|byte| matches!(**byte, b' ' | b'\t'))
        .count();
    marker + if (1..=4).contains(&spaces) { spaces } else { 1 }
}

fn continuation(source: &str, mut at: usize, end: usize, containers: &[Container]) -> usize {
    for container in containers {
        match container {
            Container::Quote => {
                let mut cursor = at;
                while cursor < end && cursor - at < 3 && source.as_bytes()[cursor] == b' ' {
                    cursor += 1;
                }
                if source.as_bytes().get(cursor) == Some(&b'>') {
                    at = cursor + 1;
                    if matches!(source.as_bytes().get(at), Some(b' ' | b'\t')) {
                        at += 1;
                    }
                }
            }
            Container::Item(width) => {
                let mut cursor = at;
                let mut columns = 0;
                while cursor < end && columns < *width {
                    match source.as_bytes()[cursor] {
                        b' ' => columns += 1,
                        b'\t' => columns += 4 - columns % 4,
                        _ => break,
                    }
                    cursor += 1;
                }
                if columns >= *width {
                    at = cursor;
                }
            }
            Container::Cell => {}
        }
    }
    at
}

fn literal_units(source: &str, range: Range<usize>) -> Vec<Unit> {
    source[range.clone()]
        .char_indices()
        .map(|(offset, ch)| Unit {
            text: ch.to_string(),
            span: range.start + offset..range.start + offset + ch.len_utf8(),
            literal: true,
        })
        .collect()
}

/// Decode tokens sequentially inside an AST-owned span. There is deliberately
/// no "find the next matching character" recovery path: a mismatch is an error.
fn decode_units(
    source: &str,
    range: Range<usize>,
    containers: &[Container],
    entities: bool,
) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    let mut at = range.start;
    while at < range.end {
        let start = at;
        let ch = source[at..range.end].chars().next().unwrap();
        at += ch.len_utf8();
        let mut text = ch.to_string();
        if entities && ch == '\\' {
            if let Some(next) = source[at..range.end]
                .chars()
                .next()
                .filter(|ch| ch.is_ascii_punctuation())
            {
                at += next.len_utf8();
                text = next.to_string();
            }
        } else if entities && ch == '&' {
            let suffix = &source[at..range.end];
            let count = suffix
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'#')
                .count();
            if suffix.as_bytes().get(count) == Some(&b';') {
                let end = at + count + 1;
                let raw = &source[start..end];
                let ast = markdown::to_mdast(raw, &markdown::ParseOptions::gfm())
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                if let Some(Node::Paragraph(paragraph)) =
                    ast.children().and_then(|children| children.first())
                {
                    if let [Node::Text(decoded)] = paragraph.children.as_slice() {
                        if decoded.value != raw {
                            text = decoded.value.clone();
                            at = end;
                        }
                    }
                }
            }
        } else if ch == '\r' && source.as_bytes().get(at) == Some(&b'\n') && at < range.end {
            at += 1;
            text = "\r\n".into();
        }
        let count = text.chars().count();
        if count > 1 && !matches!(text.as_str(), "\r\n") {
            // Named entities may expand to two independently editable
            // graphemes (e.g. &fjlig;). Give their internal boundary a stable
            // byte inside the ASCII token, rather than collapsing both carets
            // to the entity's opening ampersand.
            for (index, ch) in text.chars().enumerate() {
                units.push(Unit {
                    text: ch.to_string(),
                    span: start + (at - start) * index / count
                        ..start + (at - start) * (index + 1) / count,
                    literal: !entities,
                });
            }
        } else {
            units.push(Unit {
                text,
                span: start..at,
                literal: !entities,
            });
        }
        if matches!(ch, '\r' | '\n') {
            at = continuation(source, at, range.end, containers);
        }
    }
    Ok(units)
}

fn folded(text: &str) -> String {
    let mut result = String::new();
    let mut white = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\r' | '\n') {
            if !white {
                result.push(' ');
            }
            white = true;
        } else {
            result.push(ch);
            white = false;
        }
    }
    result
}

fn fold_units(units: &mut Vec<Unit>) {
    let mut folded_units: Vec<Unit> = Vec::with_capacity(units.len());
    for mut unit in units.drain(..) {
        if unit
            .text
            .chars()
            .all(|ch| matches!(ch, ' ' | '\t' | '\r' | '\n'))
        {
            unit.text = " ".into();
            if let Some(previous) = folded_units
                .last_mut()
                .filter(|previous| previous.text == " ")
            {
                previous.span.end = unit.span.end;
                continue;
            }
        }
        folded_units.push(unit);
    }
    *units = folded_units;
}

fn normalize(units: &mut Vec<Unit>) {
    let mut line_start = true;
    let mut previous_white = false;
    units.retain(|unit| {
        if unit.literal {
            line_start = unit.text == "\n";
            previous_white = unit.text.ends_with([' ', '\t']);
            return true;
        }
        if unit.text == " " && (line_start || previous_white) {
            return false;
        }
        line_start = false;
        previous_white = unit.text == " ";
        true
    });
}

fn known_html(raw: &str) -> bool {
    let tag = raw
        .trim_start()
        .trim_start_matches('<')
        .trim_start_matches('/')
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "u" | "mark" | "em" | "strong" | "s" | "del" | "br"
    )
}

fn html_units(source: &str, range: Range<usize>, containers: &[Container]) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    let mut at = range.start;
    while at < range.end {
        if source.as_bytes()[at] == b'<' {
            let mut end = at + 1;
            let mut quote = None;
            while end < range.end {
                let byte = source.as_bytes()[end];
                if let Some(delimiter) = quote {
                    if byte == delimiter {
                        quote = None;
                    }
                } else if matches!(byte, b'\'' | b'"') {
                    quote = Some(byte);
                } else if byte == b'>' {
                    break;
                }
                end += 1;
            }
            ensure!(end < range.end, "Unterminated HTML tag in source map");
            if source[at..=end].to_ascii_lowercase().starts_with("<br") {
                units.push(Unit {
                    text: "\n".into(),
                    span: at..end + 1,
                    literal: true,
                });
            }
            at = end + 1;
        } else {
            let end = source[at..range.end]
                .find('<')
                .map_or(range.end, |relative| at + relative);
            let mut decoded = decode_units(source, at..end, containers, true)?;
            fold_units(&mut decoded);
            units.extend(decoded);
            at = end;
        }
    }
    normalize(&mut units);
    Ok(units)
}

fn code_units(
    source: &str,
    range: Range<usize>,
    expected: &str,
    containers: &[Container],
) -> Result<Vec<Unit>> {
    let raw = &source[range.clone()];
    let trimmed = raw.trim_start_matches([' ', '\t']);
    let marker = trimmed.as_bytes().first().copied();
    let fence_len = marker.map_or(0, |marker| {
        trimmed.bytes().take_while(|byte| *byte == marker).count()
    });
    let fenced = matches!(marker, Some(b'`' | b'~' | b'$'))
        && fence_len >= if marker == Some(b'$') { 2 } else { 3 };
    let mut at = if fenced {
        raw.find('\n')
            .map_or(range.end, |offset| range.start + offset + 1)
    } else {
        range.start
    };
    let indent = if fenced { raw.len() - trimmed.len() } else { 4 };
    let mut units = Vec::new();
    while at < range.end {
        let line_start = if at == range.start {
            at
        } else {
            continuation(source, at, range.end, containers)
        };
        let line_end = source[line_start..range.end]
            .find('\n')
            .map_or(range.end, |offset| line_start + offset + 1);
        let line = &source[line_start..line_end];
        if fenced {
            let closing = line.trim();
            if closing.len() >= fence_len && closing.bytes().all(|byte| Some(byte) == marker) {
                break;
            }
        }
        let mut content_start = line_start;
        let mut columns = 0;
        while content_start < line_end && columns < indent {
            match source.as_bytes()[content_start] {
                b' ' => columns += 1,
                b'\t' => columns += 4 - columns % 4,
                _ => break,
            }
            content_start += 1;
        }
        if columns > indent {
            units.push(Unit {
                text: " ".repeat(columns - indent),
                span: content_start - 1..content_start,
                literal: true,
            });
        }
        units.extend(decode_units(source, content_start..line_end, &[], false)?);
        at = line_end;
    }
    // mdast excludes the body's final line ending, not all blank lines.
    if let Some(last) = units.last() {
        if matches!(last.text.as_str(), "\n" | "\r" | "\r\n") {
            units.pop();
        }
    }
    let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
    if text != expected {
        // CommonMark normalizes CRLF in code values on parser versions that
        // expose LF-only values; the raw token still owns the complete CRLF.
        for unit in &mut units {
            if unit.text == "\r\n" || unit.text == "\r" {
                unit.text = "\n".into();
            }
        }
    }
    ensure!(
        units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>()
            == expected,
        "Code/source mismatch at {range:?}"
    );
    Ok(units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::base::text::{
        MarkdownExtensions, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    };

    fn model(source: &str) -> BlockEditorModel {
        BlockEditorModel::from_markdown(source, MarkdownExtensions::default()).unwrap()
    }

    fn assert_round_trips(source: &str) {
        let mut model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        let positions: Vec<_> = model
            .document()
            .leaves()
            .flat_map(|block| {
                let offsets = match block.text() {
                    Some(text) => text
                        .grapheme_indices(true)
                        .map(|(offset, _)| offset)
                        .chain(std::iter::once(text.len()))
                        .collect::<Vec<_>>(),
                    None => vec![0, 1],
                };
                offsets
                    .into_iter()
                    .map(move |offset| BlockPosition::new(block.id(), offset))
            })
            .collect();
        for position in positions {
            let offset = map.source_offset(position).unwrap();
            assert!(source.is_char_boundary(offset));
            let selection = map
                .restore_selection(SelectionOffsets {
                    kind: SelectionKind::Text,
                    anchor: offset,
                    head: offset,
                })
                .unwrap();
            let BlockSelection::Text { anchor, head } = &selection else {
                panic!("Lost caret")
            };
            assert_eq!(
                (anchor.block, anchor.offset),
                (position.block, position.offset)
            );
            assert_eq!((head.block, head.offset), (position.block, position.offset));
            model.set_selection(selection).unwrap();
            assert!(model.selection().is_collapsed());
        }
    }

    #[test]
    fn repeated_content_uses_ast_source_spans() {
        let source = "# same\n\nsame\n\nsame";
        let model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        let leaves: Vec<_> = model.document().leaves().collect();
        for (leaf, (source_offset, _)) in leaves.iter().zip(source.match_indices("same")) {
            assert_eq!(
                map.source_offset(BlockPosition::new(leaf.id(), 0)),
                Some(source_offset)
            );
        }
    }

    #[test]
    fn unicode_marks_and_grapheme_boundaries() {
        let source = "# e\u{301}🙂 **加**";
        let model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        let leaf = model.document().leaves().next().unwrap();
        assert_eq!(leaf.text(), Some("e\u{301}🙂 加"));
        assert_eq!(
            map.source_offset(BlockPosition::new(leaf.id(), 8)),
            Some(12)
        );
        assert_eq!(
            map.source_offset(BlockPosition::new(leaf.id(), 11)),
            Some(15)
        );
        assert_eq!(map.source_offset(BlockPosition::new(leaf.id(), 1)), None);
        for assoc in [Assoc::Before, Assoc::After] {
            let position = map.position(4, assoc).unwrap();
            assert!([0, 3].contains(&position.offset));
        }
    }

    #[test]
    fn links_entities_and_escapes_have_raw_coordinates() {
        let source = "A &amp; \\* [same](https://same) &fjlig;";
        let model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        let leaf = model.document().leaves().next().unwrap();
        assert_eq!(leaf.text(), Some("A & * same fj"));
        assert_eq!(map.source_offset(BlockPosition::new(leaf.id(), 2)), Some(2));
        assert_eq!(
            map.source_offset(BlockPosition::new(leaf.id(), 4)),
            Some(source.find("\\*").unwrap())
        );
        assert_eq!(
            map.source_offset(BlockPosition::new(leaf.id(), 6)),
            Some(source.find("[same]").unwrap() + 1)
        );
        assert_eq!(map.position(4, Assoc::Before).unwrap().offset, 2);
        assert_eq!(map.position(4, Assoc::After).unwrap().offset, 3);
        assert_round_trips(source);
        assert_round_trips("$a  b$ and $**same**$");
    }

    #[test]
    fn native_mark_html_and_opaque_html_are_preserved() {
        for source in [
            "<mark>x</mark> <u>same</u> <strong>same</strong> <em>same</em> <s>same</s>",
            "<strong>same &amp; same</strong>",
            "a <code>x</code> <span data-x=\"same\">same</span>",
            "<div>same</div>",
        ] {
            assert_round_trips(source);
        }
    }

    #[test]
    fn lists_tasks_tables_images_and_breaks() {
        for source in [
            "- [x] same\n- [ ] **same**\n  - same",
            "> same\n> same\n>\n> - same",
            "-\n- #\n- > nested",
            "| same | **same** |\n| --- | --- |\n| same | ![same](same.png) |",
            "a ![same](same.png) b",
            "a\nb  \nc\\\nd",
            "a<br> b",
            "a  \r\nb",
            "a  \t b **  same**",
        ] {
            assert_round_trips(source);
        }
    }

    #[test]
    fn fenced_indented_and_nested_code() {
        for source in [
            "```rust\nsame\nsame🙂\n```",
            "~~~\n\nsame\n\n~~~",
            "```\nsame\n",
            "    same\n    same",
            "\tsame\n\tsame",
            "> ```\n> same\n> ```",
            "- ```\n  same\n  ```",
            "same ` same ` and `` `same` ``",
            "```\r\nsame\r\nsame\r\n```",
            "```\n```",
        ] {
            assert_round_trips(source);
        }
    }

    #[test]
    fn mermaid_custom_block_is_an_atomic_source_range() {
        struct Mermaid;
        impl MarkdownPlugin for Mermaid {
            fn name(&self) -> &str {
                "source-map-mermaid-test"
            }
            fn is_block(&self) -> bool {
                true
            }
            fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
                let Node::Code(code) = node else {
                    return None;
                };
                (code.lang.as_deref() == Some("mermaid")).then(|| {
                    MarkdownNode::new(self.name(), ())
                        .text(code.value.clone())
                        .markdown(cx.node_source(node).unwrap().to_owned())
                })
            }
        }
        let source = "```mermaid\ngraph TD; same-->same\n```";
        let model =
            BlockEditorModel::from_markdown(source, MarkdownExtensions::default().plugin(Mermaid))
                .unwrap();
        let map = SourceMap::new(source, &model).unwrap();
        let block = model.document().leaves().next().unwrap();
        assert_eq!(block.kind(), BlockKind::Custom);
        assert_eq!(
            map.source_offset(BlockPosition::new(block.id(), 0)),
            Some(0)
        );
        assert_eq!(
            map.source_offset(BlockPosition::new(block.id(), 1)),
            Some(source.len())
        );
        assert_eq!(map.position(20, Assoc::Before).unwrap().offset, 0);
        assert_eq!(map.position(20, Assoc::After).unwrap().offset, 1);
    }

    #[test]
    fn empty_and_terminal_typing_boundaries() {
        for source in [
            "",
            "\n",
            "\n\n",
            "same\n",
            "same\n\n",
            "same\n\n\n",
            "same  ",
            "same\n\n  ",
            "# ",
            "- ",
        ] {
            assert_round_trips(source);
        }
        let source = "same\n\n";
        let model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        assert_eq!(
            map.source_offset(model.document().last_caret()),
            Some(source.len())
        );
    }

    #[test]
    fn reversed_text_ranges_survive_source_round_trip() {
        let source = "# same\n\nsame";
        let mut model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        let selection = BlockSelection::Text {
            anchor: model.document().last_caret(),
            head: model.document().first_caret(),
        };
        let offsets = map.selection_offsets(&selection).unwrap();
        assert!(offsets.anchor > offsets.head);
        let restored = map.restore_selection(offsets).unwrap();
        let BlockSelection::Text { anchor, head } = &restored else {
            panic!("Lost text selection")
        };
        assert_eq!(anchor.block, model.document().last_caret().block);
        assert_eq!(head.block, model.document().first_caret().block);
        model.set_selection(restored).unwrap();
    }

    #[test]
    fn block_and_cell_variants_and_direction_survive() {
        for source in [
            "same\n\nsame",
            "- same\n- same",
            "| same | same |\n| --- | --- |\n| same | same |",
        ] {
            let mut model = model(source);
            let map = SourceMap::new(source, &model).unwrap();
            let leaves: Vec<_> = model.document().leaves().collect();
            let selection = if leaves[0].kind() == BlockKind::TableCell {
                BlockSelection::Cells {
                    anchor: leaves.last().unwrap().id(),
                    head: leaves[0].id(),
                }
            } else if matches!(
                leaves[0].parent().unwrap().kind(),
                BlockKind::ListItem { .. }
            ) {
                BlockSelection::Blocks {
                    anchor: leaves.last().unwrap().parent().unwrap().id(),
                    head: leaves[0].parent().unwrap().id(),
                }
            } else {
                BlockSelection::Blocks {
                    anchor: leaves.last().unwrap().id(),
                    head: leaves[0].id(),
                }
            };
            let offsets = map.selection_offsets(&selection).unwrap();
            assert!(offsets.anchor > offsets.head);
            let restored = map.restore_selection(offsets).unwrap();
            assert_eq!(map.selection_offsets(&restored), Some(offsets));
            model.set_selection(restored).unwrap();
        }
    }

    #[test]
    fn removed_structure_uses_association_and_safe_text_fallback() {
        let source = "a\n\n# b";
        let mut model = model(source);
        let map = SourceMap::new(source, &model).unwrap();
        assert_eq!(
            map.position(3, Assoc::Before).unwrap().block,
            model.document().first_caret().block
        );
        assert_eq!(
            map.position(3, Assoc::After).unwrap().block,
            model.document().last_caret().block
        );
        let restored = map
            .restore_selection(SelectionOffsets {
                kind: SelectionKind::Cells,
                anchor: 0,
                head: source.len(),
            })
            .unwrap();
        assert!(matches!(restored, BlockSelection::Text { .. }));
        model.set_selection(restored).unwrap();
        assert_eq!(map.position(source.len() + 1, Assoc::After), None);
    }

    #[test]
    fn mismatched_source_is_an_error_not_the_first_caret() {
        let model = model("same\n\nsame");
        assert!(SourceMap::new("different\n\nsame", &model).is_err());
    }
}
