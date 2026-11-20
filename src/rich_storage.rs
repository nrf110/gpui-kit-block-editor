//! Semantic Yjs storage. Every node owns a permanent attributed text leaf;
//! metadata keys and marks are independent registers, never a document blob.
use super::*;
use markdown_editor::rich::{RichMarks, RichNode, RichRun};
use serde_json::Value;
use std::collections::BTreeMap;
use yrs::types::Attrs;

pub(super) const SCHEMA: &str = "gpui-kit-block-editor";

pub(super) fn initialize_schema(doc: &Doc) {
    let seed = Doc::with_client_id(0);
    let meta = seed.get_or_insert_map("meta");
    {
        let mut txn = seed.transact_mut();
        meta.insert(&mut txn, "schema", SCHEMA);
        meta.insert(&mut txn, "version", i64::from(RICH_DOCUMENT_VERSION));
    }
    let update = seed
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    doc.transact_mut()
        .apply_update(Update::decode_v1(&update).expect("schema update"))
        .expect("schema seed is valid");
}

pub(super) fn body<T: ReadTxn>(section: &MapRef, txn: &T) -> Result<MapRef> {
    match section.get(txn, "body") {
        Some(Out::YMap(map)) => Ok(map),
        _ => bail!("section has no semantic body"),
    }
}

fn nodes<T: ReadTxn>(body: &MapRef, txn: &T) -> Result<MapRef> {
    match body.get(txn, "nodes") {
        Some(Out::YMap(map)) => Ok(map),
        _ => bail!("semantic body has no node map"),
    }
}

pub(super) fn node_text<T: ReadTxn>(section: &MapRef, txn: &T, id: &str) -> Result<TextRef> {
    let body = body(section, txn)?;
    let nodes = nodes(&body, txn)?;
    let Some(Out::YMap(node)) = nodes.get(txn, id) else {
        bail!("unknown semantic node: {id}")
    };
    // Walk only ancestry; resolving a caret must not decode every run/mark.
    let mut current = node.clone();
    let mut visited = HashSet::new();
    visited.insert(id.to_owned());
    loop {
        let position = string(&current, txn, "position")?;
        let Some((parent, _)) = position.rsplit_once('|') else {
            break;
        };
        ensure!(
            visited.insert(parent.to_owned()),
            "hidden semantic parent cycle"
        );
        let Some(Out::YMap(owner)) = nodes.get(txn, parent) else {
            bail!("hidden semantic orphan")
        };
        current = owner;
    }
    match node.get(txn, "text") {
        Some(Out::YText(text)) => Ok(text),
        _ => bail!("node has no text leaf"),
    }
}

fn string<T: ReadTxn>(map: &MapRef, txn: &T, key: &str) -> Result<String> {
    match map.get(txn, key) {
        Some(Out::Any(Any::String(value))) => Ok(value.to_string()),
        _ => bail!("missing semantic field {key}"),
    }
}

fn json_any(value: &Value) -> Any {
    match value {
        Value::Bool(value) => Any::Bool(*value),
        _ => Any::String(serde_json::to_string(value).expect("JSON value").into()),
    }
}

fn any_json(value: &Any) -> Result<Value> {
    match value {
        Any::Bool(value) => Ok(Value::Bool(*value)),
        Any::String(value) => Ok(serde_json::from_str(value)?),
        _ => bail!("invalid semantic attribute"),
    }
}

fn run_attrs(run: &RichRun) -> Attrs {
    let Value::Object(marks) = serde_json::to_value(&run.marks).expect("serializable marks") else {
        unreachable!()
    };
    let mut attrs: Attrs = marks
        .into_iter()
        .filter(|(_, value)| !value.is_null() && *value != Value::Bool(false))
        .map(|(key, value)| (key.into(), json_any(&value)))
        .collect();
    if let Some(atom) = &run.atom {
        attrs.insert(
            "atom".into(),
            json_any(&serde_json::to_value(atom).expect("serializable atom")),
        );
    }
    attrs
}

fn read_runs<T: ReadTxn>(text: &TextRef, txn: &T) -> Result<Vec<RichRun>> {
    let mut runs = Vec::new();
    for diff in text.diff(txn, |_| ()) {
        let Out::Any(Any::String(value)) = diff.insert else {
            bail!("semantic text contains a non-text embed")
        };
        let mut marks = serde_json::to_value(RichMarks::default())?;
        let mut atom: Option<markdown_editor::rich::RichAtom> = None;
        if let Some(attrs) = diff.attributes {
            for (key, value) in *attrs {
                if value == Any::Null {
                    continue;
                }
                if key.as_ref() == "atom" {
                    atom = Some(serde_json::from_value(any_json(&value)?)?);
                } else {
                    ensure!(
                        marks.get(key.as_ref()).is_some(),
                        "unknown text attribute {key}"
                    );
                    marks[key.as_ref()] = any_json(&value)?;
                }
            }
        }
        let marks: RichMarks = serde_json::from_value(marks)?;
        if let Some(atom) = atom {
            ensure!(
                !value.is_empty() && value.chars().all(|ch| ch == '\u{FFFC}'),
                "atom metadata requires object replacement characters"
            );
            // Y.Text coalesces equal attributes, not semantic atom boundaries.
            // Clone only the additional atoms; move the decoded payload last.
            for _ in 1..value.len() / '\u{FFFC}'.len_utf8() {
                runs.push(RichRun {
                    text: "\u{FFFC}".into(),
                    marks: marks.clone(),
                    atom: Some(atom.clone()),
                });
            }
            runs.push(RichRun {
                text: "\u{FFFC}".into(),
                marks,
                atom: Some(atom),
            });
        } else {
            runs.push(RichRun {
                text: value.to_string(),
                marks,
                atom: None,
            });
        }
    }
    Ok(runs)
}

pub(super) fn read_content<T: ReadTxn>(body: &MapRef, txn: &T) -> Result<RichDocument> {
    let nodes = nodes(body, txn)?;
    let mut all = BTreeMap::new();
    let mut positions: BTreeMap<Option<String>, Vec<(String, String)>> = BTreeMap::new();
    for (id, value) in nodes.iter(txn) {
        let Out::YMap(map) = value else {
            bail!("node is not a Y.Map")
        };
        let position = string(&map, txn, "position")?;
        let (parent, rank) = position
            .rsplit_once('|')
            .map_or((None, position.as_str()), |(parent, rank)| {
                (Some(parent.to_owned()), rank)
            });
        validate_rank(rank)?;
        positions
            .entry(parent)
            .or_default()
            .push((rank.to_owned(), id.to_owned()));
        let kind = serde_json::from_str(&string(&map, txn, "kind")?)?;
        let Some(Out::YMap(attrs)) = map.get(txn, "attrs") else {
            bail!("node has no attribute map")
        };
        let mut values = BTreeMap::new();
        for (key, value) in attrs.iter(txn) {
            let Out::Any(value) = value else {
                bail!("invalid node attribute")
            };
            values.insert(key.to_owned(), any_json(&value)?);
        }
        let Some(Out::YText(text)) = map.get(txn, "text") else {
            bail!("node has no Y.Text")
        };
        let mut runs = read_runs(&text, txn)?;
        if kind == RichKind::CodeBlock && !runs.is_empty() {
            // Kind and inline marks are independent CRDT registers. Code
            // displays plain text; keep concurrent marks/atoms in shared
            // storage so undoing the conversion can reveal them again.
            let mut bytes = 0;
            for run in &runs {
                run.validate().map_err(|error| anyhow!("{error}"))?;
                bytes += run.text.len();
            }
            let mut plain = std::mem::take(&mut runs[0].text);
            plain.reserve(bytes - plain.len());
            for run in &runs[1..] {
                plain.push_str(&run.text);
            }
            runs.clear();
            if !plain.is_empty() {
                runs.push(RichRun {
                    text: plain,
                    ..RichRun::default()
                });
            }
        }
        all.insert(
            id.to_owned(),
            RichNode {
                id: id.to_owned(),
                kind,
                attrs: values,
                runs,
                children: vec![],
            },
        );
    }
    for siblings in positions.values_mut() {
        siblings.sort();
    }
    fn children(
        parent: Option<&str>,
        positions: &BTreeMap<Option<String>, Vec<(String, String)>>,
        all: &mut BTreeMap<String, RichNode>,
    ) -> Vec<RichNode> {
        positions
            .get(&parent.map(str::to_owned))
            .into_iter()
            .flatten()
            .filter_map(|(_, id)| {
                let mut node = all.remove(id)?;
                node.children = children(Some(id), positions, all);
                Some(node)
            })
            .collect()
    }
    // Orphans and parent cycles are hidden, not rewritten. Concurrent moves
    // can temporarily detach a subtree without replacing its CRDT identities.
    Ok(RichDocument {
        version: RICH_DOCUMENT_VERSION,
        blocks: children(None, &positions, &mut all),
    })
}

fn set_string(map: &MapRef, txn: &mut yrs::TransactionMut<'_>, key: &str, value: &str) {
    if !matches!(map.get(txn, key), Some(Out::Any(Any::String(old))) if old.as_ref() == value) {
        map.insert(txn, key, value);
    }
}

fn write_text(text: &TextRef, txn: &mut yrs::TransactionMut<'_>, runs: &[RichRun]) {
    let old = text.get_string(txn);
    let desired: String = runs.iter().map(|run| run.text.as_str()).collect();
    if old != desired {
        super::splice_text(text, txn, &old, &desired).expect("validated text fits Yjs offsets");
    }
    // Compare the current attributed intervals after splicing. Only changed
    // keys are formatted, so concurrent independent bold/italic edits merge.
    let current = text
        .diff(txn, |_| ())
        .into_iter()
        .map(|part| {
            let Out::Any(Any::String(value)) = part.insert else {
                unreachable!("validated text")
            };
            (
                value.encode_utf16().count() as u32,
                part.attributes.map(|attrs| *attrs).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    let (mut index, mut remaining, mut offset) = (0, current.first().map_or(0, |part| part.0), 0);
    for run in runs {
        let attrs = run_attrs(run);
        let mut length = run.text.encode_utf16().count() as u32;
        while length != 0 {
            let count = length.min(remaining);
            let old = &current[index].1;
            let mut delta = Attrs::new();
            for (key, value) in &attrs {
                if old.get(key) != Some(value) {
                    delta.insert(key.clone(), value.clone());
                }
            }
            for key in old.keys() {
                if !attrs.contains_key(key) {
                    delta.insert(key.clone(), Any::Null);
                }
            }
            if !delta.is_empty() {
                text.format(txn, offset, count, delta);
            }
            offset += count;
            length -= count;
            remaining -= count;
            if remaining == 0 {
                index += 1;
                remaining = current.get(index).map_or(0, |part| part.0);
            }
        }
    }
}

pub(super) fn write_content(
    body: &MapRef,
    txn: &mut yrs::TransactionMut<'_>,
    content: &RichDocument,
) {
    let nodes = match body.get(txn, "nodes") {
        Some(Out::YMap(nodes)) => nodes,
        _ => body.insert(txn, "nodes", MapPrelim::default()),
    };
    let mut retained = HashSet::new();
    fn write(
        nodes: &MapRef,
        txn: &mut yrs::TransactionMut<'_>,
        list: &[RichNode],
        parent: Option<&str>,
        retained: &mut HashSet<String>,
    ) {
        // Retain existing positions when sibling order is unchanged. New/moved
        // nodes allocate between surviving neighbours, without global rebalance.
        let mut lower: Option<String> = None;
        for (index, node) in list.iter().enumerate() {
            retained.insert(node.id.clone());
            let map = match nodes.get(txn, &node.id) {
                Some(Out::YMap(map)) => map,
                _ => nodes.insert(txn, node.id.as_str(), MapPrelim::default()),
            };
            let old = string(&map, txn, "position").ok();
            let old_rank = old
                .as_deref()
                .and_then(|value| match value.rsplit_once('|') {
                    Some((owner, rank)) if Some(owner) == parent => Some(rank),
                    None if parent.is_none() => Some(value),
                    _ => None,
                });
            let upper = list[index + 1..].iter().find_map(|next| {
                let Some(Out::YMap(map)) = nodes.get(txn, &next.id) else {
                    return None;
                };
                let value = string(&map, txn, "position").ok()?;
                let rank = match value.rsplit_once('|') {
                    Some((owner, rank)) if Some(owner) == parent => rank,
                    None if parent.is_none() => value.as_str(),
                    _ => return None,
                };
                lower
                    .as_deref()
                    .is_none_or(|lower| lower < rank)
                    .then(|| rank.to_owned())
            });
            let rank = old_rank
                .filter(|rank| {
                    lower.as_deref().is_none_or(|lower| lower < *rank)
                        && upper.as_deref().is_none_or(|upper| *rank < upper)
                })
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    super::fractional_between(lower.as_deref(), upper.as_deref())
                        .expect("validated semantic ranks")
                });
            set_string(&map, txn, "position", &super::placement(parent, &rank));
            lower = Some(rank);
            set_string(
                &map,
                txn,
                "kind",
                &serde_json::to_string(&node.kind).expect("node kind"),
            );
            let attrs = match map.get(txn, "attrs") {
                Some(Out::YMap(attrs)) => attrs,
                _ => map.insert(txn, "attrs", MapPrelim::default()),
            };
            let removed: Vec<_> = attrs
                .keys(txn)
                .filter(|key| !node.attrs.contains_key(*key))
                .map(str::to_owned)
                .collect();
            for key in removed {
                attrs.remove(txn, key.as_str());
            }
            for (key, value) in &node.attrs {
                let value = json_any(value);
                if attrs.get(txn, key).as_ref() != Some(&Out::Any(value.clone())) {
                    attrs.insert(txn, key.as_str(), value);
                }
            }
            let text = match map.get(txn, "text") {
                Some(Out::YText(text)) => text,
                _ => map.insert(txn, "text", TextPrelim::new("")),
            };
            write_text(&text, txn, &node.runs);
            write(nodes, txn, &node.children, Some(&node.id), retained);
        }
    }
    write(&nodes, txn, &content.blocks, None, &mut retained);
    let removed: Vec<_> = nodes
        .keys(txn)
        .filter(|id| !retained.contains(*id))
        .map(str::to_owned)
        .collect();
    for id in removed {
        nodes.remove(txn, id.as_str());
    }
}

pub(super) fn retain_boundary_ids(old: &RichDocument, new: &mut RichDocument) {
    fn retain(old: &[RichNode], new: &mut [RichNode]) {
        for (old, new) in old.iter().zip(new.iter_mut()) {
            new.id = old.id.clone();
            retain(&old.children, &mut new.children);
        }
    }
    retain(&old.blocks, &mut new.blocks);
}

pub(super) fn validate_schema(doc: &Doc, require_version: bool) -> Result<()> {
    let txn = doc.transact();
    if let Some(meta) = txn.get_map("meta") {
        let version = meta.get(&txn, "version");
        ensure!(
            matches!(version, Some(Out::Any(Any::Number(value))) if value.as_i64() == Some(i64::from(RICH_DOCUMENT_VERSION))),
            "unsupported document schema version"
        );
        ensure!(
            matches!(meta.get(&txn, "schema"), Some(Out::Any(Any::String(value))) if value.as_ref() == SCHEMA),
            "unknown document schema"
        );
    } else {
        ensure!(!require_version, "document schema version is missing");
    }
    if let Some(sections) = txn.get_map("sections") {
        for (_, value) in sections.iter(&txn) {
            let Out::YMap(section) = value else {
                bail!("section is not a Y.Map")
            };
            let body = body(&section, &txn)?;
            let content = read_content(&body, &txn)?;
            content.validate().map_err(|error| anyhow!("{error}"))?;
            let encoded = string(&section, &txn, "kind")?;
            ensure!(
                super::encode_kind(&super::read_kind(&section, &txn)) == encoded,
                "unsupported section kind"
            );
            let position = string(&section, &txn, "position")?;
            validate_rank(
                position
                    .rsplit_once('|')
                    .map_or(position.as_str(), |(_, rank)| rank),
            )?;
        }
    }
    Ok(())
}

pub(super) fn validate_rank(rank: &str) -> Result<()> {
    ensure!(
        !rank.is_empty()
            && rank
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "invalid structural rank"
    );
    Ok(())
}
