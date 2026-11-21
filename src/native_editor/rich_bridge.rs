// Modified for gpui-kit-block-editor: integrated from gpui-kit-markdown-editor
// as a private native-editor module; original implementation retained.
//! Direct semantic DTO ↔ native tree bridge. Never parses or serializes Markdown.
use crate::native_editor::{
    BlockEditorModel, BlockId, BlockSelection,
    document::{AtomData, BlockContent, EditorDocument},
    inline::{ATOM_CHAR, InlineAtom, InlineText},
    kind::BlockKindData,
    rich::*,
    step::DetachedBlock,
};
use gpui::{AbsoluteLength, DefiniteLength, Hsla, SharedString};
use gpui_kit::base::text::{
    MarkdownExtensions, MarkdownNode,
    editor_host::{ColumnAlign, ImageNode, LinkMark, TextMark},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, LazyLock, RwLock},
};

type Result<T, E = SharedString> = std::result::Result<T, E>;
fn error(message: impl Into<String>) -> SharedString {
    message.into().into()
}

type Encoder = dyn Fn(&MarkdownNode) -> Result<Value> + Send + Sync;
type Decoder = dyn Fn(&str, &Value) -> Result<MarkdownNode> + Send + Sync;
struct Codec {
    encode: Box<Encoder>,
    decode: Box<Decoder>,
}
static CODECS: LazyLock<RwLock<HashMap<String, Arc<Codec>>>> = LazyLock::new(Default::default);
fn codecs() -> &'static RwLock<HashMap<String, Arc<Codec>>> {
    &CODECS
}

/// Register a lossless serde codec for the typed `MarkdownNode` payload named `name`.
/// Registration is process-wide, and duplicate names are rejected.
pub fn register_custom_codec<T>(name: &str) -> Result<()>
where
    T: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    if name.is_empty() {
        return Err(error("custom codec name is empty"));
    }
    let mut registry = codecs()
        .write()
        .map_err(|_| error("custom codec registry poisoned"))?;
    if registry.contains_key(name) {
        return Err(error(format!("custom codec already registered: {name}")));
    }
    registry.insert(
        name.to_owned(),
        Arc::new(Codec {
            encode: Box::new(|node| {
                let data = node.data::<T>().ok_or_else(|| {
                    error(format!("custom payload type mismatch: {}", node.name()))
                })?;
                serde_json::to_value(data).map_err(|e| error(e.to_string()))
            }),
            decode: Box::new(|name, value| {
                let data: T =
                    serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
                Ok(MarkdownNode::new(name.to_owned(), data))
            }),
        }),
    );
    Ok(())
}
fn codec(name: &str) -> Result<Option<Arc<Codec>>> {
    Ok(codecs()
        .read()
        .map_err(|_| error("custom codec registry poisoned"))?
        .get(name)
        .cloned())
}
fn export_custom(node: &MarkdownNode) -> Result<RichAtom> {
    let payload = if let Some(codec) = codec(node.name())? {
        json!({"codec":"registered", "data": (codec.encode)(node)?})
    } else if node.data::<()>().is_some() {
        json!({"codec":"unit", "data":null})
    } else if let Some(data) = node.data::<Value>() {
        json!({"codec":"json", "data":data})
    } else if let Some(data) = node.data::<String>() {
        json!({"codec":"string", "data":data})
    } else {
        return Err(error(format!(
            "custom payload '{}' requires a registered rich codec",
            node.name()
        )));
    };
    Ok(RichAtom::Custom {
        name: node.name().to_owned(),
        text: node.as_text().to_owned(),
        markdown: node.as_markdown().to_owned(),
        accessibility_label: node.accessibility_name().to_owned(),
        payload,
    })
}
fn import_custom(atom: &RichAtom) -> Result<MarkdownNode> {
    let RichAtom::Custom {
        name,
        text,
        markdown,
        accessibility_label,
        payload,
    } = atom
    else {
        return Err(error("expected custom atom"));
    };
    if name.is_empty() {
        return Err(error("custom atom name is empty"));
    }
    let data = payload
        .get("data")
        .ok_or_else(|| error("custom atom payload has no data"))?;
    let node = match payload.get("codec").and_then(Value::as_str) {
        Some("unit") if data.is_null() => MarkdownNode::new(name.clone(), ()),
        Some("json") => MarkdownNode::new(name.clone(), data.clone()),
        Some("string") => MarkdownNode::new(
            name.clone(),
            data.as_str()
                .ok_or_else(|| error("custom string payload is not a string"))?
                .to_owned(),
        ),
        Some("registered") => (codec(name)?
            .ok_or_else(|| error(format!("missing rich codec for '{name}'")))?
            .decode)(name, data)?,
        _ => return Err(error("unsupported custom atom payload codec")),
    };
    Ok(node
        .text(text.clone())
        .markdown(markdown.clone())
        .accessibility_label(accessibility_label.clone()))
}

fn native_link(link: &RichLink) -> LinkMark {
    LinkMark {
        url: link.url.clone().into(),
        identifier: link.identifier.clone().map(Into::into),
        title: link.title.clone().map(Into::into),
    }
}
fn rich_link(link: &LinkMark) -> RichLink {
    RichLink {
        url: link.url.to_string(),
        identifier: link.identifier.as_ref().map(ToString::to_string),
        title: link.title.as_ref().map(ToString::to_string),
    }
}
fn native_marks(m: &RichMarks) -> TextMark {
    TextMark {
        bold: m.bold,
        italic: m.italic,
        strikethrough: m.strikethrough,
        underline: m.underline,
        code: m.code,
        highlight: m.highlight.map(|[h, s, l, a]| Hsla { h, s, l, a }),
        link: m.link.as_ref().map(native_link),
    }
}
fn rich_marks(m: &TextMark) -> RichMarks {
    RichMarks {
        bold: m.bold,
        italic: m.italic,
        strikethrough: m.strikethrough,
        underline: m.underline,
        code: m.code,
        highlight: m.highlight.map(|c| [c.h, c.s, c.l, c.a]),
        link: m.link.as_ref().map(rich_link),
    }
}
fn rich_length(length: DefiniteLength) -> RichLength {
    match length {
        DefiniteLength::Absolute(AbsoluteLength::Pixels(v)) => RichLength::Pixels(v.into()),
        DefiniteLength::Absolute(AbsoluteLength::Rems(v)) => RichLength::Rems(v.0),
        DefiniteLength::Fraction(v) => RichLength::Relative(v),
    }
}
fn native_length(length: &RichLength) -> DefiniteLength {
    match length {
        RichLength::Pixels(v) => gpui::px(*v).into(),
        RichLength::Rems(v) => gpui::rems(*v).into(),
        RichLength::Relative(v) => DefiniteLength::Fraction(*v),
    }
}
fn rich_atom(atom: &InlineAtom) -> Result<RichAtom> {
    match atom {
        InlineAtom::Custom(node) => export_custom(node),
        InlineAtom::Image(i) => Ok(RichAtom::Image {
            url: i.url.to_string(),
            link: i.link.as_ref().map(rich_link),
            title: i.title.as_ref().map(ToString::to_string),
            alt: i.alt.as_ref().map(ToString::to_string),
            width: i.width.map(rich_length),
            height: i.height.map(rich_length),
        }),
    }
}
fn native_atom(atom: &RichAtom) -> Result<InlineAtom> {
    match atom {
        RichAtom::Custom { .. } => Ok(InlineAtom::Custom(import_custom(atom)?)),
        RichAtom::Image {
            url,
            link,
            title,
            alt,
            width,
            height,
        } => {
            let mut image = ImageNode::default();
            image.url = url.clone().into();
            image.link = link.as_ref().map(native_link);
            image.title = title.clone().map(Into::into);
            image.alt = alt.clone().map(Into::into);
            image.width = width.as_ref().map(native_length);
            image.height = height.as_ref().map(native_length);
            Ok(InlineAtom::Image(image))
        }
    }
}
fn inline(runs: &[RichRun]) -> Result<InlineText> {
    let mut out = InlineText::new();
    for run in runs {
        out.push(match &run.atom {
            Some(atom) => InlineText::atom(native_atom(atom)?, native_marks(&run.marks)),
            None => InlineText::plain(&run.text, native_marks(&run.marks)),
        });
    }
    Ok(out)
}

fn string(node: &RichNode, key: &str) -> Result<Option<SharedString>> {
    match node.attrs.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone().into())),
        _ => Err(error(format!("{key} must be a string"))),
    }
}
fn boolean(node: &RichNode, key: &str) -> Result<Option<bool>> {
    match node.attrs.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(error(format!("{key} must be a boolean"))),
    }
}
fn number(node: &RichNode, key: &str) -> Result<Option<u32>> {
    match node.attrs.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| error(format!("{key} must be an unsigned 32-bit integer"))),
    }
}
fn kind(node: &RichNode) -> Result<BlockKindData> {
    Ok(match node.kind {
        RichKind::Paragraph => BlockKindData::Paragraph,
        RichKind::Heading => {
            let level = number(node, "level")?.unwrap_or(1);
            if !(1..=6).contains(&level) {
                return Err(error("heading level must be 1..=6"));
            }
            BlockKindData::Heading { level: level as u8 }
        }
        RichKind::CodeBlock => BlockKindData::CodeBlock {
            lang: string(node, "lang")?,
            meta: string(node, "meta")?,
        },
        RichKind::List => BlockKindData::List {
            ordered: boolean(node, "ordered")?.unwrap_or(false),
            start: number(node, "start")?,
        },
        RichKind::ListItem => BlockKindData::ListItem {
            checked: boolean(node, "checked")?,
            spread: boolean(node, "spread")?.unwrap_or(false),
        },
        RichKind::Quote => BlockKindData::Blockquote,
        RichKind::Table => {
            let aligns = match node.attrs.get("aligns") {
                None => Vec::new(),
                Some(Value::Array(aligns)) => aligns
                    .iter()
                    .map(|align| match align.as_str() {
                        Some("none") => Ok(ColumnAlign::None),
                        Some("left") => Ok(ColumnAlign::Left),
                        Some("center") => Ok(ColumnAlign::Center),
                        Some("right") => Ok(ColumnAlign::Right),
                        _ => Err(error("invalid table alignment")),
                    })
                    .collect::<Result<Vec<_>>>()?,
                _ => return Err(error("table aligns must be an array")),
            };
            BlockKindData::Table { aligns }
        }
        RichKind::TableRow => BlockKindData::TableRow,
        RichKind::TableCell => BlockKindData::TableCell,
        RichKind::HorizontalRule => BlockKindData::HorizontalRule,
        RichKind::Break => BlockKindData::Break {
            html: boolean(node, "html")?.unwrap_or(false),
        },
        RichKind::Definition => BlockKindData::Definition {
            identifier: string(node, "identifier")?
                .ok_or_else(|| error("definition requires identifier"))?,
            url: string(node, "url")?.ok_or_else(|| error("definition requires url"))?,
            title: string(node, "title")?,
        },
        RichKind::Custom => BlockKindData::Custom,
        RichKind::Unknown => BlockKindData::Unknown,
    })
}
fn known_attrs(kind: RichKind) -> &'static [&'static str] {
    match kind {
        RichKind::Heading => &["level"],
        RichKind::CodeBlock => &["lang", "meta"],
        RichKind::List => &["ordered", "start"],
        RichKind::ListItem => &["checked", "spread"],
        RichKind::Table => &["aligns"],
        RichKind::Break => &["html"],
        RichKind::Definition => &["identifier", "url", "title"],
        _ => &[],
    }
}

pub(crate) fn validate_run(run: &RichRun) -> Result<()> {
    if let Some(color) = run.marks.highlight {
        if color
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(error(
                "highlight HSLA components must be finite values in 0..=1",
            ));
        }
    }
    if let Some(atom) = &run.atom {
        if run.text != "\u{FFFC}" {
            return Err(error("atom run must contain exactly U+FFFC"));
        }
        match atom {
            RichAtom::Image { width, height, .. } => {
                for length in [width, height].into_iter().flatten() {
                    let (RichLength::Pixels(v) | RichLength::Rems(v) | RichLength::Relative(v)) =
                        length;
                    if !v.is_finite() || *v < 0.0 {
                        return Err(error("image dimensions must be finite and nonnegative"));
                    }
                }
            }
            RichAtom::Custom { .. } => {
                import_custom(atom)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate(document: &RichDocument) -> Result<()> {
    if document.version != RICH_DOCUMENT_VERSION {
        return Err(error(format!(
            "unsupported rich document version {}",
            document.version
        )));
    }
    fn node(n: &RichNode, parent: Option<RichKind>, ids: &mut HashSet<String>) -> Result<()> {
        if n.id.is_empty() || !ids.insert(n.id.clone()) {
            return Err(error("empty or duplicate rich node ID"));
        }
        let native = kind(n)?;
        if matches!(n.kind, RichKind::ListItem) && parent != Some(RichKind::List) {
            return Err(error("list item must belong to a list"));
        }
        if matches!(n.kind, RichKind::TableRow) && parent != Some(RichKind::Table) {
            return Err(error("table row must belong to a table"));
        }
        if matches!(n.kind, RichKind::TableCell) && parent != Some(RichKind::TableRow) {
            return Err(error("table cell must belong to a table row"));
        }
        if native.is_container() {
            if !n.runs.is_empty() {
                return Err(error("container cannot hold inline runs"));
            }
            if n.children.is_empty()
                && !matches!(
                    n.kind,
                    RichKind::List | RichKind::Quote | RichKind::Table | RichKind::TableRow
                )
            {
                return Err(error("empty rich container"));
            }
            if n.kind == RichKind::List && n.children.iter().any(|c| c.kind != RichKind::ListItem) {
                return Err(error("list holds a non-item"));
            }
            if n.kind == RichKind::Table && n.children.iter().any(|c| c.kind != RichKind::TableRow)
            {
                return Err(error("table holds a non-row"));
            }
            if n.kind == RichKind::TableRow
                && n.children.iter().any(|c| c.kind != RichKind::TableCell)
            {
                return Err(error("table row holds a non-cell"));
            }
            if n.kind == RichKind::ListItem
                && n.children.first().is_none_or(|c| {
                    matches!(
                        c.kind,
                        RichKind::List
                            | RichKind::ListItem
                            | RichKind::Quote
                            | RichKind::Table
                            | RichKind::TableRow
                    )
                })
            {
                return Err(error("list item must start with a leaf"));
            }
        } else if !n.children.is_empty() {
            return Err(error("leaf cannot hold children"));
        }
        for run in &n.runs {
            validate_run(run)?;
            if run.atom.is_none() && run.text.contains(ATOM_CHAR) && n.kind != RichKind::CodeBlock {
                return Err(error("U+FFFC requires atom metadata"));
            }
            match n.kind {
                RichKind::CodeBlock if run.atom.is_some() || run.marks != RichMarks::default() => {
                    return Err(error("code block runs cannot have marks or atoms"));
                }
                RichKind::Custom
                    if !matches!(&run.atom, Some(RichAtom::Custom { .. }))
                        || run.marks != RichMarks::default() =>
                {
                    return Err(error("custom block requires an unmarked custom atom"));
                }
                RichKind::HorizontalRule
                | RichKind::Break
                | RichKind::Definition
                | RichKind::Unknown => return Err(error("atomic block does not accept runs")),
                _ => {}
            }
        }
        if n.kind == RichKind::Custom && n.runs.len() != 1 {
            return Err(error("custom block requires exactly one atom"));
        }
        for child in &n.children {
            node(child, Some(n.kind), ids)?;
        }
        Ok(())
    }
    let mut ids = HashSet::new();
    for n in &document.blocks {
        node(n, None, &mut ids)?;
    }
    Ok(())
}

fn import_document(
    document: &RichDocument,
    extensions: Arc<MarkdownExtensions>,
    prior: &HashMap<String, BlockId>,
) -> Result<(
    EditorDocument,
    HashMap<BlockId, String>,
    HashMap<BlockId, BTreeMap<String, Value>>,
)> {
    document.validate()?;
    fn block(
        n: &RichNode,
        prior: &HashMap<String, BlockId>,
        ids: &mut HashMap<BlockId, String>,
        attrs: &mut HashMap<BlockId, BTreeMap<String, Value>>,
    ) -> Result<DetachedBlock> {
        let native = kind(n)?;
        let mut detached = if native.is_container() {
            if n.children.is_empty() && !matches!(n.kind, RichKind::Table | RichKind::TableRow) {
                return Err(error(
                    "empty section metadata container cannot be mounted in a native editor",
                ));
            }
            DetachedBlock::container(
                native,
                n.children
                    .iter()
                    .map(|n| block(n, prior, ids, attrs))
                    .collect::<Result<Vec<_>>>()?,
            )
        } else {
            let content = match n.kind {
                RichKind::Paragraph | RichKind::Heading | RichKind::TableCell => {
                    BlockContent::Inline(inline(&n.runs)?)
                }
                RichKind::CodeBlock => BlockContent::Plain(n.text()),
                RichKind::Custom => BlockContent::Atom(AtomData::Custom(import_custom(
                    n.runs[0].atom.as_ref().unwrap(),
                )?)),
                _ => BlockContent::Atom(AtomData::None),
            };
            DetachedBlock::leaf(native, content)
        };
        if let Some(id) = prior.get(&n.id) {
            detached.id = *id;
        }
        ids.insert(detached.id, n.id.clone());
        let mut extra = n.attrs.clone();
        for key in known_attrs(n.kind) {
            extra.remove(*key);
        }
        attrs.insert(detached.id, extra);
        Ok(detached)
    }
    let mut doc = EditorDocument::empty(extensions);
    let mut ids = HashMap::new();
    let mut attrs = HashMap::new();
    if !document.blocks.is_empty() {
        doc.detach(doc.root, 0);
        for (ix, n) in document.blocks.iter().enumerate() {
            let detached = block(n, prior, &mut ids, &mut attrs)?;
            doc.attach(doc.root, ix, detached);
        }
        doc.refresh_order();
    }
    doc.check_invariants().map_err(error)?;
    Ok((doc, ids, attrs))
}

impl BlockEditorModel {
    pub fn from_rich(document: &RichDocument, extensions: MarkdownExtensions) -> Result<Self> {
        let extensions = crate::native_editor::import::preserve_source_extensions(extensions);
        let (doc, ids, attrs) = import_document(document, Arc::new(extensions), &HashMap::new())?;
        let mut model = Self::from_document(doc);
        model.rich_ids = std::cell::RefCell::new(ids);
        model.rich_attrs = attrs;
        if model.doc.walk_leaves().is_empty() {
            let block = model.doc.children(model.doc.root)[0];
            model.selection = BlockSelection::Blocks {
                anchor: block,
                head: block,
            };
        }
        Ok(model)
    }

    /// Replace semantic content atomically, retaining native IDs for matching stable IDs.
    pub fn set_document(&mut self, document: &RichDocument) -> Result<()> {
        let prior = self
            .rich_ids
            .borrow()
            .iter()
            .map(|(id, stable)| (stable.clone(), *id))
            .collect();
        let (mut doc, ids, attrs) = import_document(document, self.doc.extensions.clone(), &prior)?;
        doc.revision = self.doc.revision + 1;
        self.doc = doc;
        self.rich_ids = std::cell::RefCell::new(ids);
        self.rich_attrs = attrs;
        self.selection = if self.doc.walk_leaves().is_empty() {
            let block = self.doc.children(self.doc.root)[0];
            BlockSelection::Blocks {
                anchor: block,
                head: block,
            }
        } else {
            BlockSelection::caret(self.doc.first_caret())
        };
        self.selection_epoch += 1;
        self.stored_marks = None;
        self.history.clear();
        *self.source_cache.borrow_mut() = None;
        *self.prefix_cache.borrow_mut() = None;
        Ok(())
    }

    pub fn stable_id(&self, block: BlockId) -> Option<String> {
        self.doc.get(block)?;
        self.rich_ids.borrow().get(&block).cloned()
    }
    pub fn block_id(&self, stable: &str) -> Option<BlockId> {
        self.rich_ids
            .borrow()
            .iter()
            .find_map(|(id, value)| (value == stable && self.doc.get(*id).is_some()).then_some(*id))
    }
    pub fn rich_document(&self) -> Result<RichDocument> {
        fn block(
            model: &BlockEditorModel,
            id: BlockId,
            ids: &mut HashMap<BlockId, String>,
        ) -> Result<RichNode> {
            let entry = model
                .doc
                .get(id)
                .ok_or_else(|| error("missing native block"))?;
            let stable = ids
                .entry(id)
                .or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone();
            let mut attrs = model.rich_attrs.get(&id).cloned().unwrap_or_default();
            let mut attr = |key: &str, value: Value| {
                attrs.insert(key.to_owned(), value);
            };
            let kind = match &entry.kind {
                BlockKindData::Paragraph => RichKind::Paragraph,
                BlockKindData::Heading { level } => {
                    attr("level", json!(level));
                    RichKind::Heading
                }
                BlockKindData::CodeBlock { lang, meta } => {
                    if let Some(v) = lang {
                        attr("lang", json!(v.as_ref()));
                    }
                    if let Some(v) = meta {
                        attr("meta", json!(v.as_ref()));
                    }
                    RichKind::CodeBlock
                }
                BlockKindData::Blockquote => RichKind::Quote,
                BlockKindData::List { ordered, start } => {
                    attr("ordered", json!(ordered));
                    if let Some(v) = start {
                        attr("start", json!(v));
                    }
                    RichKind::List
                }
                BlockKindData::ListItem { checked, spread } => {
                    if let Some(v) = checked {
                        attr("checked", json!(v));
                    }
                    attr("spread", json!(spread));
                    RichKind::ListItem
                }
                BlockKindData::Table { aligns } => {
                    attr(
                        "aligns",
                        json!(
                            aligns
                                .iter()
                                .map(|v| match v {
                                    ColumnAlign::None => "none",
                                    ColumnAlign::Left => "left",
                                    ColumnAlign::Center => "center",
                                    ColumnAlign::Right => "right",
                                })
                                .collect::<Vec<_>>()
                        ),
                    );
                    RichKind::Table
                }
                BlockKindData::TableRow => RichKind::TableRow,
                BlockKindData::TableCell => RichKind::TableCell,
                BlockKindData::HorizontalRule => RichKind::HorizontalRule,
                BlockKindData::Break { html } => {
                    attr("html", json!(html));
                    RichKind::Break
                }
                BlockKindData::Definition {
                    identifier,
                    url,
                    title,
                } => {
                    attr("identifier", json!(identifier.as_ref()));
                    attr("url", json!(url.as_ref()));
                    if let Some(v) = title {
                        attr("title", json!(v.as_ref()));
                    }
                    RichKind::Definition
                }
                BlockKindData::Custom => RichKind::Custom,
                BlockKindData::Unknown => RichKind::Unknown,
                BlockKindData::Root => return Err(error("root cannot be a rich node")),
            };
            let mut runs = Vec::new();
            let mut children = Vec::new();
            match &entry.content {
                BlockContent::Children(native) => {
                    for id in native {
                        children.push(block(model, *id, ids)?);
                    }
                }
                BlockContent::Inline(inline) => {
                    for (range, marks, atom) in inline.pieces() {
                        runs.push(RichRun {
                            text: inline.text()[range].to_owned(),
                            marks: rich_marks(marks),
                            atom: atom.map(rich_atom).transpose()?,
                        });
                    }
                }
                BlockContent::Plain(text) if !text.is_empty() => runs.push(RichRun {
                    text: text.clone(),
                    ..Default::default()
                }),
                BlockContent::Atom(AtomData::Custom(node)) => runs.push(RichRun {
                    text: ATOM_CHAR.to_string(),
                    atom: Some(export_custom(node)?),
                    ..Default::default()
                }),
                _ => {}
            }
            Ok(RichNode {
                id: stable,
                kind,
                attrs,
                runs,
                children,
            })
        }
        let mut ids = self.rich_ids.borrow_mut();
        let blocks = self
            .doc
            .children(self.doc.root)
            .iter()
            .map(|id| block(self, *id, &mut ids))
            .collect::<Result<Vec<_>>>()?;
        Ok(RichDocument {
            version: RICH_DOCUMENT_VERSION,
            blocks,
        })
    }
}
