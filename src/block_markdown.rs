use std::ops::Range;

use anyhow::{Context, Result};
use markdown::mdast::Node;

use crate::block::{BlockSpec, SectionKind};

/// A source projection, retained only while a native edit is being imported.
/// Every copied interval comes from an AST position, including repeated text.
pub(crate) struct ParsedBlocks {
    pub blocks: Vec<BlockSpec>,
    locations: Vec<Location>,
    boundaries: Vec<usize>,
}

struct Location {
    path: Vec<usize>,
    span: Range<usize>,
    segments: Vec<Segment>,
}

struct Segment {
    source: Range<usize>,
    output: usize,
    output_end: usize,
}

#[derive(Default)]
struct Body {
    markdown: String,
    segments: Vec<Segment>,
}

impl Body {
    fn copy(&mut self, source: &str, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        self.segments.push(Segment {
            source: range.clone(),
            output: self.markdown.len(),
            output_end: self.markdown.len() + range.len(),
        });
        self.markdown.push_str(&source[range]);
    }

    fn encoded_space(&mut self, at: usize, byte: u8) {
        let entity = if byte == b'\t' { "&#9;" } else { "&#32;" };
        self.segments.push(Segment {
            source: at..at + 1,
            output: self.markdown.len(),
            output_end: self.markdown.len() + entity.len(),
        });
        self.markdown.push_str(entity);
    }
}

struct Draft {
    kind: SectionKind,
    body: Body,
    span: Range<usize>,
    children: Vec<Draft>,
}

impl Draft {
    fn empty(kind: SectionKind, span: Range<usize>) -> Self {
        Self {
            kind,
            body: Body::default(),
            span,
            children: Vec::new(),
        }
    }

    fn finish(self, path: Vec<usize>, locations: &mut Vec<Location>) -> BlockSpec {
        if self.kind.is_editable() {
            locations.push(Location {
                path: path.clone(),
                span: self.span,
                segments: self.body.segments,
            });
        }
        let children = self
            .children
            .into_iter()
            .enumerate()
            .map(|(index, child)| {
                let mut nested = path.clone();
                nested.push(index);
                child.finish(nested, locations)
            })
            .collect();
        BlockSpec {
            kind: self.kind,
            markdown: self.body.markdown,
            children,
        }
    }
}

#[derive(Clone, Copy)]
enum Container {
    Quote,
    Item(usize),
}

struct Wrapper {
    content: Range<usize>,
    opening: Range<usize>,
    closing: Range<usize>,
}

impl ParsedBlocks {
    /// Return a byte offset in a standalone editable block's Markdown. Syntax
    /// removed by deindentation associates with the following body's boundary.
    pub fn locate(&self, source_byte_offset: usize) -> Option<(Vec<usize>, usize)> {
        let index = self
            .boundaries
            .partition_point(|offset| *offset <= source_byte_offset);
        let at = self.boundaries[index.saturating_sub(1)];
        let location = self.locations.iter().min_by_key(|location| {
            let distance = if at < location.span.start {
                location.span.start - at
            } else {
                at.saturating_sub(location.span.end)
            };
            // At a shared boundary prefer the new block, not the preceding one.
            (
                distance,
                usize::from(at >= location.span.end && at != location.span.start),
            )
        })?;
        let offset = location
            .segments
            .iter()
            .min_by_key(|segment| {
                let distance = if at < segment.source.start {
                    segment.source.start - at
                } else {
                    at.saturating_sub(segment.source.end)
                };
                (distance, usize::from(at >= segment.source.end))
            })
            .map_or(0, |segment| {
                if at >= segment.source.end {
                    segment.output_end
                } else if at <= segment.source.start {
                    segment.output
                } else {
                    segment.output + at - segment.source.start
                }
            });
        Some((location.path.clone(), offset))
    }
}

pub(crate) fn parse_blocks(source: &str) -> Result<ParsedBlocks> {
    let mut options = markdown::ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let root =
        markdown::to_mdast(source, &options).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut drafts = collect_children(source, &root, &[])?;
    let exterior_start = root
        .children()
        .and_then(|children| children.last())
        .and_then(Node::position)
        .map_or(0, |position| position.end.offset);
    if drafts.is_empty() {
        drafts.push(Draft::empty(SectionKind::Text, 0..source.len()));
    }
    // Native Markdown uses a pair of newlines between paragraphs, and retains
    // an unmatched terminal newline as an empty typing paragraph as well.
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
        let end = *pair.last().unwrap();
        drafts.push(Draft::empty(SectionKind::Text, end..end));
    }
    preserve_reference_context(&root, &mut drafts)?;
    let mut locations = Vec::new();
    let blocks = drafts
        .into_iter()
        .enumerate()
        .map(|(index, draft)| draft.finish(vec![index], &mut locations))
        .collect();
    Ok(ParsedBlocks {
        blocks,
        locations,
        boundaries: source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .collect(),
    })
}

fn span(node: &Node) -> Result<Range<usize>> {
    let position = node
        .position()
        .context("Markdown AST node has no source position")?;
    Ok(position.start.offset..position.end.offset)
}

fn collect_children(source: &str, node: &Node, containers: &[Container]) -> Result<Vec<Draft>> {
    let mut blocks = Vec::new();
    let mut previous_end = 0;
    let mut first = true;
    if let Some(children) = node.children() {
        for child in children {
            if matches!(node, Node::Root(_)) {
                let range = span(child)?;
                let gap = &source[previous_end..range.start];
                if gap.chars().all(char::is_whitespace) {
                    let cr_only = !gap.contains('\n');
                    let mut endings = gap
                        .char_indices()
                        .filter(|(_, ch)| *ch == '\n' || (*ch == '\r' && cr_only))
                        .map(|(offset, ch)| previous_end + offset + ch.len_utf8());
                    let mut at = previous_end;
                    let mut separator = !first;
                    while let Some(end) = endings.next() {
                        let end = endings.next().unwrap_or(end);
                        // Two newlines separate existing paragraphs. Additional
                        // pairs (or leading pairs) represent empty edit blocks.
                        if !separator {
                            blocks.push(Draft::empty(SectionKind::Text, at..at));
                        }
                        separator = false;
                        at = end;
                    }
                }
                previous_end = range.end;
                first = false;
            }
            blocks.extend(collect(source, child, containers)?);
        }
    }
    Ok(blocks)
}

fn collect(source: &str, node: &Node, containers: &[Container]) -> Result<Vec<Draft>> {
    let range = span(node)?;
    match node {
        Node::List(list) => {
            let mut draft = Draft::empty(
                SectionKind::List {
                    ordered: list.ordered,
                    start: u64::from(list.start.unwrap_or(1)),
                },
                range,
            );
            draft.children = collect_children(source, node, containers)?;
            if draft.children.is_empty() {
                draft.children.push(Draft::empty(
                    SectionKind::ListItem { checked: None },
                    draft.span.clone(),
                ));
            }
            Ok(vec![draft])
        }
        Node::ListItem(item) => {
            let mut nested = containers.to_vec();
            nested.push(Container::Item(item_indent(&source[range.clone()])));
            let mut children = collect_children(source, node, &nested)?;
            let inferred_checked = if item.checked.is_none() {
                children
                    .first()
                    .and_then(|child| match child.body.markdown.trim() {
                        "[x]" | "[X]" => Some(true),
                        "[ ]" => Some(false),
                        _ => None,
                    })
            } else {
                None
            };
            let kind = SectionKind::ListItem {
                checked: item.checked.or(inferred_checked),
            };
            let mut draft = if children
                .first()
                .is_some_and(|child| child.kind.is_editable())
            {
                children.remove(0)
            } else {
                Draft::empty(kind.clone(), range.start..range.start)
            };
            draft.kind = kind;
            if inferred_checked.is_some() || (item.checked.is_some()
                && item.children.first().is_some_and(|node| {
                    matches!(node, Node::Paragraph(paragraph) if paragraph.children.is_empty())
                }))
            {
                draft.body = Body::default();
            }
            draft.span.start = range.start;
            if draft.body.markdown.is_empty() {
                draft.span.end = children.first().map_or(range.end, |child| child.span.start);
            }
            draft.children = children;
            Ok(vec![draft])
        }
        Node::Blockquote(_) => {
            let mut nested = containers.to_vec();
            nested.push(Container::Quote);
            let mut draft = Draft::empty(SectionKind::Quote, range.clone());
            draft.children = collect_children(source, node, &nested)?;
            if draft.children.is_empty() {
                draft
                    .children
                    .push(Draft::empty(SectionKind::Text, range.clone()));
            }
            if let Some(first) = draft.children.first_mut() {
                expand_start(first, range.start);
            }
            Ok(vec![draft])
        }
        Node::Paragraph(_) => paragraph(source, node, containers),
        _ => {
            let mut draft = Draft::empty(SectionKind::Text, range.clone());
            copy_deindented(&mut draft.body, source, range, containers);
            Ok(vec![draft])
        }
    }
}

fn expand_start(draft: &mut Draft, start: usize) {
    draft.span.start = start;
    if !draft.kind.is_editable() {
        if let Some(first) = draft.children.first_mut() {
            expand_start(first, start);
        }
    }
}

fn paragraph(source: &str, node: &Node, containers: &[Container]) -> Result<Vec<Draft>> {
    let range = span(node)?;
    let mut wrappers = Vec::new();
    let mut protected = Vec::new();
    inline_ranges(node, &mut wrappers, &mut protected)?;
    html_wrappers(node, &mut Vec::new(), &mut wrappers)?;
    wrappers.sort_by_key(|wrapper| wrapper.opening.start);
    let mut blocks = Vec::new();
    let mut start = range.start;
    let mut previous_cut = None;
    for (index, byte) in source.as_bytes()[range.clone()].iter().enumerate() {
        let at = range.start + index;
        if *byte != b'\n' || protected.iter().any(|range| range.contains(&at)) {
            continue;
        }
        let line = source[start..at].trim_end_matches([' ', '\t', '\r']);
        if ["<br>", "<br/>", "<br />"].iter().any(|ending| {
            line.get(line.len().saturating_sub(ending.len())..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(ending))
        }) {
            continue;
        }
        let end = if source.as_bytes().get(at.wrapping_sub(1)) == Some(&b'\r') {
            at - 1
        } else {
            at
        };
        blocks.push(paragraph_piece(
            source,
            start..end,
            previous_cut,
            Some(at),
            &wrappers,
            containers,
        ));
        start = at + 1;
        previous_cut = Some(at);
    }
    blocks.push(paragraph_piece(
        source,
        start..range.end,
        previous_cut,
        None,
        &wrappers,
        containers,
    ));
    Ok(blocks)
}

fn paragraph_piece(
    source: &str,
    range: Range<usize>,
    previous_cut: Option<usize>,
    next_cut: Option<usize>,
    wrappers: &[Wrapper],
    containers: &[Container],
) -> Draft {
    let mut draft = Draft::empty(SectionKind::Text, range.clone());
    if let Some(at) = previous_cut {
        for wrapper in wrappers
            .iter()
            .filter(|wrapper| wrapper.content.start <= at && at < wrapper.content.end)
        {
            draft
                .body
                .markdown
                .push_str(&source[wrapper.opening.clone()]);
        }
    }
    let start = if previous_cut.is_some() {
        continuation(source, range.start, range.end, containers)
    } else {
        range.start
    };
    // A delimiter adjacent to whitespace is not a valid standalone emphasis
    // boundary. Encode only those edge spaces, retaining their source anchors.
    let mut content_start = start;
    if previous_cut.is_some_and(|at| wrappers.iter().any(|wrapper| wrapper.content.contains(&at))) {
        while content_start < range.end && matches!(source.as_bytes()[content_start], b' ' | b'\t')
        {
            draft
                .body
                .encoded_space(content_start, source.as_bytes()[content_start]);
            content_start += 1;
        }
    }
    let mut content_end = range.end;
    if next_cut.is_some_and(|at| wrappers.iter().any(|wrapper| wrapper.content.contains(&at))) {
        while content_end > content_start
            && matches!(source.as_bytes()[content_end - 1], b' ' | b'\t')
        {
            content_end -= 1;
        }
    }
    copy_deindented(
        &mut draft.body,
        source,
        content_start..content_end,
        containers,
    );
    for at in content_end..range.end {
        draft.body.encoded_space(at, source.as_bytes()[at]);
    }
    if let Some(at) = next_cut {
        for wrapper in wrappers
            .iter()
            .rev()
            .filter(|wrapper| wrapper.content.start <= at && at < wrapper.content.end)
        {
            draft
                .body
                .markdown
                .push_str(&source[wrapper.closing.clone()]);
        }
    }
    draft
}

fn inline_ranges(
    node: &Node,
    wrappers: &mut Vec<Wrapper>,
    protected: &mut Vec<Range<usize>>,
) -> Result<()> {
    let range = span(node)?;
    match node {
        Node::Break(_)
        | Node::InlineCode(_)
        | Node::InlineMath(_)
        | Node::Image(_)
        | Node::ImageReference(_)
        | Node::Html(_) => {
            protected.push(range);
            return Ok(());
        }
        Node::Emphasis(_)
        | Node::Strong(_)
        | Node::Delete(_)
        | Node::Link(_)
        | Node::LinkReference(_) => {
            if let Some(children) = node.children() {
                if let Some(last) = children.last() {
                    let width = if matches!(node, Node::Strong(_) | Node::Delete(_)) {
                        2
                    } else {
                        1
                    };
                    let content_start = range.start + width;
                    let content_end = if matches!(node, Node::Link(_) | Node::LinkReference(_)) {
                        span(last)?.end
                    } else {
                        range.end - width
                    };
                    protected.push(range.start..content_start);
                    protected.push(content_end..range.end);
                    wrappers.push(Wrapper {
                        content: content_start..content_end,
                        opening: range.start..content_start,
                        closing: content_end..range.end,
                    });
                }
            }
        }
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            inline_ranges(child, wrappers, protected)?;
        }
    }
    Ok(())
}

fn html_wrappers(
    node: &Node,
    stack: &mut Vec<(String, Range<usize>)>,
    wrappers: &mut Vec<Wrapper>,
) -> Result<()> {
    if let Node::Html(html) = node {
        let raw = html.value.trim();
        let closing = raw.starts_with("</");
        let name: String = raw
            .trim_start_matches('<')
            .trim_start_matches('/')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "strong"
                | "b"
                | "em"
                | "i"
                | "del"
                | "s"
                | "strike"
                | "u"
                | "sub"
                | "sup"
                | "mark"
                | "span"
                | "a"
        ) {
            let range = span(node)?;
            if closing {
                if let Some(index) = stack.iter().rposition(|(tag, _)| tag == &name) {
                    let (_, opening) = stack.remove(index);
                    wrappers.push(Wrapper {
                        content: opening.end..range.start,
                        opening,
                        closing: range,
                    });
                }
            } else if !raw.ends_with("/>") {
                stack.push((name, range));
            }
        }
    } else if let Some(children) = node.children() {
        for child in children {
            html_wrappers(child, stack, wrappers)?;
        }
    }
    Ok(())
}

/// Reference definitions have document scope in Markdown, whereas each result
/// is parsed independently by the native editor. Carry the original definition
/// source into every block that uses it; do not rewrite link labels or URLs.
fn preserve_reference_context(root: &Node, drafts: &mut Vec<Draft>) -> Result<()> {
    fn metadata<'a>(
        node: &'a Node,
        definitions: &mut Vec<(&'a str, Range<usize>)>,
        references: &mut Vec<(&'a str, Range<usize>)>,
    ) -> Result<()> {
        match node {
            Node::Definition(definition) => definitions.push((&definition.identifier, span(node)?)),
            Node::LinkReference(reference) => references.push((&reference.identifier, span(node)?)),
            Node::ImageReference(reference) => {
                references.push((&reference.identifier, span(node)?))
            }
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                metadata(child, definitions, references)?;
            }
        }
        Ok(())
    }
    fn is_definition(draft: &Draft, range: &Range<usize>) -> bool {
        draft.kind.is_editable()
            && draft
                .body
                .segments
                .first()
                .is_some_and(|segment| segment.source.start == range.start)
            && draft
                .body
                .segments
                .last()
                .is_some_and(|segment| segment.source.end == range.end)
    }
    fn definition_body<'a>(drafts: &'a [Draft], range: &Range<usize>) -> Option<&'a str> {
        for draft in drafts {
            if is_definition(draft, range) {
                return Some(&draft.body.markdown);
            }
            if let Some(body) = definition_body(&draft.children, range) {
                return Some(body);
            }
        }
        None
    }
    fn append(
        drafts: &mut [Draft],
        references: &[(&str, Range<usize>)],
        definitions: &[(&str, String)],
    ) {
        for draft in drafts {
            if draft.kind.is_editable() {
                for (identifier, definition) in definitions {
                    if references.iter().any(|(reference, range)| {
                        reference == identifier
                            && range.start < draft.span.end
                            && draft.span.start < range.end
                    }) {
                        draft.body.markdown.push_str("\n\n");
                        draft.body.markdown.push_str(definition);
                    }
                }
            }
            append(&mut draft.children, references, definitions);
        }
    }
    fn strip(drafts: &mut Vec<Draft>, ranges: &[Range<usize>]) {
        drafts.retain_mut(|draft| {
            if ranges.iter().any(|range| is_definition(draft, range)) {
                if draft.kind == SectionKind::Text {
                    return false;
                }
                draft.body = Body::default();
            }
            strip(&mut draft.children, ranges);
            if draft.kind == SectionKind::Quote && draft.children.is_empty() {
                draft
                    .children
                    .push(Draft::empty(SectionKind::Text, draft.span.clone()));
            }
            true
        });
    }
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    metadata(root, &mut definitions, &mut references)?;
    let used_ranges: Vec<_> = definitions
        .iter()
        .filter(|(identifier, _)| {
            references
                .iter()
                .any(|(reference, _)| reference == identifier)
        })
        .map(|(_, range)| range.clone())
        .collect();
    let mut context: Vec<(&str, String)> = Vec::new();
    for (identifier, range) in definitions {
        if !context.iter().any(|(existing, _)| *existing == identifier) {
            if let Some(body) = definition_body(drafts, &range) {
                context.push((identifier, body.to_owned()));
            }
        }
    }
    strip(drafts, &used_ranges);
    append(drafts, &references, &context);
    Ok(())
}

fn copy_deindented(body: &mut Body, source: &str, range: Range<usize>, containers: &[Container]) {
    let mut start = range.start;
    for (index, byte) in source.as_bytes()[range.clone()].iter().enumerate() {
        if *byte == b'\n' {
            let next = range.start + index + 1;
            body.copy(source, start..next);
            start = continuation(source, next, range.end, containers);
        }
    }
    body.copy(source, start..range.end);
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
                if cursor < end && source.as_bytes()[cursor] == b'>' {
                    at = cursor + 1;
                    if at < end && matches!(source.as_bytes()[at], b' ' | b'\t') {
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
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(parsed: &ParsedBlocks) -> Vec<&str> {
        parsed
            .blocks
            .iter()
            .map(|block| block.markdown.as_str())
            .collect()
    }

    fn block_at<'a>(parsed: &'a ParsedBlocks, path: &[usize]) -> &'a BlockSpec {
        let mut siblings = &parsed.blocks;
        let mut block = None;
        for index in path {
            block = Some(&siblings[*index]);
            siblings = &siblings[*index].children;
        }
        block.unwrap()
    }

    #[test]
    fn headings_and_physical_paragraph_lines_are_independent() {
        let parsed = parse_blocks("# Heading\n\nfirst\nsecond").unwrap();
        assert_eq!(texts(&parsed), ["# Heading", "first", "second"]);
        assert_eq!(parsed.locate(18), Some((vec![2], 1)));
    }

    #[test]
    fn identical_text_uses_the_correct_ast_occurrence() {
        let parsed = parse_blocks("same\nsame\n\nsame").unwrap();
        assert_eq!(texts(&parsed), ["same", "same", "same"]);
        assert_eq!(parsed.locate(5), Some((vec![1], 0)));
        assert_eq!(parsed.locate(11), Some((vec![2], 0)));
    }

    #[test]
    fn multiline_marks_and_links_are_closed_and_reopened() {
        let source = "**one\n*two\nthree***\n\n[a\nb](https://example.test \"title\")";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(
            texts(&parsed),
            [
                "**one**",
                "***two***",
                "***three***",
                "[a](https://example.test \"title\")",
                "[b](https://example.test \"title\")",
            ]
        );
        assert_eq!(parsed.locate(7), Some((vec![1], 3)));
        assert_eq!(parsed.locate(12), Some((vec![2], 4)));
    }

    #[test]
    fn whitespace_at_reopened_mark_boundaries_stays_valid_markdown() {
        let parsed = parse_blocks("*one \n two*").unwrap();
        assert_eq!(texts(&parsed), ["*one&#32;*", "*&#32;two*"]);
        for block in &parsed.blocks {
            let ast = markdown::to_mdast(&block.markdown, &markdown::ParseOptions::gfm()).unwrap();
            assert!(matches!(
                &ast.children().unwrap()[0].children().unwrap()[0],
                Node::Emphasis(_)
            ));
        }
    }

    #[test]
    fn nested_ordered_tasks_and_quotes_keep_their_structure() {
        let source = "3. parent\n   - [x] done\n   - [ ] todo\n\n   > quoted\n   > next\n4. last";
        let parsed = parse_blocks(source).unwrap();
        let list = &parsed.blocks[0];
        assert_eq!(
            list.kind,
            SectionKind::List {
                ordered: true,
                start: 3
            }
        );
        assert_eq!(list.markdown, "");
        assert_eq!(list.children[0].markdown, "parent");
        let nested = &list.children[0].children[0];
        assert_eq!(
            nested.kind,
            SectionKind::List {
                ordered: false,
                start: 1
            }
        );
        assert_eq!(
            nested.children[0].kind,
            SectionKind::ListItem {
                checked: Some(true)
            }
        );
        assert_eq!(nested.children[0].markdown, "done");
        assert_eq!(
            nested.children[1].kind,
            SectionKind::ListItem {
                checked: Some(false)
            }
        );
        let quote = &list.children[0].children[1];
        assert_eq!(quote.kind, SectionKind::Quote);
        assert_eq!(quote.children[0].markdown, "quoted");
        assert_eq!(quote.children[1].markdown, "next");
        assert_eq!(list.children[1].markdown, "last");
        let at = source.find("todo").unwrap() + 2;
        assert_eq!(parsed.locate(at), Some((vec![0, 0, 0, 1], 2)));
        let at = source.find("> next").unwrap();
        assert_eq!(parsed.locate(at), Some((vec![0, 0, 1, 1], 0)));
    }

    #[test]
    fn continuation_indentation_maps_to_the_deindented_body() {
        let source = "> - **α\n>   β**\n>   γ";
        let parsed = parse_blocks(source).unwrap();
        let item = block_at(&parsed, &[0, 0, 0]);
        assert_eq!(item.markdown, "**α**");
        assert_eq!(item.children[0].markdown, "**β**");
        assert_eq!(item.children[1].markdown, "γ");
        let at = source.find('β').unwrap();
        assert_eq!(parsed.locate(at), Some((vec![0, 0, 0, 0], 2)));
        assert_eq!(parsed.locate(at + 2), Some((vec![0, 0, 0, 0], 4)));
        assert_eq!(parsed.locate(at - 3), Some((vec![0, 0, 0, 0], 2)));
    }

    #[test]
    fn code_and_tables_are_atomic_including_list_item_bodies() {
        let source = "```rust\nlet a = 1;\nlet b = 2;\n```\n\n| a | b |\n| - | - |\n| c | d |";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(
            parsed.blocks[0].markdown,
            "```rust\nlet a = 1;\nlet b = 2;\n```"
        );
        assert_eq!(parsed.blocks[1].markdown, "| a | b |\n| - | - |\n| c | d |");
        let source = "- ```rs\n  α\n  β\n  ```\n- last";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(block_at(&parsed, &[0, 0]).markdown, "```rs\nα\nβ\n```");
        assert_eq!(
            parsed.locate(source.find('β').unwrap()),
            Some((vec![0, 0], "```rs\nα\n".len()))
        );
        let parsed = parse_blocks("- | a | b |\n  | - | - |\n  | c | d |").unwrap();
        assert_eq!(
            block_at(&parsed, &[0, 0]).markdown,
            "| a | b |\n| - | - |\n| c | d |"
        );
    }

    #[test]
    fn explicit_breaks_stay_inside_the_body() {
        for source in [
            "one\\\ntwo",
            "one  \ntwo",
            "one<br>\ntwo",
            "one<br />\ntwo",
            "one<br>two",
        ] {
            let parsed = parse_blocks(source).unwrap();
            assert_eq!(texts(&parsed), [source], "{source:?}");
        }
        let source = "> - one\\\n>   two";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(block_at(&parsed, &[0, 0, 0]).markdown, "one\\\ntwo");
        assert_eq!(
            parsed.locate(source.find("two").unwrap()),
            Some((vec![0, 0, 0], 5))
        );
    }

    #[test]
    fn inline_html_marks_survive_physical_line_splitting() {
        let parsed = parse_blocks("a <u>one\ntwo</u>").unwrap();
        assert_eq!(texts(&parsed), ["a <u>one</u>", "<u>two</u>"]);
    }

    #[test]
    fn reference_links_keep_document_scope_after_splitting() {
        let parsed =
            parse_blocks("[one\ntwo][id]\n\n[id]: https://example.test \"title\"").unwrap();
        assert_eq!(
            parsed.blocks[0].markdown,
            "[one][id]\n\n[id]: https://example.test \"title\""
        );
        assert_eq!(
            parsed.blocks[1].markdown,
            "[two][id]\n\n[id]: https://example.test \"title\""
        );
        for block in &parsed.blocks {
            let reparsed = parse_blocks(&block.markdown).unwrap();
            assert_eq!(reparsed.blocks.len(), 1);
            assert_eq!(reparsed.blocks[0].markdown, block.markdown);
        }
    }

    #[test]
    fn empty_containers_and_terminal_native_paragraphs_are_editable() {
        let parsed = parse_blocks("").unwrap();
        assert_eq!(texts(&parsed), [""]);
        assert_eq!(parsed.locate(0), Some((vec![0], 0)));
        for source in ["text\n", "text\n\n"] {
            let parsed = parse_blocks(source).unwrap();
            assert_eq!(texts(&parsed), ["text", ""]);
            assert_eq!(parsed.locate(source.len()), Some((vec![1], 0)));
        }
        let parsed = parse_blocks("- ").unwrap();
        assert_eq!(
            block_at(&parsed, &[0, 0]).kind,
            SectionKind::ListItem { checked: None }
        );
        assert_eq!(parsed.locate(2), Some((vec![0, 0], 0)));
        for source in ["- [x]", "- [ ]", "- [x] "] {
            let parsed = parse_blocks(source).unwrap();
            assert_eq!(block_at(&parsed, &[0, 0]).markdown, "");
            assert_eq!(
                block_at(&parsed, &[0, 0]).kind,
                SectionKind::ListItem {
                    checked: Some(source.contains('x'))
                }
            );
            assert_eq!(parsed.locate(source.len()), Some((vec![0, 0], 0)));
        }
        let parsed = parse_blocks("> ").unwrap();
        assert_eq!(parsed.locate(2), Some((vec![0, 0], 0)));
        let source = "- first\n- ";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(parsed.locate(source.len()), Some((vec![0, 1], 0)));
    }

    #[test]
    fn unicode_offsets_are_raw_utf8_and_always_on_boundaries() {
        let source = "é🙂\n漢字";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(texts(&parsed), ["é🙂", "漢字"]);
        assert_eq!(parsed.locate(2), Some((vec![0], 2)));
        assert_eq!(parsed.locate(4), Some((vec![0], 2)));
        assert_eq!(parsed.locate(7), Some((vec![1], 0)));
        assert_eq!(parsed.locate(source.len()), Some((vec![1], 6)));
        for at in 0..=source.len() + 2 {
            let (path, offset) = parsed.locate(at).unwrap();
            let block = block_at(&parsed, &path);
            assert!(block.kind.is_editable());
            assert!(block.markdown.is_char_boundary(offset));
        }
    }
}
