use crate::native_editor::rich::{RichDocument, RichKind, RichNode};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// Structural role of a collaborative section. Editable sections own native
/// rich content; containers own child ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SectionKind {
    Text,
    List { ordered: bool, start: u64 },
    ListItem { checked: Option<bool> },
    CollapsibleListItem { default_collapsed: bool },
    Quote,
    Collapsible { default_collapsed: bool },
}

impl SectionKind {
    pub fn is_editable(&self) -> bool {
        matches!(
            self,
            Self::Text
                | Self::ListItem { .. }
                | Self::CollapsibleListItem { .. }
                | Self::Collapsible { .. }
        )
    }

    pub fn is_list_item(&self) -> bool {
        matches!(
            self,
            Self::ListItem { .. } | Self::CollapsibleListItem { .. }
        )
    }

    pub fn default_collapsed(&self) -> Option<bool> {
        match self {
            Self::Collapsible { default_collapsed }
            | Self::CollapsibleListItem { default_collapsed } => Some(*default_collapsed),
            _ => None,
        }
    }
}

/// Semantic section tree before stable CRDT section identities are assigned.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSpec {
    pub kind: SectionKind,
    pub content: RichDocument,
    pub children: Vec<BlockSpec>,
}

/// Serialize a semantic body at the Markdown boundary only.
pub fn content_markdown(content: &RichDocument) -> String {
    content
        .to_markdown()
        .expect("validated rich content must export to Markdown")
}

/// Lift native structural nodes directly, without a Markdown intermediate.
pub fn specs_from_rich(content: &RichDocument) -> Result<Vec<BlockSpec>> {
    content
        .validate()
        .map_err(|error| anyhow!(error.to_string()))?;
    let mut specs = lift_nodes(&content.blocks)?;
    if specs.is_empty() {
        specs.push(BlockSpec {
            kind: SectionKind::Text,
            content: RichDocument::paragraph(""),
            children: Vec::new(),
        });
    }
    Ok(specs)
}

fn document(blocks: Vec<RichNode>) -> RichDocument {
    RichDocument {
        blocks,
        ..RichDocument::default()
    }
}

fn lift_nodes(nodes: &[RichNode]) -> Result<Vec<BlockSpec>> {
    let mut specs: Vec<BlockSpec> = Vec::new();
    let mut definitions = Vec::new();
    for node in nodes {
        if node.kind == RichKind::Definition {
            definitions.push(node.clone());
            continue;
        }
        specs.push(lift_node(node)?);
    }
    if !definitions.is_empty() {
        // Definitions are invisible metadata, never an uneditable-only body.
        // Keep them beside a real editable node rather than dropping identity.
        fn editable(specs: &mut [BlockSpec]) -> Option<&mut RichDocument> {
            for spec in specs {
                if spec.kind.is_editable() {
                    return Some(&mut spec.content);
                }
                if let Some(body) = editable(&mut spec.children) {
                    return Some(body);
                }
            }
            None
        }
        fn references(node: &RichNode, identifier: &str) -> bool {
            node.runs.iter().any(|run| {
                run.marks
                    .link
                    .as_ref()
                    .is_some_and(|link| link.identifier.as_deref() == Some(identifier))
            }) || node
                .children
                .iter()
                .any(|child| references(child, identifier))
        }
        fn referencing<'a>(
            specs: &'a mut [BlockSpec],
            identifier: &str,
        ) -> Option<&'a mut RichDocument> {
            for spec in specs {
                if spec.kind.is_editable()
                    && spec
                        .content
                        .blocks
                        .iter()
                        .any(|node| references(node, identifier))
                {
                    return Some(&mut spec.content);
                }
                if let Some(body) = referencing(&mut spec.children, identifier) {
                    return Some(body);
                }
            }
            None
        }
        for definition in definitions {
            let referenced = definition
                .attrs
                .get("identifier")
                .and_then(serde_json::Value::as_str)
                .and_then(|identifier| referencing(&mut specs, identifier));
            if let Some(body) = referenced {
                body.blocks.push(definition);
            } else if let Some(body) = editable(&mut specs) {
                body.blocks.push(definition);
            } else {
                let mut body = RichDocument::paragraph("");
                body.blocks.push(definition);
                specs.push(BlockSpec {
                    kind: SectionKind::Text,
                    content: body,
                    children: Vec::new(),
                });
            }
        }
    }
    Ok(specs)
}

fn lift_node(node: &RichNode) -> Result<BlockSpec> {
    match node.kind {
        RichKind::List | RichKind::Quote => {
            let kind = if node.kind == RichKind::List {
                SectionKind::List {
                    ordered: node
                        .attrs
                        .get("ordered")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    start: node
                        .attrs
                        .get("start")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1),
                }
            } else {
                SectionKind::Quote
            };
            let skeleton = RichNode {
                id: node.id.clone(),
                kind: node.kind,
                attrs: node.attrs.clone(),
                runs: node.runs.clone(),
                children: Vec::new(),
            };
            let mut children = lift_nodes(&node.children)?;
            if children.is_empty() {
                children.push(BlockSpec {
                    kind: if node.kind == RichKind::List {
                        SectionKind::ListItem { checked: None }
                    } else {
                        SectionKind::Text
                    },
                    content: RichDocument::paragraph(""),
                    children: Vec::new(),
                });
            }
            Ok(BlockSpec {
                kind,
                content: document(vec![skeleton]),
                children,
            })
        }
        RichKind::ListItem => {
            let checked = node
                .attrs
                .get("checked")
                .and_then(serde_json::Value::as_bool);
            anyhow::ensure!(
                node.runs.is_empty(),
                "List-item content must be represented by native child blocks"
            );
            let mut children = lift_nodes(&node.children)?;
            if children
                .first()
                .is_some_and(|child| child.kind == SectionKind::Text)
            {
                let mut first = children.remove(0);
                first.content.blocks[0].attrs.insert(
                    "section_list_item".into(),
                    serde_json::json!({ "id": node.id, "attrs": node.attrs }),
                );
                first.kind = SectionKind::ListItem { checked };
                first.children.extend(children);
                return Ok(first);
            }
            let mut attrs = node.attrs.clone();
            attrs.remove("checked");
            let paragraph = RichNode {
                id: node.id.clone(),
                kind: RichKind::Paragraph,
                attrs,
                runs: Vec::new(),
                children: Vec::new(),
            };
            Ok(BlockSpec {
                kind: SectionKind::ListItem { checked },
                content: document(vec![paragraph]),
                children,
            })
        }
        _ => Ok(BlockSpec {
            kind: SectionKind::Text,
            content: document(vec![node.clone()]),
            children: Vec::new(),
        }),
    }
}
