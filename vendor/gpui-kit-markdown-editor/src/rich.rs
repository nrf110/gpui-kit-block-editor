//! Versioned semantic editor content. Markdown is an import/export format only.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const RICH_DOCUMENT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RichDocument {
    pub version: u32,
    pub blocks: Vec<RichNode>,
}

impl Default for RichDocument {
    fn default() -> Self {
        Self {
            version: RICH_DOCUMENT_VERSION,
            blocks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RichNode {
    pub id: String,
    pub kind: RichKind,
    #[serde(default)]
    pub attrs: BTreeMap<String, Value>,
    #[serde(default)]
    pub runs: Vec<RichRun>,
    #[serde(default)]
    pub children: Vec<RichNode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichKind {
    Paragraph,
    Heading,
    CodeBlock,
    List,
    ListItem,
    Quote,
    Table,
    TableRow,
    TableCell,
    HorizontalRule,
    Break,
    Definition,
    Custom,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RichRun {
    pub text: String,
    pub marks: RichMarks,
    pub atom: Option<RichAtom>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RichMarks {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub code: bool,
    /// Hue, saturation, lightness, alpha; each component is in 0..=1.
    pub highlight: Option<[f32; 4]>,
    pub link: Option<RichLink>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RichLink {
    pub url: String,
    pub identifier: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RichAtom {
    Image {
        url: String,
        link: Option<RichLink>,
        title: Option<String>,
        alt: Option<String>,
        width: Option<RichLength>,
        height: Option<RichLength>,
    },
    Custom {
        name: String,
        text: String,
        markdown: String,
        accessibility_label: String,
        /// Lossless `{ "codec": ..., "data": ... }` payload envelope.
        /// Built-ins are `unit`, `json`, and `string`; `registered` uses
        /// the serde codec registered for this atom's name.
        payload: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", content = "value", rename_all = "snake_case")]
/// Relative values are fractions of the parent size, not percentages.
pub enum RichLength {
    Pixels(f32),
    Rems(f32),
    Relative(f32),
}

impl RichDocument {
    pub fn paragraph(text: &str) -> Self {
        Self {
            version: RICH_DOCUMENT_VERSION,
            blocks: vec![RichNode::paragraph(text)],
        }
    }

    pub fn from_markdown(source: &str) -> Result<Self, gpui::SharedString> {
        crate::BlockEditorModel::from_markdown(
            source,
            gpui_kit::base::text::MarkdownExtensions::default(),
        )?
        .rich_document()
    }

    pub fn to_markdown(&self) -> Result<String, gpui::SharedString> {
        Ok(crate::BlockEditorModel::from_rich(
            self,
            gpui_kit::base::text::MarkdownExtensions::default(),
        )?
        .source())
    }

    pub fn plain_text(&self) -> String {
        fn leaves(node: &RichNode, out: &mut String, first: &mut bool) {
            if node.kind == RichKind::Definition {
                return;
            }
            if !node.runs.is_empty() || node.children.is_empty() {
                if !*first {
                    out.push('\n');
                }
                *first = false;
                for run in &node.runs {
                    out.push_str(&run.text);
                }
            }
            for child in &node.children {
                leaves(child, out, first);
            }
        }
        let mut out = String::new();
        let mut first = true;
        for node in &self.blocks {
            leaves(node, &mut out, &mut first);
        }
        out
    }

    pub fn validate(&self) -> Result<(), gpui::SharedString> {
        crate::rich_bridge::validate(self)
    }
}

impl RichNode {
    pub fn paragraph(text: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: RichKind::Paragraph,
            attrs: BTreeMap::new(),
            runs: if text.is_empty() {
                Vec::new()
            } else {
                vec![RichRun {
                    text: text.to_owned(),
                    ..Default::default()
                }]
            },
            children: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        for run in &self.runs {
            text.push_str(&run.text);
        }
        text
    }
}

pub use crate::rich_bridge::register_custom_codec;
