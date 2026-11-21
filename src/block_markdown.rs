use std::{collections::HashSet, ops::Range};

use anyhow::{Context, Result};
use markdown::mdast::Node;
use markdown_editor::rich::{RichDocument, RichKind};

use crate::block::{BlockSpec, SectionKind};

/// Markdown import result; source positions never survive into runtime state.
pub(crate) struct ParsedBlocks {
    pub blocks: Vec<BlockSpec>,
}

#[derive(Default)]
struct Body {
    markdown: String,
}

impl Body {
    fn copy(&mut self, source: &str, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        self.markdown.push_str(&source[range]);
    }

    fn encoded_space(&mut self, byte: u8) {
        let entity = if byte == b'\t' { "&#9;" } else { "&#32;" };
        self.markdown.push_str(entity);
    }
}

struct Draft {
    kind: SectionKind,
    body: Body,
    span: Range<usize>,
    children: Vec<Draft>,
    // Disclosure children have already resolved references in their own source.
    local_context: bool,
}

impl Draft {
    fn empty(kind: SectionKind, span: Range<usize>) -> Self {
        Self {
            kind,
            body: Body::default(),
            span,
            children: Vec::new(),
            local_context: false,
        }
    }
    fn finish(self) -> Result<BlockSpec> {
        let children = self
            .children
            .into_iter()
            .map(Draft::finish)
            .collect::<Result<_>>()?;
        let content = if self.kind.is_editable() {
            let mut content = RichDocument::from_markdown(&self.body.markdown)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if content.blocks.is_empty() {
                content = RichDocument::paragraph("");
            } else if content
                .blocks
                .iter()
                .all(|node| node.kind == RichKind::Definition)
            {
                content
                    .blocks
                    .insert(0, markdown_editor::rich::RichNode::paragraph(""));
            }
            let mut definitions = HashSet::new();
            content.blocks.retain(|node| {
                node.kind != RichKind::Definition
                    || node
                        .attrs
                        .get("identifier")
                        .and_then(serde_json::Value::as_str)
                        .is_none_or(|id| definitions.insert(id.to_owned()))
            });
            content
        } else {
            RichDocument::default()
        };
        Ok(BlockSpec {
            kind: self.kind,
            content,
            children,
        })
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

/// Physical paragraph lines become separate bodies, with Markdown's rendered
/// whitespace normalization. Native rich-text literals bypass this boundary.
pub(crate) fn parse_blocks(source: &str) -> Result<ParsedBlocks> {
    let blocks = parse_drafts(source)?
        .into_iter()
        .map(Draft::finish)
        .collect::<Result<_>>()?;
    Ok(ParsedBlocks { blocks })
}

fn parse_drafts(source: &str) -> Result<Vec<Draft>> {
    let mut options = markdown::ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let root =
        markdown::to_mdast(source, &options).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let reference_context = definition_context(source, &root)?;
    let mut drafts = collect_children(source, &root, &[], &reference_context)?;
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
    Ok(drafts)
}
fn definition_context(source: &str, root: &Node) -> Result<String> {
    fn visit(source: &str, node: &Node, out: &mut String) -> Result<()> {
        if matches!(node, Node::Definition(_)) {
            out.push_str("\n\n");
            out.push_str(&source[span(node)?]);
        }
        if let Some(children) = node.children() {
            for child in children {
                visit(source, child, out)?;
            }
        }
        Ok(())
    }
    let mut context = String::new();
    visit(source, root, &mut context)?;
    Ok(context)
}

fn drafts_with_context(source: &str, context: &str) -> Result<Vec<Draft>> {
    if context.is_empty() {
        return parse_drafts(source);
    }
    let mut contextual = String::with_capacity(source.len() + context.len());
    contextual.push_str(source);
    contextual.push_str(context);
    let mut drafts = parse_drafts(&contextual)?;
    // Definitions appended for scope are not extra collaborative sections.
    drafts.retain(|draft| draft.span.start < source.len());
    Ok(drafts)
}

fn title_with_context(title: String, context: &str) -> Result<String> {
    if context.is_empty() {
        return Ok(title);
    }
    let mut source = String::with_capacity(title.len() + context.len());
    source.push_str(&title);
    source.push_str(context);
    let root = markdown::to_mdast(&source, &markdown::ParseOptions::gfm())
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    fn references<'a>(node: &'a Node, end: usize, out: &mut Vec<&'a str>) {
        if node
            .position()
            .is_some_and(|position| position.start.offset >= end)
        {
            return;
        }
        match node {
            Node::LinkReference(reference) => out.push(&reference.identifier),
            Node::ImageReference(reference) => out.push(&reference.identifier),
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                references(child, end, out);
            }
        }
    }
    fn append(
        node: &Node,
        source: &str,
        end: usize,
        references: &[&str],
        out: &mut String,
    ) -> Result<()> {
        if let Node::Definition(definition) = node
            && span(node)?.start >= end
            && references.contains(&definition.identifier.as_str())
        {
            out.push_str("\n\n");
            out.push_str(&source[span(node)?]);
        }
        if let Some(children) = node.children() {
            for child in children {
                append(child, source, end, references, out)?;
            }
        }
        Ok(())
    }
    let mut used = Vec::new();
    references(&root, title.len(), &mut used);
    let mut resolved = title;
    append(&root, &source, resolved.len(), &used, &mut resolved)?;
    Ok(resolved)
}

/// Export a disclosure at the Markdown boundary. The escaped Markdown attribute
/// retains rich summary semantics that HTML's inline-only summary cannot express.
pub(crate) fn collapsible_markdown(
    content: &RichDocument,
    children_markdown: &str,
    default_collapsed: bool,
    child_count: usize,
) -> String {
    let markdown = crate::block::content_markdown(content);
    let html = content_html(&markdown);
    let open = if default_collapsed { "" } else { " open" };
    format!(
        "<details data-native-block=\"true\" data-child-count=\"{child_count}\"{open}>\n<summary data-markdown=\"{}\">{html}</summary>\n\n{children_markdown}\n\n</details>",
        escape_html(&markdown),
    )
}

fn content_html(source: &str) -> String {
    // Raw HTML is deliberately escaped, including custom payloads. Never turn
    // imported source into executable HTML merely because it is a summary.
    markdown::to_html_with_options(source, &markdown::Options::gfm())
        .expect("validated Markdown must convert to HTML")
        .trim_end()
        .to_owned()
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn decode_html(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(at) = remaining.find('&') {
        decoded.push_str(&remaining[..at]);
        remaining = &remaining[at..];
        let Some(end) = remaining.find(';') else {
            break;
        };
        let entity = &remaining[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|value| u32::from_str_radix(value, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|value| value.parse().ok())
                })
                .and_then(char::from_u32),
        };
        if let Some(ch) = ch {
            decoded.push(ch);
            remaining = &remaining[end + 1..];
        } else {
            decoded.push('&');
            remaining = &remaining[1..];
        }
    }
    decoded.push_str(remaining);
    decoded
}

struct HtmlTag<'a> {
    name: &'a str,
    closing: bool,
    attrs: Vec<(&'a str, Option<&'a str>)>,
    end: usize,
}

fn html_tag(source: &str, at: usize) -> Option<HtmlTag<'_>> {
    let bytes = source.as_bytes();
    if bytes.get(at) != Some(&b'<') {
        return None;
    }
    let mut cursor = at + 1;
    let closing = bytes.get(cursor) == Some(&b'/');
    cursor += usize::from(closing);
    let start = cursor;
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
    {
        cursor += 1;
    }
    if cursor == start {
        return None;
    }
    let name = &source[start..cursor];
    let mut attrs = Vec::new();
    loop {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b'>') {
            return Some(HtmlTag {
                name,
                closing,
                attrs,
                end: cursor + 1,
            });
        }
        if bytes.get(cursor) == Some(&b'/') && bytes.get(cursor + 1) == Some(&b'>') {
            return Some(HtmlTag {
                name,
                closing,
                attrs,
                end: cursor + 2,
            });
        }
        let start = cursor;
        while bytes.get(cursor).is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/' | b'<' | b'\'' | b'"')
        }) {
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        let attr = &source[start..cursor];
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let mut value = None;
        if bytes.get(cursor) == Some(&b'=') {
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            let quote = bytes
                .get(cursor)
                .copied()
                .filter(|byte| matches!(byte, b'\'' | b'"'));
            cursor += usize::from(quote.is_some());
            let start = cursor;
            while let Some(byte) = bytes.get(cursor) {
                if quote.map_or(byte.is_ascii_whitespace() || *byte == b'>', |quote| {
                    *byte == quote
                }) {
                    break;
                }
                cursor += 1;
            }
            value = Some(&source[start..cursor]);
            if quote.is_some() {
                if bytes.get(cursor).copied() != quote {
                    return None;
                }
                cursor += 1;
            }
        }
        attrs.push((attr, value));
    }
}

fn details_draft(
    source: &str,
    start: usize,
    limit: usize,
    containers: &[Container],
    reference_context: &str,
) -> Result<Option<(Draft, usize)>> {
    let Some(opening) = html_tag(source, start) else {
        return Ok(None);
    };
    if opening.closing || !opening.name.eq_ignore_ascii_case("details") {
        return Ok(None);
    }
    let mut depth = 1;
    let mut at = opening.end;
    let mut fence: Option<(u8, usize)> = None;
    let (closing_start, end) = loop {
        if at >= limit {
            return Ok(None);
        }
        if at == start || source.as_bytes().get(at.wrapping_sub(1)) == Some(&b'\n') {
            let line_start = continuation(source, at, limit, containers);
            let line_end = source[line_start..limit]
                .find('\n')
                .map_or(limit, |end| line_start + end);
            let line = source[line_start..line_end].trim_start_matches(' ');
            if let Some(marker @ (b'`' | b'~')) = line.as_bytes().first().copied() {
                let width = line.bytes().take_while(|byte| *byte == marker).count();
                if width >= 3 {
                    if let Some((active, minimum)) = fence {
                        if active == marker && width >= minimum && line[width..].trim().is_empty() {
                            fence = None;
                        }
                    } else {
                        fence = Some((marker, width));
                    }
                    at = (line_end + 1).min(limit);
                    continue;
                }
            }
            if fence.is_some() {
                at = (line_end + 1).min(limit);
                continue;
            }
        }
        if source.as_bytes()[at] == b'`' {
            let width = source.as_bytes()[at..limit]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            let mut cursor = at + width;
            let mut closing = None;
            while cursor < limit {
                if source.as_bytes()[cursor] == b'`' {
                    let run = source.as_bytes()[cursor..limit]
                        .iter()
                        .take_while(|byte| **byte == b'`')
                        .count();
                    if run == width {
                        closing = Some(cursor + run);
                        break;
                    }
                    cursor += run;
                } else {
                    cursor += 1;
                }
            }
            if let Some(end) = closing {
                at = end;
                continue;
            }
        }
        if source[at..limit].starts_with("<!--") {
            at = source[at + 4..limit]
                .find("-->")
                .map_or(limit, |end| at + 4 + end + 3);
            continue;
        }
        if let Some(tag) = html_tag(source, at) {
            if tag.end > limit {
                return Ok(None);
            }
            if tag.name.eq_ignore_ascii_case("details") {
                if tag.closing {
                    depth -= 1;
                    if depth == 0 {
                        break (at, tag.end);
                    }
                } else {
                    depth += 1;
                }
            }
            at = tag.end;
        } else {
            at += source[at..].chars().next().unwrap().len_utf8();
        }
    };
    let mut body = Body::default();
    copy_deindented(&mut body, source, start..end, containers);
    let raw = &body.markdown;
    let opening = html_tag(raw, 0).context("Invalid details opening")?;
    let collapsed = !opening
        .attrs
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("open"));
    let native = opening.attrs.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("data-native-block") && *value == Some("true")
    });
    let has_children = native
        && opening
            .attrs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("data-child-count"))
            .and_then(|(_, value)| *value)
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|count| count > 0);
    let summary_start =
        opening.end + raw[opening.end..].len() - raw[opening.end..].trim_start().len();
    let Some(summary) = html_tag(raw, summary_start) else {
        return Ok(None);
    };
    if summary.closing || !summary.name.eq_ignore_ascii_case("summary") {
        return Ok(None);
    }
    let mut cursor = summary.end;
    let summary_end = loop {
        let Some(next) = raw[cursor..].find('<') else {
            return Ok(None);
        };
        cursor += next;
        if let Some(tag) = html_tag(raw, cursor) {
            if tag.closing && tag.name.eq_ignore_ascii_case("summary") {
                break (cursor, tag.end);
            }
            cursor = tag.end;
        } else {
            cursor += 1;
        }
    };
    let title = summary
        .attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("data-markdown"))
        .and_then(|(_, value)| *value)
        .map(decode_html)
        .unwrap_or_else(|| raw[summary.end..summary_end.0].trim().to_owned());
    // Deindentation changes offsets; locate the known terminal closing tag in
    // the deindented body instead of retaining a runtime source map.
    let closing_len = end - closing_start;
    let children_source = &raw[summary_end.1..raw.len() - closing_len];
    let children_source = if native {
        let body = children_source
            .strip_prefix("\n\n")
            .unwrap_or(children_source);
        body.strip_suffix("\n\n").unwrap_or(body)
    } else {
        children_source.trim_matches(['\r', '\n'])
    };
    let mut draft = Draft::empty(
        SectionKind::Collapsible {
            default_collapsed: collapsed,
        },
        start..end,
    );
    draft.body.markdown = title_with_context(title, reference_context)?;
    if !children_source.is_empty() {
        draft.children = drafts_with_context(children_source, reference_context)?;
    } else if has_children {
        draft.children.push(Draft::empty(SectionKind::Text, 0..0));
    }
    for child in &mut draft.children {
        child.local_context = true;
    }
    Ok(Some((draft, end)))
}

fn span(node: &Node) -> Result<Range<usize>> {
    let position = node
        .position()
        .context("Markdown AST node has no source position")?;
    Ok(position.start.offset..position.end.offset)
}

fn trailing_drafts(
    source: &str,
    range: Range<usize>,
    containers: &[Container],
    reference_context: &str,
) -> Result<Vec<Draft>> {
    let mut trailing = Body::default();
    copy_deindented(&mut trailing, source, range, containers);
    let trailing = trailing.markdown.trim_matches(['\r', '\n']);
    if trailing.is_empty() {
        return Ok(Vec::new());
    }
    let mut drafts = drafts_with_context(trailing, reference_context)?;
    for draft in &mut drafts {
        draft.local_context = true;
    }
    Ok(drafts)
}

fn collect_children(
    source: &str,
    node: &Node,
    containers: &[Container],
    reference_context: &str,
) -> Result<Vec<Draft>> {
    let mut blocks = Vec::new();
    let mut previous_end = 0;
    let mut first = true;
    let mut consumed_until = 0;
    if let Some(children) = node.children() {
        for child in children {
            let range = span(child)?;
            if range.start < consumed_until {
                // The closing HTML node can also contain text after </details>.
                // Only the prefix was consumed by the disclosure parser.
                if range.end > consumed_until {
                    blocks.extend(trailing_drafts(
                        source,
                        consumed_until..range.end,
                        containers,
                        reference_context,
                    )?);
                    previous_end = range.end;
                    consumed_until = range.end;
                }
                continue;
            }
            if matches!(node, Node::Root(_)) {
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
            if matches!(child, Node::Html(_))
                && let Some((draft, end)) = details_draft(
                    source,
                    range.start,
                    span(node)?.end,
                    containers,
                    reference_context,
                )?
            {
                blocks.push(draft);
                consumed_until = end;
                previous_end = end;
                if range.end > end {
                    blocks.extend(trailing_drafts(
                        source,
                        end..range.end,
                        containers,
                        reference_context,
                    )?);
                    previous_end = range.end;
                    consumed_until = range.end;
                }
            } else {
                blocks.extend(collect(source, child, containers, reference_context)?);
            }
        }
    }
    Ok(blocks)
}

fn collect(
    source: &str,
    node: &Node,
    containers: &[Container],
    reference_context: &str,
) -> Result<Vec<Draft>> {
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
            draft.children = collect_children(source, node, containers, reference_context)?;
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
            let mut children = collect_children(source, node, &nested, reference_context)?;
            if let Some(SectionKind::Collapsible { default_collapsed }) =
                children.first().map(|child| &child.kind)
            {
                let kind = SectionKind::CollapsibleListItem {
                    default_collapsed: *default_collapsed,
                };
                let mut draft = children.remove(0);
                draft.kind = kind;
                draft.span = range;
                draft.children.extend(children);
                return Ok(vec![draft]);
            }
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
                .is_some_and(|child| child.kind == SectionKind::Text)
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
            draft.children = collect_children(source, node, &nested, reference_context)?;
            if draft.children.is_empty() {
                draft
                    .children
                    .push(Draft::empty(SectionKind::Text, range.clone()));
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
    // Rebased emphasis delimiters cannot touch literal whitespace. Entities
    // keep the delimiters valid; Markdown still normalizes rendered indentation.
    let mut content_start = start;
    if previous_cut.is_some_and(|at| wrappers.iter().any(|wrapper| wrapper.content.contains(&at))) {
        while content_start < range.end && matches!(source.as_bytes()[content_start], b' ' | b'\t')
        {
            draft.body.encoded_space(source.as_bytes()[content_start]);
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
        draft.body.encoded_space(source.as_bytes()[at]);
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
        draft.kind.is_editable() && draft.span == *range
    }
    fn definition_body<'a>(drafts: &'a [Draft], range: &Range<usize>) -> Option<&'a str> {
        for draft in drafts {
            if draft.local_context {
                continue;
            }
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
            if draft.local_context {
                continue;
            }
            if draft.kind.is_editable() && draft.kind.default_collapsed().is_none() {
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
            if draft.local_context {
                return true;
            }
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
    let definition_ranges: Vec<_> = definitions.iter().map(|(_, range)| range.clone()).collect();
    let mut context: Vec<(&str, String)> = Vec::new();
    for (identifier, range) in definitions {
        if !context.iter().any(|(existing, _)| *existing == identifier) {
            if let Some(body) = definition_body(drafts, &range) {
                context.push((identifier, body.to_owned()));
            }
        }
    }
    strip(drafts, &definition_ranges);
    append(drafts, &references, &context);
    // Unused definitions are metadata too. Attaching them to an existing body
    // avoids manufacturing an empty visible block on each export/import cycle.
    fn first_body(drafts: &mut [Draft]) -> Option<&mut Body> {
        for draft in drafts {
            if draft.kind.is_editable() {
                return Some(&mut draft.body);
            }
            if let Some(body) = first_body(&mut draft.children) {
                return Some(body);
            }
        }
        None
    }
    let mut unused = context
        .iter()
        .filter(|(id, _)| !references.iter().any(|(reference, _)| reference == id))
        .peekable();
    if unused.peek().is_some() {
        if first_body(drafts).is_none() {
            drafts.push(Draft::empty(SectionKind::Text, 0..0));
        }
        let body = first_body(drafts).expect("metadata has an editable owner");
        for (_, definition) in unused {
            if !body.markdown.is_empty() {
                body.markdown.push_str("\n\n");
            }
            body.markdown.push_str(definition);
        }
    }
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
    use crate::block::specs_from_rich;

    fn texts(parsed: &ParsedBlocks) -> Vec<String> {
        parsed
            .blocks
            .iter()
            .map(|block| block.content.plain_text())
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
    fn headings_and_physical_lines_are_semantic_bodies() {
        let parsed = parse_blocks("# Heading\n\nfirst\nsecond").unwrap();
        assert_eq!(texts(&parsed), ["Heading", "first", "second"]);
        assert_eq!(parsed.blocks[0].content.blocks[0].kind, RichKind::Heading);
        let parsed = parse_blocks("same\nsame\n\nsame").unwrap();
        assert_eq!(texts(&parsed), ["same", "same", "same"]);
        assert_ne!(
            parsed.blocks[0].content.blocks[0].id,
            parsed.blocks[1].content.blocks[0].id
        );
        assert_eq!(texts(&parse_blocks("é🙂\n漢字").unwrap()), ["é🙂", "漢字"]);
    }

    #[test]
    fn multiline_marks_links_and_whitespace_survive() {
        let parsed =
            parse_blocks("**one\n*two\nthree***\n\n[a\nb](https://example.test \"title\")")
                .unwrap();
        assert_eq!(texts(&parsed), ["one", "two", "three", "a", "b"]);
        assert!(parsed.blocks[0].content.blocks[0].runs[0].marks.bold);
        for block in &parsed.blocks[1..3] {
            let marks = &block.content.blocks[0].runs[0].marks;
            assert!(marks.bold && marks.italic);
        }
        for block in &parsed.blocks[3..] {
            let link = block.content.blocks[0].runs[0].marks.link.as_ref().unwrap();
            assert_eq!(link.url, "https://example.test");
            assert_eq!(link.title.as_deref(), Some("title"));
        }
        let parsed = parse_blocks("*one \n two*").unwrap();
        assert_eq!(texts(&parsed), ["one ", "two"]);
        assert!(parsed.blocks.iter().all(|block| {
            block.content.blocks[0]
                .runs
                .iter()
                .all(|run| run.marks.italic)
        }));
        // Markdown line indentation is not literal leading rich-text content.
        let parsed = parse_blocks("[one \n two](https://example.test \"title\")").unwrap();
        assert_eq!(texts(&parsed), ["one ", "two"]);
        for block in &parsed.blocks {
            for run in &block.content.blocks[0].runs {
                let link = run.marks.link.as_ref().unwrap();
                assert_eq!(link.url, "https://example.test");
                assert_eq!(link.title.as_deref(), Some("title"));
            }
        }
        let parsed = parse_blocks("**one\n *two***").unwrap();
        assert_eq!(texts(&parsed), ["one", "two"]);
        let runs = &parsed.blocks[1].content.blocks[0].runs;
        assert!(runs.iter().all(|run| run.marks.bold && run.marks.italic));
        let parsed = parse_blocks("> - *one \n>    two*").unwrap();
        let item = block_at(&parsed, &[0, 0, 0]);
        assert_eq!(item.content.plain_text(), "one ");
        assert_eq!(item.children[0].content.plain_text(), "two");
        assert!(
            item.children[0].content.blocks[0]
                .runs
                .iter()
                .all(|run| run.marks.italic)
        );
        // Literal semantic content retains both edge spaces without Markdown.
        let literal = RichDocument::paragraph(" two ");
        let specs = specs_from_rich(&literal).unwrap();
        assert_eq!(specs[0].content.plain_text(), " two ");
    }

    #[test]
    fn nested_ordered_tasks_and_quotes_keep_structure() {
        let parsed = parse_blocks(
            "3. parent\n   - [x] done\n   - [ ] todo\n\n   > quoted\n   > next\n4. last",
        )
        .unwrap();
        let list = &parsed.blocks[0];
        assert_eq!(
            list.kind,
            SectionKind::List {
                ordered: true,
                start: 3
            }
        );
        assert!(list.content.blocks.is_empty());
        assert_eq!(list.children[0].content.plain_text(), "parent");
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
        assert_eq!(nested.children[0].content.plain_text(), "done");
        assert_eq!(
            nested.children[1].kind,
            SectionKind::ListItem {
                checked: Some(false)
            }
        );
        let quote = &list.children[0].children[1];
        assert_eq!(quote.kind, SectionKind::Quote);
        assert_eq!(quote.children[0].content.plain_text(), "quoted");
        assert_eq!(quote.children[1].content.plain_text(), "next");
        assert_eq!(list.children[1].content.plain_text(), "last");
        let parsed = parse_blocks("> - **α\n>   β**\n>   γ").unwrap();
        let item = block_at(&parsed, &[0, 0, 0]);
        assert_eq!(item.content.plain_text(), "α");
        assert_eq!(item.children[0].content.plain_text(), "β");
        assert_eq!(item.children[1].content.plain_text(), "γ");
        assert!(item.children[0].content.blocks[0].runs[0].marks.bold);
    }

    #[test]
    fn code_and_tables_are_atomic_including_item_bodies() {
        let parsed =
            parse_blocks("```rust\nlet a = 1;\nlet b = 2;\n```\n\n| a | b |\n| - | - |\n| c | d |")
                .unwrap();
        assert_eq!(parsed.blocks.len(), 2);
        let code = &parsed.blocks[0].content.blocks[0];
        assert_eq!(code.kind, RichKind::CodeBlock);
        assert_eq!(code.attrs["lang"], "rust");
        assert_eq!(code.text(), "let a = 1;\nlet b = 2;");
        assert_eq!(parsed.blocks[1].content.blocks[0].kind, RichKind::Table);
        let parsed = parse_blocks("- ```rs\n  α\n  β\n  ```\n- last").unwrap();
        assert_eq!(block_at(&parsed, &[0, 0]).content.blocks[0].text(), "α\nβ");
        let parsed = parse_blocks("- | a | b |\n  | - | - |\n  | c | d |").unwrap();
        assert_eq!(
            block_at(&parsed, &[0, 0]).content.blocks[0].kind,
            RichKind::Table
        );
    }

    #[test]
    fn explicit_breaks_and_html_marks_stay_semantic() {
        for source in [
            "one\\\ntwo",
            "one  \ntwo",
            "one<br>\ntwo",
            "one<br />\ntwo",
            "one<br>two",
        ] {
            let parsed = parse_blocks(source).unwrap();
            assert_eq!(parsed.blocks.len(), 1, "{source:?}");
            assert!(parsed.blocks[0].content.plain_text().contains("one"));
            assert!(parsed.blocks[0].content.plain_text().contains("two"));
        }
        let parsed = parse_blocks("a <u>one\ntwo</u>").unwrap();
        assert_eq!(texts(&parsed), ["a one", "two"]);
        assert!(
            parsed.blocks[0].content.blocks[0]
                .runs
                .last()
                .unwrap()
                .marks
                .underline
        );
        assert!(parsed.blocks[1].content.blocks[0].runs[0].marks.underline);
    }

    #[test]
    fn references_keep_scope_and_unused_definitions_remain() {
        let parsed =
            parse_blocks("[one\ntwo][id]\n\n[id]: https://example.test \"title\"").unwrap();
        assert_eq!(texts(&parsed), ["one", "two"]);
        for block in &parsed.blocks {
            let link = block.content.blocks[0].runs[0].marks.link.as_ref().unwrap();
            assert_eq!(link.url, "https://example.test");
            assert_eq!(link.title.as_deref(), Some("title"));
            let definition = block
                .content
                .blocks
                .iter()
                .find(|node| node.kind == RichKind::Definition)
                .unwrap();
            assert_eq!(definition.attrs["identifier"], "id");
            assert_eq!(definition.attrs["url"], "https://example.test");
            assert_eq!(definition.attrs["title"], "title");
            let reparsed = parse_blocks(&crate::block::content_markdown(&block.content)).unwrap();
            assert_eq!(reparsed.blocks.len(), 1);
            assert_eq!(
                reparsed.blocks[0].content.plain_text(),
                block.content.plain_text()
            );
            let link = reparsed.blocks[0].content.blocks[0].runs[0]
                .marks
                .link
                .as_ref()
                .unwrap();
            assert_eq!(link.url, "https://example.test");
            assert_eq!(link.title.as_deref(), Some("title"));
        }
        let unused = parse_blocks("[unused]: https://example.test \"unused title\"").unwrap();
        assert_eq!(unused.blocks[0].content.blocks[0].kind, RichKind::Paragraph);
        let definition = unused.blocks[0]
            .content
            .blocks
            .iter()
            .find(|node| node.kind == RichKind::Definition)
            .unwrap();
        assert_eq!(definition.attrs["identifier"], "unused");
        assert_eq!(definition.attrs["url"], "https://example.test");
        assert_eq!(definition.attrs["title"], "unused title");
        let reparsed =
            parse_blocks(&crate::block::content_markdown(&unused.blocks[0].content)).unwrap();
        let definition = reparsed.blocks[0]
            .content
            .blocks
            .iter()
            .find(|node| node.kind == RichKind::Definition)
            .unwrap();
        assert_eq!(definition.attrs["identifier"], "unused");
        assert_eq!(definition.attrs["url"], "https://example.test");
        assert_eq!(definition.attrs["title"], "unused title");
    }

    #[test]
    fn empty_containers_and_typing_paragraphs_remain_editable() {
        assert_eq!(texts(&parse_blocks("").unwrap()), [""]);
        for source in ["text\n", "text\n\n"] {
            assert_eq!(texts(&parse_blocks(source).unwrap()), ["text", ""]);
        }
        assert_eq!(
            texts(&parse_blocks("\n\ntext\n\n\n\nnext").unwrap()),
            ["", "text", "", "next"]
        );
        let parsed = parse_blocks("- ").unwrap();
        assert_eq!(
            block_at(&parsed, &[0, 0]).kind,
            SectionKind::ListItem { checked: None }
        );
        for source in ["- [x]", "- [ ]", "- [x] "] {
            let parsed = parse_blocks(source).unwrap();
            let item = block_at(&parsed, &[0, 0]);
            assert_eq!(item.content.plain_text(), "");
            assert_eq!(
                item.kind,
                SectionKind::ListItem {
                    checked: Some(source.contains('x'))
                }
            );
        }
        let parsed = parse_blocks("> ").unwrap();
        assert_eq!(block_at(&parsed, &[0, 0]).kind, SectionKind::Text);
        let parsed = parse_blocks("- first\n- ").unwrap();
        assert_eq!(block_at(&parsed, &[0, 1]).content.plain_text(), "");
    }

    #[test]
    fn native_nested_normalization_preserves_identities() {
        let rich = RichDocument::from_markdown(
            "3. **parent**\n   - [x] child\n\n   > quoted\n\n   ```rs\n   code\n   ```",
        )
        .unwrap();
        let specs = specs_from_rich(&rich).unwrap();
        assert_eq!(
            specs[0].kind,
            SectionKind::List {
                ordered: true,
                start: 3
            }
        );
        assert_eq!(specs[0].content.blocks[0].id, rich.blocks[0].id);
        let item = &rich.blocks[0].children[0];
        let body = &specs[0].children[0];
        assert_eq!(body.content.blocks[0].id, item.children[0].id);
        assert_eq!(
            body.content.blocks[0].attrs["section_list_item"]["id"],
            item.id
        );
        assert!(body.content.blocks[0].runs[0].marks.bold);
        assert!(
            body.children
                .iter()
                .any(|child| matches!(child.kind, SectionKind::List { .. }))
        );
        assert!(
            body.children
                .iter()
                .any(|child| child.kind == SectionKind::Quote)
        );
        assert!(body.children.iter().any(|child| {
            child
                .content
                .blocks
                .first()
                .is_some_and(|node| node.kind == RichKind::CodeBlock)
        }));
    }

    #[test]
    fn definition_only_bodies_normalize_without_losing_ids() {
        let rich = RichDocument::from_markdown("[id]: https://example.test").unwrap();
        let specs = specs_from_rich(&rich).unwrap();
        assert_eq!(specs[0].content.blocks[0].kind, RichKind::Paragraph);
        assert!(
            specs[0]
                .content
                .blocks
                .iter()
                .any(|node| node.kind == RichKind::Definition)
        );
    }

    #[test]
    fn collapsibles_roundtrip_nested_content_and_escaped_summary() {
        let title = RichDocument::from_markdown(
            "**A & B** `\"<tag>\"` [link](https://example.test?q=a&b=c)",
        )
        .unwrap();
        let nested = collapsible_markdown(&RichDocument::paragraph("inner"), "> quoted", false, 1);
        let body =
            format!("- [x] task `</details>`\n  - child\n\n```html\n</details>\n```\n\n{nested}");
        for collapsed in [false, true] {
            let source = collapsible_markdown(&title, &body, collapsed, 3);
            assert!(source.contains("&amp;"));
            assert!(source.contains("&lt;tag&gt;"));
            let parsed = parse_blocks(&source).unwrap();
            assert_eq!(parsed.blocks.len(), 1);
            let details = &parsed.blocks[0];
            assert_eq!(
                details.kind,
                SectionKind::Collapsible {
                    default_collapsed: collapsed
                }
            );
            assert_eq!(details.content.plain_text(), title.plain_text());
            assert!(details.content.blocks[0].runs[0].marks.bold);
            assert_eq!(details.children.len(), 3);
            assert!(matches!(details.children[0].kind, SectionKind::List { .. }));
            assert_eq!(details.children[1].content.blocks[0].text(), "</details>");
            assert_eq!(
                details.children[2].kind,
                SectionKind::Collapsible {
                    default_collapsed: false
                }
            );
            assert_eq!(details.children[2].children[0].kind, SectionKind::Quote);
        }
    }

    #[test]
    fn plain_html_and_nested_container_details_import() {
        let parsed = parse_blocks(
            "<details open>\n<summary>A &amp; <strong>B</strong></summary>\n\nbody\n\n</details>",
        )
        .unwrap();
        let details = &parsed.blocks[0];
        assert_eq!(
            details.kind,
            SectionKind::Collapsible {
                default_collapsed: false
            }
        );
        assert_eq!(details.content.plain_text(), "A & B");
        assert!(details.content.blocks[0].runs.last().unwrap().marks.bold);
        assert_eq!(details.children[0].content.plain_text(), "body");
        let title_only = collapsible_markdown(&RichDocument::paragraph("title"), "", true, 0);
        let parsed = parse_blocks(&title_only).unwrap();
        assert!(parsed.blocks[0].children.is_empty());
        let quoted = title_only
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = parse_blocks(&quoted).unwrap();
        assert_eq!(
            block_at(&parsed, &[0, 0]).kind,
            SectionKind::Collapsible {
                default_collapsed: true
            }
        );
    }

    #[test]
    fn disclosure_empty_typing_child_is_distinct_from_title_only() {
        let title = RichDocument::paragraph("title");
        for count in [0, 1] {
            let parsed = parse_blocks(&collapsible_markdown(&title, "", true, count)).unwrap();
            assert_eq!(parsed.blocks[0].children.len(), count);
            if count > 0 {
                assert_eq!(parsed.blocks[0].children[0].content.plain_text(), "");
            }
        }
    }

    #[test]
    fn details_keep_surrounding_blocks_and_external_reference_scope() {
        let source = "before\n\n<details>\n<summary>Title</summary>\n\n[body][id]\n\n</details>\n\nafter\n\n[id]: https://example.test \"title\"";
        let parsed = parse_blocks(source).unwrap();
        assert_eq!(parsed.blocks[0].content.plain_text(), "before");
        let details = &parsed.blocks[1];
        assert!(matches!(details.kind, SectionKind::Collapsible { .. }));
        let link = details.children[0].content.blocks[0].runs[0]
            .marks
            .link
            .as_ref()
            .unwrap();
        assert_eq!(link.url, "https://example.test");
        assert_eq!(link.title.as_deref(), Some("title"));
        assert_eq!(parsed.blocks[2].content.plain_text(), "after");
    }

    #[test]
    fn initial_disclosures_become_collapsible_list_items() {
        for open in ["", " open"] {
            let source = format!(
                "- <details{open}>\n  <summary>A &amp; <strong>B</strong></summary>\n\n  body\n\n  </details>\n- ordinary"
            );
            let parsed = parse_blocks(&source).unwrap();
            assert_eq!(parsed.blocks.len(), 1);
            let list = &parsed.blocks[0];
            assert_eq!(list.children.len(), 2);
            let item = &list.children[0];
            assert_eq!(
                item.kind,
                SectionKind::CollapsibleListItem {
                    default_collapsed: open.is_empty(),
                }
            );
            assert_eq!(item.content.plain_text(), "A & B");
            assert!(item.content.blocks[0].runs.last().unwrap().marks.bold);
            assert_eq!(item.children.len(), 1);
            assert_eq!(item.children[0].content.plain_text(), "body");
            assert_eq!(
                list.children[1].kind,
                SectionKind::ListItem { checked: None }
            );
            assert_eq!(list.children[1].content.plain_text(), "ordinary");
        }
    }

    #[test]
    fn disclosure_list_items_keep_body_and_trailing_item_children() {
        let source = "- <details>\n  <summary>Title</summary>\n\n  body\n\n  </details>\n\n  after\n\n  - nested\n- sibling";
        let parsed = parse_blocks(source).unwrap();
        let item = block_at(&parsed, &[0, 0]);
        assert!(matches!(item.kind, SectionKind::CollapsibleListItem { .. }));
        assert_eq!(item.content.plain_text(), "Title");
        assert_eq!(item.children.len(), 3);
        assert_eq!(item.children[0].content.plain_text(), "body");
        assert_eq!(item.children[1].content.plain_text(), "after");
        assert!(matches!(item.children[2].kind, SectionKind::List { .. }));
        assert_eq!(item.children[2].children[0].content.plain_text(), "nested");
        assert_eq!(block_at(&parsed, &[0, 1]).content.plain_text(), "sibling");

        // CommonMark can include the trailing text in the HTML AST node itself.
        for source in [
            "- <details><summary>Title</summary>body</details>\n  after",
            "- <details>\n  <summary>Title</summary>\n\n  body\n\n  </details>\n  after",
        ] {
            let parsed = parse_blocks(source).unwrap();
            let item = block_at(&parsed, &[0, 0]);
            assert_eq!(item.children.len(), 2, "{source:?}");
            assert_eq!(item.children[0].content.plain_text(), "body");
            assert_eq!(item.children[1].content.plain_text(), "after");
        }
    }

    #[test]
    fn disclosure_list_references_keep_summary_body_and_trailing_scope() {
        for separator in ["\n", "\n\n"] {
            let source = format!(
                "- <details open>\n  <summary>**[Title][id]**</summary>\n\n  [body][id]\n\n  </details>{separator}  [after][id]\n- [sibling][id]\n\n[id]: https://example.test \"reference\""
            );
            let parsed = parse_blocks(&source).unwrap();
            let item = block_at(&parsed, &[0, 0]);
            assert_eq!(item.content.plain_text(), "Title");
            assert!(item.content.blocks[0].runs[0].marks.bold);
            assert_eq!(item.children.len(), 2);
            assert_eq!(item.children[0].content.plain_text(), "body");
            assert_eq!(item.children[1].content.plain_text(), "after");
            for content in [
                &item.content,
                &item.children[0].content,
                &item.children[1].content,
                &block_at(&parsed, &[0, 1]).content,
            ] {
                let link = content.blocks[0].runs[0].marks.link.as_ref().unwrap();
                assert_eq!(link.url, "https://example.test");
                assert_eq!(link.title.as_deref(), Some("reference"));
                assert!(content.blocks.iter().any(|node| {
                    node.kind == RichKind::Definition && node.attrs["identifier"] == "id"
                }));
            }
        }
    }

    #[test]
    fn native_disclosure_lists_keep_rich_summaries_and_nested_mixed_groups() {
        let title =
            RichDocument::from_markdown("**Rich** `title` [link](https://example.test)").unwrap();
        let nested = collapsible_markdown(&RichDocument::paragraph("inner"), "inside", false, 1);
        let nested_item = format!("- {}", nested.replace('\n', "\n  "));
        let body = format!("- ordinary\n{nested_item}");
        let disclosure = collapsible_markdown(&title, &body, true, 1);
        let source = format!("> 3. {}\n> 4. last", disclosure.replace('\n', "\n>    "));
        let parsed = parse_blocks(&source).unwrap();
        let list = block_at(&parsed, &[0, 0]);
        assert_eq!(
            list.kind,
            SectionKind::List {
                ordered: true,
                start: 3,
            }
        );
        assert_eq!(list.children.len(), 2);
        let outer = &list.children[0];
        assert_eq!(
            outer.kind,
            SectionKind::CollapsibleListItem {
                default_collapsed: true,
            }
        );
        assert_eq!(outer.content.plain_text(), title.plain_text());
        assert!(outer.content.blocks[0].runs[0].marks.bold);
        assert!(
            outer.content.blocks[0]
                .runs
                .iter()
                .any(|run| run.marks.code)
        );
        assert!(
            outer.content.blocks[0]
                .runs
                .iter()
                .any(|run| run.marks.link.is_some())
        );
        assert_eq!(outer.children.len(), 1);
        let nested_list = &outer.children[0];
        assert!(matches!(nested_list.kind, SectionKind::List { .. }));
        assert_eq!(nested_list.children.len(), 2);
        assert_eq!(
            nested_list.children[0].kind,
            SectionKind::ListItem { checked: None }
        );
        assert_eq!(
            nested_list.children[1].kind,
            SectionKind::CollapsibleListItem {
                default_collapsed: false,
            }
        );
        assert_eq!(nested_list.children[1].content.plain_text(), "inner");
        assert_eq!(
            nested_list.children[1].children[0].content.plain_text(),
            "inside"
        );
        assert_eq!(list.children[1].content.plain_text(), "last");
    }

    #[test]
    fn disclosure_list_empty_titles_and_typing_bodies_remain_distinct() {
        for collapsed in [false, true] {
            for count in [0, 1] {
                let disclosure =
                    collapsible_markdown(&RichDocument::paragraph(""), "", collapsed, count);
                let parsed =
                    parse_blocks(&format!("- {}", disclosure.replace('\n', "\n  "))).unwrap();
                let item = block_at(&parsed, &[0, 0]);
                assert_eq!(
                    item.kind,
                    SectionKind::CollapsibleListItem {
                        default_collapsed: collapsed,
                    }
                );
                assert_eq!(item.content.plain_text(), "");
                assert_eq!(item.children.len(), count);
                if count > 0 {
                    assert_eq!(item.children[0].kind, SectionKind::Text);
                    assert_eq!(item.children[0].content.plain_text(), "");
                }
            }
        }
        let parsed = parse_blocks("- <details><summary></summary></details>").unwrap();
        let item = block_at(&parsed, &[0, 0]);
        assert!(matches!(item.kind, SectionKind::CollapsibleListItem { .. }));
        assert_eq!(item.content.plain_text(), "");
        assert!(item.children.is_empty());
    }

    #[test]
    fn ordinary_list_titles_keep_nested_standalone_disclosures() {
        let parsed = parse_blocks(
            "- ordinary\n\n  <details>\n  <summary>nested</summary>\n\n  body\n\n  </details>\n- sibling",
        )
        .unwrap();
        let item = block_at(&parsed, &[0, 0]);
        assert_eq!(item.kind, SectionKind::ListItem { checked: None });
        assert_eq!(item.content.plain_text(), "ordinary");
        assert_eq!(item.children.len(), 1);
        assert_eq!(
            item.children[0].kind,
            SectionKind::Collapsible {
                default_collapsed: true,
            }
        );
        assert_eq!(item.children[0].content.plain_text(), "nested");
        assert_eq!(item.children[0].children[0].content.plain_text(), "body");
        assert_eq!(block_at(&parsed, &[0, 1]).content.plain_text(), "sibling");
    }

    #[test]
    fn document_disclosure_lists_roundtrip_without_blank_wrappers() {
        let nested = collapsible_markdown(&RichDocument::paragraph("inner"), "", true, 1);
        let body = format!("- ordinary\n- {}", nested.replace('\n', "\n  "));
        let title = RichDocument::from_markdown("**Title** [link](https://example.test)").unwrap();
        let disclosure = collapsible_markdown(&title, &body, false, 1);
        let source = format!("- {}\n- sibling", disclosure.replace('\n', "\n  "));
        let mut document = crate::document::Document::new();
        document.insert_section(None, &source).unwrap();
        let markdown = document.markdown();
        let mut restored = crate::document::Document::new();
        restored.insert_section(None, &markdown).unwrap();
        let shape = |document: &crate::document::Document| {
            document
                .sections()
                .into_iter()
                .map(|section| (section.depth, section.kind, section.content.plain_text()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&document), shape(&restored));
        assert_eq!(restored.markdown(), markdown);
        let sections = restored.sections();
        assert_eq!(
            sections
                .iter()
                .filter(|section| matches!(section.kind, SectionKind::CollapsibleListItem { .. }))
                .count(),
            2
        );
        let outer = &sections[1];
        assert_eq!(outer.content.plain_text(), title.plain_text());
        assert!(outer.content.blocks[0].runs[0].marks.bold);
        assert!(
            outer.content.blocks[0]
                .runs
                .iter()
                .any(|run| run.marks.link.is_some())
        );
    }
}
