//! Collaborative section storage backed by a Yjs-compatible Yrs document.
//!
//! Sections and semantic nodes are Y.Maps, node leaves are
//! attributed Y.Texts, and each parent/rank placement is one atomic register.
//! Markdown and JSON are explicit boundaries, never collaborative storage.

use crate::block::{BlockSpec, SectionKind};
use crate::native_editor::rich::{RICH_DOCUMENT_VERSION, RichDocument, RichKind};
use serde::{Deserialize, Serialize};
#[path = "rich_storage.rs"]
mod rich_storage;
use anyhow::{Result, anyhow, bail, ensure};
use similar::{ChangeTag, TextDiff};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;
use yrs::encoding::read::Read;
use yrs::sync::time::{Clock, SystemClock};
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::{Decode, Decoder, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Assoc, Doc, GetString, IndexedSequence, Map, MapPrelim, MapRef, OffsetKind, Options, Out,
    ReadTxn, StateVector, StickyIndex, Text, TextPrelim, TextRef, Transact, UndoManager, Update,
};

pub type SectionId = String;

const LOCAL_ORIGIN: &str = "section-editor/local";
const REMOTE_ORIGIN: &str = "section-editor/remote";

/// An anchor into a stable semantic node. Resolution never resurrects nodes.
#[derive(Clone, Debug)]
pub struct RelativePosition {
    section: SectionId,
    node: String,
    index: StickyIndex,
}

impl RelativePosition {
    pub fn section(&self) -> &str {
        &self.section
    }
    pub fn node(&self) -> &str {
        &self.node
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: SectionId,
    pub content: RichDocument,
    pub parent: Option<SectionId>,
    pub kind: SectionKind,
    pub depth: usize,
}

#[derive(Serialize, Deserialize)]
struct JsonDocument {
    version: u32,
    sections: Vec<Section>,
}

/// A replica of a structured rich-text document. New replicas start empty.
///
/// IDs are permanent. Concurrent moves resolve as a Y.Map register write, while
/// edits merge inside the existing nested Y.Text. Deleted IDs are never reused.
pub struct Document {
    doc: Doc,
    sections: MapRef,
    history: UndoManager,
    history_time: Arc<AtomicU64>,
    history_group_depth: usize,
    typing_section: Option<SectionId>,
}

struct Entry {
    id: SectionId,
    position: String,
    parent: Option<SectionId>,
    kind: SectionKind,
    depth: usize,
    body: MapRef,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        let doc = Doc::with_options(Options {
            // Yjs text positions count UTF-16 code units, not UTF-8 bytes.
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let sections = doc.get_or_insert_map("sections");
        // A common, immutable seed has the same Yjs identity on every replica.
        // It is not a random per-peer epoch or an undoable local operation.
        rich_storage::initialize_schema(&doc);
        // Freeze the clock inside an explicit command, so a command can span
        // queued UI events without the ordinary 500ms typing timeout splitting it.
        let history_time = Arc::new(AtomicU64::new(0));
        let clock_time = history_time.clone();
        let mut history = UndoManager::with_options(yrs::undo::Options {
            timestamp: Arc::new(move || match clock_time.load(Ordering::Relaxed) {
                0 => SystemClock.now(),
                frozen => frozen,
            }),
            ..yrs::undo::Options::default()
        });
        history.include_origin(LOCAL_ORIGIN);
        history.expand_scope(&doc, &sections);
        Self {
            doc,
            sections,
            history,
            history_time,
            history_group_depth: 0,
            typing_section: None,
        }
    }

    pub fn relative_position(
        &self,
        id: &str,
        node_id: &str,
        source_byte_offset: usize,
        assoc: Assoc,
    ) -> Result<RelativePosition> {
        let txn = self.doc.transact();
        let section = self.section_map(&txn, id)?;
        let source = rich_storage::node_text(&section, &txn, node_id)?;
        let text = source.get_string(&txn);
        ensure!(
            text.is_char_boundary(source_byte_offset),
            "source offset is not a UTF-8 character boundary"
        );
        let offset = u32::try_from(text[..source_byte_offset].encode_utf16().count())?;
        let index = if offset == source.len(&txn) && assoc == Assoc::After {
            // Yrs' item-relative constructor excludes the right edge. A
            // type-relative anchor follows the end, including an empty Y.Text.
            StickyIndex::from_type(&txn, &source, assoc)
        } else {
            source
                .sticky_index(&txn, offset, assoc)
                .ok_or_else(|| anyhow!("source offset is out of range"))?
        };
        Ok(RelativePosition {
            section: id.to_owned(),
            node: node_id.to_owned(),
            index,
        })
    }

    pub fn resolve_position(
        &self,
        position: &RelativePosition,
    ) -> Option<(SectionId, String, usize)> {
        let txn = self.doc.transact();
        if !self
            .entries(&txn)
            .iter()
            .any(|entry| entry.id == position.section)
        {
            return None;
        }
        let section = self.section_map(&txn, position.section()).ok()?;
        let source = rich_storage::node_text(&section, &txn, position.node()).ok()?;
        let offset = position.index.get_offset(&txn)?;
        // A foreign replacement of a section/source must not resolve an anchor
        // against a different Y.Text merely because its section ID matches.
        if offset.branch.as_ref() != source.as_ref() {
            return None;
        }
        let text = source.get_string(&txn);
        let mut utf16 = 0;
        for (byte, ch) in text.char_indices() {
            if utf16 == offset.index {
                return Some((position.section.clone(), position.node.clone(), byte));
            }
            utf16 += ch.len_utf16() as u32;
        }
        (utf16 == offset.index)
            .then(|| (position.section.clone(), position.node.clone(), text.len()))
    }

    pub fn undo(&mut self) -> bool {
        self.stop_capturing();
        self.history.undo_blocking()
    }

    pub fn redo(&mut self) -> bool {
        self.stop_capturing();
        self.history.redo_blocking()
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn stop_capturing(&mut self) {
        // Native command handlers may request their usual boundaries while a
        // larger slash operation owns the capture group.
        if self.history_group_depth != 0 {
            return;
        }
        self.history.reset();
        self.typing_section = None;
    }

    pub fn clear_history(&mut self) {
        self.history.clear_all();
        self.stop_capturing();
    }

    /// Isolate one command, including mutations delivered in later UI events.
    /// Calls may nest; the outermost end closes the command's undo item.
    pub fn begin_history_group(&mut self) {
        if self.history_group_depth == 0 {
            self.stop_capturing();
            self.history_time
                .store(SystemClock.now().max(1), Ordering::Relaxed);
        }
        self.history_group_depth += 1;
    }

    pub fn end_history_group(&mut self) {
        if self.history_group_depth == 0 {
            return;
        }
        self.history_group_depth -= 1;
        if self.history_group_depth == 0 {
            self.history_time.store(0, Ordering::Relaxed);
            self.stop_capturing();
        }
    }

    /// Visible tree in preorder, including noneditable list/quote containers.
    pub fn sections(&self) -> Vec<Section> {
        let txn = self.doc.transact();
        self.entries(&txn)
            .into_iter()
            .map(|entry| Section {
                id: entry.id,
                content: rich_storage::read_content(&entry.body, &txn)
                    .expect("validated semantic storage"),
                parent: entry.parent,
                kind: entry.kind,
                depth: entry.depth,
            })
            .collect()
    }

    /// Direct children in sibling order. `None` selects document roots.
    pub fn children(&self, parent: Option<&str>) -> Vec<Section> {
        self.sections()
            .into_iter()
            .filter(|section| section.parent.as_deref() == parent)
            .collect()
    }

    /// Parse and insert all Markdown roots; return the first editable identity.
    pub fn insert_section(&mut self, before: Option<&str>, markdown: &str) -> Result<SectionId> {
        let parsed = crate::block_markdown::parse_blocks(markdown)?;
        let blocks = if parsed.blocks.is_empty() {
            vec![BlockSpec {
                kind: SectionKind::Text,
                content: RichDocument::paragraph(""),
                children: vec![],
            }]
        } else {
            parsed.blocks
        };
        for spec in &blocks {
            validate_spec(spec)?;
            ensure!(!spec.kind.is_list_item(), "list items require a list group");
        }
        ensure!(
            blocks.iter().any(has_editable),
            "Markdown has no editable block"
        );
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            validate_placement(&entries, None, before, None)?;
            let mut first = None;
            for spec in &blocks {
                let entries = document.entries(&txn);
                let position = sibling_position(&entries, None, before, None)?;
                let mut paths = Vec::new();
                document.insert_spec(
                    &mut txn,
                    None,
                    position,
                    spec,
                    &mut vec![],
                    &mut paths,
                    &mut None,
                )?;
                if first.is_none() {
                    first = paths.into_iter().find_map(|(_, id)| {
                        let map = document.section_map(&txn, &id).ok()?;
                        read_kind(&map, &txn).is_editable().then_some(id)
                    });
                }
            }
            first.ok_or_else(|| anyhow!("Markdown has no editable block"))
        })
    }

    pub fn insert_block(
        &mut self,
        parent: Option<&str>,
        before: Option<&str>,
        spec: &BlockSpec,
    ) -> Result<SectionId> {
        validate_spec(spec)?;
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            validate_placement(&entries, parent, before, None)?;
            validate_child(&entries, parent, &spec.kind)?;
            let position = sibling_position(&entries, parent, before, None)?;
            document.insert_spec(
                &mut txn,
                parent,
                position,
                spec,
                &mut vec![],
                &mut vec![],
                &mut None,
            )
        })
    }

    /// Structural commands form one undo item, also inside an existing group.
    fn command<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.begin_history_group();
        let result = operation(self);
        self.end_history_group();
        result
    }

    fn insert_spec(
        &self,
        txn: &mut yrs::TransactionMut<'_>,
        parent: Option<&str>,
        position: String,
        spec: &BlockSpec,
        path: &mut Vec<usize>,
        paths: &mut Vec<(Vec<usize>, SectionId)>,
        reuse: &mut Option<(SectionId, Vec<usize>)>,
    ) -> Result<SectionId> {
        let retained = reuse.as_ref().is_some_and(|(_, target)| target == path);
        let id = if retained {
            reuse.take().unwrap().0
        } else {
            Uuid::new_v4().to_string()
        };
        let section = if retained {
            self.section_map(txn, &id)?
        } else {
            self.sections.insert(txn, id.as_str(), MapPrelim::default())
        };
        let encoded_position = placement(parent, &position);
        if !matches!(section.get(txn, "position"), Some(Out::Any(Any::String(current))) if current.as_ref() == encoded_position.as_str())
        {
            section.insert(txn, "position", encoded_position);
        }
        if !retained || read_kind(&section, txn) != spec.kind {
            section.insert(txn, "kind", encode_kind(&spec.kind));
        }
        let body = if retained {
            rich_storage::body(&section, txn)?
        } else {
            section.insert(txn, "body", MapPrelim::default())
        };
        rich_storage::write_content(&body, txn, &spec.content);
        paths.push((path.clone(), id.clone()));
        let mut previous = if retained {
            self.entries(txn)
                .into_iter()
                .filter(|entry| entry.parent.as_deref() == Some(&id))
                .last()
                .map(|entry| entry.position)
        } else {
            None
        };
        for (index, child) in spec.children.iter().enumerate() {
            let rank = fractional_between(previous.as_deref(), None)?;
            previous = Some(rank.clone());
            path.push(index);
            self.insert_spec(txn, Some(&id), rank, child, path, paths, reuse)?;
            path.pop();
        }
        Ok(id)
    }

    /// Markdown is an explicit import boundary. Retain matching node identities
    /// where possible so boundary callers do not replace an unchanged text leaf.
    pub fn set_markdown(&mut self, id: &str, markdown: &str) -> Result<()> {
        let mut content =
            RichDocument::from_markdown(markdown).map_err(|error| anyhow!("{error}"))?;
        let old = self
            .sections()
            .into_iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?
            .content;
        rich_storage::retain_boundary_ids(&old, &mut content);
        self.set_content(id, &content)
    }

    pub fn set_content(&mut self, id: &str, content: &RichDocument) -> Result<()> {
        content.validate().map_err(|error| anyhow!("{error}"))?;
        crate::media::validate_media_content(content)?;
        let body = {
            let txn = self.doc.transact();
            let section = self.section_map(&txn, id)?;
            ensure!(
                read_kind(&section, &txn).is_editable(),
                "cannot edit a structural container"
            );
            let body = rich_storage::body(&section, &txn)?;
            if rich_storage::read_content(&body, &txn)? == *content {
                return Ok(());
            }
            body
        };
        if self.history_group_depth == 0 && self.typing_section.as_deref() != Some(id) {
            self.stop_capturing();
            self.typing_section = Some(id.to_owned());
        }
        rich_storage::write_content(
            &body,
            &mut self.doc.transact_mut_with(LOCAL_ORIGIN),
            content,
        );
        Ok(())
    }

    /// Reorder within the current parent; cross-group targets are rejected.
    pub fn move_section(&mut self, id: &str, before: Option<&str>) -> Result<()> {
        let sections = self.sections();
        let section = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        self.reparent_section(id, section.parent.as_deref(), before)
    }

    /// Move the existing node and its full subtree, retaining every Y.Text.
    pub fn reparent_section(
        &mut self,
        id: &str,
        parent: Option<&str>,
        before: Option<&str>,
    ) -> Result<()> {
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            let entry = find_entry(&entries, id)?;
            validate_placement(&entries, parent, before, Some(id))?;
            let into_list = parent.is_some_and(|id| {
                entries
                    .iter()
                    .any(|entry| entry.id == id && matches!(entry.kind, SectionKind::List { .. }))
            });
            let kind = if entry.kind.default_collapsed().is_some() {
                list_role(&entry.kind, into_list, false)
            } else {
                entry.kind.clone()
            };
            validate_child(&entries, parent, &kind)?;
            if before == Some(id) {
                ensure!(
                    entry.parent.as_deref() == parent,
                    "target has another parent"
                );
                return Ok(());
            }
            let position = sibling_position(&entries, parent, before, Some(id))?;
            if kind != entry.kind {
                document
                    .section_map(&txn, id)?
                    .insert(&mut txn, "kind", encode_kind(&kind));
            }
            document.section_map(&txn, id)?.insert(
                &mut txn,
                "position",
                placement(parent, &position),
            );
            let old_parent = entry
                .parent
                .clone()
                .filter(|old| Some(old.as_str()) != parent);
            drop(txn);
            document.prune_empty_ancestors(old_parent.as_deref())?;
            Ok(())
        })
    }

    /// Move selected subtree roots in document order, retaining their IDs and
    /// Y.Texts. Editable roots adopt the destination's list role; containers
    /// cannot enter lists. Duplicates and selected descendants move only once.
    /// Validate the complete batch before writing, and undo it as one command.
    pub fn move_sections(
        &mut self,
        ids: &[SectionId],
        parent: Option<&str>,
        before: Option<&str>,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            let selected: HashSet<_> = ids.iter().map(String::as_str).collect();
            for id in &selected {
                find_entry(&entries, id)?;
            }
            let mut roots = Vec::with_capacity(selected.len());
            let mut covered_depth = None;
            for entry in &entries {
                if covered_depth.is_some_and(|depth| entry.depth > depth) {
                    continue;
                }
                covered_depth = None;
                if selected.contains(entry.id.as_str()) {
                    roots.push(entry);
                    covered_depth = Some(entry.depth);
                }
            }
            let target_is_list = match parent {
                Some(id) => matches!(find_entry(&entries, id)?.kind, SectionKind::List { .. }),
                None => false,
            };
            let target_is_task = target_is_list
                && entries.iter().any(|entry| {
                    entry.parent.as_deref() == parent
                        && matches!(entry.kind, SectionKind::ListItem { checked: Some(_) })
                });
            for entry in &roots {
                validate_placement(&entries, parent, before, Some(&entry.id))?;
                ensure!(
                    before != Some(entry.id.as_str()),
                    "cannot drop before a selected section"
                );
            }
            // Allocate every rank and resolve every map before the first write:
            // even a malformed foreign rank cannot partially apply a batch.
            let mut lower = None;
            let mut upper = None;
            for sibling in entries.iter().filter(|entry| {
                entry.parent.as_deref() == parent && !selected.contains(entry.id.as_str())
            }) {
                if before == Some(sibling.id.as_str()) {
                    upper = Some(sibling.position.as_str());
                    break;
                }
                lower = Some(sibling.position.as_str());
            }
            let mut moves: Vec<(&Entry, MapRef, SectionKind, String)> =
                Vec::with_capacity(roots.len());
            for entry in roots {
                let kind = list_role(&entry.kind, target_is_list, target_is_task);
                validate_child(&entries, parent, &kind)?;
                let position = fractional_between(
                    moves.last().map(|moving| moving.3.as_str()).or(lower),
                    upper,
                )?;
                moves.push((
                    entry,
                    document.section_map(&txn, &entry.id)?,
                    kind,
                    position,
                ));
            }
            for (entry, map, kind, position) in &moves {
                if kind != &entry.kind {
                    map.insert(&mut txn, "kind", encode_kind(kind));
                }
                map.insert(&mut txn, "position", placement(parent, position));
            }
            drop(txn);
            let mut pruned = HashSet::new();
            for (entry, _, _, _) in &moves {
                if let Some(old_parent) = entry.parent.as_deref().filter(|old| Some(*old) != parent)
                {
                    if pruned.insert(old_parent) {
                        document.prune_empty_ancestors(Some(old_parent))?;
                    }
                }
            }
            Ok(())
        })
    }

    pub fn indent_section(&mut self, id: &str) -> Result<bool> {
        let sections = self.sections();
        let section = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        let siblings: Vec<_> = sections
            .iter()
            .filter(|other| other.parent == section.parent)
            .collect();
        let index = siblings.iter().position(|other| other.id == id).unwrap();
        let Some(previous) = index.checked_sub(1).map(|index| siblings[index]) else {
            return Ok(false);
        };
        self.command(|document| {
            if section.kind.is_list_item() {
                let group = sections
                    .iter()
                    .find(|other| Some(&other.id) == section.parent.as_ref())
                    .ok_or_else(|| anyhow!("list item has no list"))?;
                let nested = sections
                    .iter()
                    .find(|other| {
                        other.parent.as_deref() == Some(&previous.id) && other.kind == group.kind
                    })
                    .map(|other| other.id.clone());
                let nested = match nested {
                    Some(id) => id,
                    None => document.insert_block(
                        Some(&previous.id),
                        None,
                        &BlockSpec {
                            kind: group.kind.clone(),
                            content: RichDocument::default(),
                            children: vec![],
                        },
                    )?,
                };
                document.reparent_section(id, Some(&nested), None)?;
            } else {
                if matches!(previous.kind, SectionKind::List { .. }) {
                    return Ok(false);
                }
                document.reparent_section(id, Some(&previous.id), None)?;
            }
            Ok(true)
        })
    }

    pub fn outdent_section(&mut self, id: &str) -> Result<bool> {
        let sections = self.sections();
        let section = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        let Some(parent_id) = &section.parent else {
            return Ok(false);
        };
        let parent = sections
            .iter()
            .find(|other| &other.id == parent_id)
            .unwrap();
        if section.kind.is_list_item() {
            let owner = sections
                .iter()
                .find(|other| Some(&other.id) == parent.parent.as_ref());
            let Some(owner) = owner.filter(|owner| owner.kind.is_list_item()) else {
                return self.lift_list_item(id, false);
            };
            let before = next_sibling(&sections, &owner.id);
            return self.command(|document| {
                document.reparent_section(id, owner.parent.as_deref(), before.as_deref())?;
                document.remove_empty_list(parent_id)?;
                Ok(true)
            });
        }
        if parent.parent.as_ref().is_some_and(|id| {
            sections
                .iter()
                .any(|other| &other.id == id && matches!(other.kind, SectionKind::List { .. }))
        }) {
            return Ok(false);
        }
        let before = next_sibling(&sections, parent_id);
        self.reparent_section(id, parent.parent.as_deref(), before.as_deref())?;
        Ok(true)
    }

    /// An empty item becomes a normal block after its list. Trailing items
    /// become a following list, so exiting in the middle preserves source order.
    pub fn exit_list_item(&mut self, id: &str) -> Result<bool> {
        self.lift_list_item(id, true)
    }

    fn lift_list_item(&mut self, id: &str, empty_only: bool) -> Result<bool> {
        let sections = self.sections();
        let item = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        if !item.kind.is_list_item() || (empty_only && !item.content.plain_text().trim().is_empty())
        {
            return Ok(false);
        }
        let group = sections
            .iter()
            .find(|section| Some(&section.id) == item.parent.as_ref())
            .ok_or_else(|| anyhow!("list item has no list"))?;
        let siblings: Vec<_> = sections
            .iter()
            .filter(|section| section.parent == item.parent)
            .collect();
        let index = siblings
            .iter()
            .position(|section| section.id == id)
            .unwrap();
        let before = next_sibling(&sections, &group.id);
        self.command(|document| {
            // Kind and placement change in one transaction: never publish a
            // paragraph directly inside a list, even to local observers.
            {
                let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
                let entries = document.entries(&txn);
                let position =
                    sibling_position(&entries, group.parent.as_deref(), before.as_deref(), None)?;
                let map = document.section_map(&txn, id)?;
                let kind = if empty_only {
                    SectionKind::Text
                } else {
                    list_role(&item.kind, false, false)
                };
                map.insert(&mut txn, "kind", encode_kind(&kind));
                map.insert(
                    &mut txn,
                    "position",
                    placement(group.parent.as_deref(), &position),
                );
            }
            if index + 1 < siblings.len() {
                let kind = match group.kind {
                    SectionKind::List { ordered, start } => SectionKind::List {
                        ordered,
                        start: start.saturating_add(index as u64 + 1),
                    },
                    _ => bail!("list item has no list"),
                };
                let following = document.insert_block(
                    group.parent.as_deref(),
                    before.as_deref(),
                    &BlockSpec {
                        kind,
                        content: RichDocument::default(),
                        children: vec![],
                    },
                )?;
                for sibling in &siblings[index + 1..] {
                    document.reparent_section(&sibling.id, Some(&following), None)?;
                }
            }
            document.remove_empty_list(&group.id)?;
            Ok(true)
        })
    }

    fn remove_empty_list(&mut self, id: &str) -> Result<()> {
        self.prune_empty_ancestors(Some(id))
    }

    fn prune_empty_ancestors(&mut self, parent: Option<&str>) -> Result<()> {
        let mut parent = parent.map(str::to_owned);
        while let Some(id) = parent {
            let sections = self.sections();
            let Some(section) = sections.iter().find(|section| section.id == id) else {
                break;
            };
            if !matches!(section.kind, SectionKind::List { .. } | SectionKind::Quote)
                || sections
                    .iter()
                    .any(|child| child.parent.as_deref() == Some(&id))
            {
                break;
            }
            parent = section.parent.clone();
            self.sections
                .remove(&mut self.doc.transact_mut_with(LOCAL_ORIGIN), id.as_str());
        }
        Ok(())
    }

    pub fn set_kind(&mut self, id: &str, kind: SectionKind) -> Result<()> {
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            let entry = find_entry(&entries, id)?;
            validate_child(&entries, entry.parent.as_deref(), &kind)?;
            for child in entries
                .iter()
                .filter(|child| child.parent.as_deref() == Some(id))
            {
                validate_kind_child(&kind, &child.kind)?;
            }
            ensure!(
                kind.is_editable()
                    || rich_storage::read_content(&entry.body, &txn)?
                        .plain_text()
                        .is_empty(),
                "container text must be empty"
            );
            document
                .section_map(&txn, id)?
                .insert(&mut txn, "kind", encode_kind(&kind));
            Ok(())
        })
    }

    /// Toggle a single editable block's list role without changing its ID.
    pub fn wrap_list(&mut self, id: &str, ordered: bool, task: bool) -> Result<()> {
        let sections = self.sections();
        let item = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        if item.kind.is_list_item() {
            let group = sections
                .iter()
                .find(|section| Some(&section.id) == item.parent.as_ref())
                .unwrap();
            let checked = match item.kind {
                SectionKind::ListItem { checked } => checked,
                _ => None,
            };
            if matches!(item.kind, SectionKind::ListItem { .. })
                && matches!(group.kind, SectionKind::List { ordered: current, .. } if current == ordered)
                && checked.is_some() == task
            {
                self.lift_list_item(id, false)?;
                return Ok(());
            }
            return self.command(|document| {
                let start = match group.kind {
                    SectionKind::List { start, .. } => start,
                    _ => 1,
                };
                document.set_kind(&group.id, SectionKind::List { ordered, start })?;
                document.set_kind(
                    id,
                    SectionKind::ListItem {
                        checked: if task { Some(false) } else { None },
                    },
                )
            });
        }
        ensure!(
            matches!(
                item.kind,
                SectionKind::Text | SectionKind::Collapsible { .. }
            ),
            "only editable text can become a list"
        );
        self.command(|document| {
            let group = document.insert_block(
                item.parent.as_deref(),
                Some(id),
                &BlockSpec {
                    kind: SectionKind::List { ordered, start: 1 },
                    content: RichDocument::default(),
                    children: vec![],
                },
            )?;
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let map = document.section_map(&txn, id)?;
            map.insert(
                &mut txn,
                "kind",
                encode_kind(&SectionKind::ListItem {
                    checked: if task { Some(false) } else { None },
                }),
            );
            map.insert(
                &mut txn,
                "position",
                placement(Some(&group), &fractional_between(None, None)?),
            );
            Ok(())
        })
    }

    /// Toggle the current node in/out of an outer quote, retaining its subtree.
    pub fn wrap_quote(&mut self, id: &str) -> Result<()> {
        let sections = self.sections();
        let mut section = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        if section.kind.is_list_item() {
            section = sections
                .iter()
                .find(|other| Some(&other.id) == section.parent.as_ref())
                .ok_or_else(|| anyhow!("list item has no list"))?;
        }
        let id = section.id.as_str();
        let parent = sections
            .iter()
            .find(|other| Some(&other.id) == section.parent.as_ref());
        self.command(|document| {
            if let Some(quote) = parent.filter(|parent| parent.kind == SectionKind::Quote) {
                let before = next_sibling(&sections, &quote.id);
                let siblings = document.children(Some(&quote.id));
                let index = siblings.iter().position(|other| other.id == id).unwrap();
                document.reparent_section(id, quote.parent.as_deref(), before.as_deref())?;
                if index + 1 < siblings.len() {
                    let following = document.insert_block(
                        quote.parent.as_deref(),
                        before.as_deref(),
                        &BlockSpec {
                            kind: SectionKind::Quote,
                            content: RichDocument::default(),
                            children: vec![],
                        },
                    )?;
                    for sibling in &siblings[index + 1..] {
                        document.reparent_section(&sibling.id, Some(&following), None)?;
                    }
                }
            } else {
                let quote = document.insert_block(
                    section.parent.as_deref(),
                    Some(id),
                    &BlockSpec {
                        kind: SectionKind::Quote,
                        content: RichDocument::default(),
                        children: vec![],
                    },
                )?;
                document.reparent_section(id, Some(&quote), None)?;
            }
            Ok(())
        })
    }

    /// Toggle an editable title into a disclosure item, retaining its identity.
    /// Reapplying lifts it out as a standalone disclosure with the same body.
    pub fn wrap_collapsible_list(&mut self, id: &str) -> Result<()> {
        let sections = self.sections();
        let item = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        ensure!(
            item.kind.is_editable(),
            "only editable text can become a collapsible list"
        );
        if matches!(item.kind, SectionKind::CollapsibleListItem { .. }) {
            self.lift_list_item(id, false)?;
            return Ok(());
        }
        let kind = SectionKind::CollapsibleListItem {
            default_collapsed: item.kind.default_collapsed().unwrap_or(true),
        };
        self.command(|document| {
            if item.kind.is_list_item() {
                document.set_kind(
                    item.parent
                        .as_deref()
                        .ok_or_else(|| anyhow!("list item has no list"))?,
                    SectionKind::List {
                        ordered: false,
                        start: 1,
                    },
                )?;
                document.set_kind(id, kind)?;
            } else {
                let group = document.insert_block(
                    item.parent.as_deref(),
                    Some(id),
                    &BlockSpec {
                        kind: SectionKind::List {
                            ordered: false,
                            start: 1,
                        },
                        content: RichDocument::default(),
                        children: vec![],
                    },
                )?;
                let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
                let map = document.section_map(&txn, id)?;
                map.insert(&mut txn, "kind", encode_kind(&kind));
                map.insert(
                    &mut txn,
                    "position",
                    placement(Some(&group), &fractional_between(None, None)?),
                );
            }
            if !sections
                .iter()
                .any(|section| section.parent.as_deref() == Some(id))
            {
                document.insert_block(
                    Some(id),
                    None,
                    &BlockSpec {
                        kind: SectionKind::Text,
                        content: RichDocument::paragraph(""),
                        children: vec![],
                    },
                )?;
            }
            Ok(())
        })
    }

    /// Retain the title and its subtree, and add an initial typing child.
    pub fn wrap_collapsible(&mut self, id: &str) -> Result<SectionId> {
        let section = self
            .sections()
            .into_iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        ensure!(
            section.kind == SectionKind::Text,
            "only editable text can become a collapsible"
        );
        self.command(|document| {
            if section.content.plain_text().trim().is_empty() {
                let mut content = RichDocument::paragraph("Details");
                rich_storage::retain_boundary_ids(&section.content, &mut content);
                document.set_content(id, &content)?;
            }
            document.set_kind(
                id,
                SectionKind::Collapsible {
                    default_collapsed: true,
                },
            )?;
            let before = document
                .children(Some(id))
                .first()
                .map(|child| child.id.clone());
            document.insert_block(
                Some(id),
                before.as_deref(),
                &BlockSpec {
                    kind: SectionKind::Text,
                    content: RichDocument::paragraph(""),
                    children: vec![],
                },
            )
        })
    }

    pub fn export_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&JsonDocument {
            version: RICH_DOCUMENT_VERSION,
            sections: self.sections(),
        })?)
    }

    pub fn import_json(json: &str) -> Result<Self> {
        let input: JsonDocument = serde_json::from_str(json)?;
        ensure!(
            input.version == RICH_DOCUMENT_VERSION,
            "unsupported JSON document version"
        );
        Self::from_sections(input.sections)
    }

    fn from_sections(sections: Vec<Section>) -> Result<Self> {
        let mut known = HashMap::new();
        for section in &sections {
            ensure!(!section.id.is_empty(), "empty section ID");
            ensure!(
                known.insert(section.id.as_str(), section).is_none(),
                "duplicate section ID"
            );
            section
                .content
                .validate()
                .map_err(|error| anyhow!("{error}"))?;
            crate::media::validate_media_content(&section.content)?;
            ensure!(
                section.kind.is_editable() || section.content.plain_text().is_empty(),
                "container text must be empty"
            );
        }
        for section in &sections {
            if let Some(parent) = section.parent.as_deref() {
                let owner = known
                    .get(parent)
                    .ok_or_else(|| anyhow!("missing section parent"))?;
                validate_kind_child(&owner.kind, &section.kind)?;
            } else {
                ensure!(!section.kind.is_list_item(), "root list item");
            }
            let mut visited = HashSet::new();
            let mut ancestor = Some(section.id.as_str());
            while let Some(id) = ancestor {
                ensure!(visited.insert(id), "section parent cycle");
                ancestor = known
                    .get(id)
                    .ok_or_else(|| anyhow!("missing section parent"))?
                    .parent
                    .as_deref();
            }
        }
        let mut document = Self::new();
        {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let mut ranks: HashMap<Option<&str>, String> = HashMap::new();
            for section in &sections {
                let parent = section.parent.as_deref();
                let rank = fractional_between(ranks.get(&parent).map(String::as_str), None)?;
                let map =
                    document
                        .sections
                        .insert(&mut txn, section.id.as_str(), MapPrelim::default());
                map.insert(&mut txn, "kind", encode_kind(&section.kind));
                map.insert(&mut txn, "position", placement(parent, &rank));
                let body = map.insert(&mut txn, "body", MapPrelim::default());
                rich_storage::write_content(&body, &mut txn, &section.content);
                ranks.insert(parent, rank);
            }
        }
        document.clear_history();
        Ok(document)
    }

    /// Replace one editable body with parsed siblings/subtrees. Paths refer to
    /// the caller's specs (including containers). The first editable descendant
    /// reuses `id` and its Y.Text; existing children remain attached to that ID.
    pub fn replace_section(
        &mut self,
        id: &str,
        specs: &[BlockSpec],
    ) -> Result<Vec<(Vec<usize>, SectionId)>> {
        let sections = self.sections();
        let old = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        ensure!(old.kind.is_editable(), "cannot edit a structural container");
        let input = if specs.is_empty() {
            vec![BlockSpec {
                kind: SectionKind::Text,
                content: RichDocument::paragraph(""),
                children: vec![],
            }]
        } else {
            specs.to_vec()
        };
        for spec in &input {
            validate_spec(spec)?;
        }
        ensure!(
            input.iter().any(has_editable),
            "replacement has no editable block"
        );
        let checked = match old.kind {
            SectionKind::ListItem { checked } => checked,
            _ => None,
        };
        let in_list = old.kind.is_list_item();
        let inherited_kind = |index| match old.kind {
            SectionKind::CollapsibleListItem { default_collapsed } => {
                SectionKind::CollapsibleListItem { default_collapsed }
            }
            _ => SectionKind::ListItem {
                checked: if index == 0 {
                    checked
                } else {
                    checked.map(|_| false)
                },
            },
        };
        let flatten_list = in_list
            && input.len() == 1
            && matches!(input[0].kind, SectionKind::List { .. })
            && !input[0].children.is_empty();
        let group_kind = flatten_list.then(|| input[0].kind.clone());
        let mut specs = Vec::new();
        let mut translations = HashMap::new();
        let mut reuse_path = None;
        if flatten_list {
            for (index, spec) in input[0].children.iter().enumerate() {
                map_spec_paths(
                    spec,
                    &mut vec![0, index],
                    &mut vec![index],
                    &mut translations,
                    &mut reuse_path,
                );
                specs.push(spec.clone());
            }
        } else {
            for (index, input) in input.into_iter().enumerate() {
                let mut spec = input;
                let mut normalized = vec![index];
                if in_list && spec.kind == SectionKind::Text {
                    spec.kind = inherited_kind(index);
                } else if in_list && matches!(spec.kind, SectionKind::Collapsible { .. }) {
                    spec.kind = list_role(&spec.kind, true, false);
                }
                let wrap = in_list && !spec.kind.is_list_item();
                if wrap {
                    normalized.push(0);
                }
                map_spec_paths(
                    &spec,
                    &mut vec![index],
                    &mut normalized,
                    &mut translations,
                    &mut reuse_path,
                );
                if matches!(spec.kind, SectionKind::CollapsibleListItem { .. })
                    && spec.children.is_empty()
                    && (index != 0
                        || !sections
                            .iter()
                            .any(|section| section.parent.as_deref() == Some(id)))
                {
                    spec.children.push(BlockSpec {
                        kind: SectionKind::Text,
                        content: RichDocument::paragraph(""),
                        children: vec![],
                    });
                }
                specs.push(if wrap {
                    BlockSpec {
                        kind: inherited_kind(index),
                        content: RichDocument::paragraph(""),
                        children: vec![spec],
                    }
                } else {
                    spec
                });
            }
        }
        let reuse_path = reuse_path.unwrap();
        fn collect_ids<'a>(
            nodes: &'a [crate::native_editor::rich::RichNode],
            ids: &mut HashSet<&'a str>,
        ) {
            for node in nodes {
                ids.insert(node.id.as_str());
                collect_ids(&node.children, ids);
            }
        }
        fn has_existing_nodes(
            nodes: &[crate::native_editor::rich::RichNode],
            ids: &HashSet<&str>,
        ) -> bool {
            nodes.iter().any(|node| {
                ids.contains(node.id.as_str()) || has_existing_nodes(&node.children, ids)
            })
        }
        fn has_existing_ids(spec: &BlockSpec, ids: &HashSet<&str>) -> bool {
            has_existing_nodes(&spec.content.blocks, ids)
                || spec
                    .children
                    .iter()
                    .any(|child| has_existing_ids(child, ids))
        }
        let mut old_ids = HashSet::new();
        collect_ids(&old.content.blocks, &mut old_ids);
        // Fresh boundary DTOs have no shared IDs. Native normalization already
        // carries identities, including nodes moved to another replacement body.
        if !specs.iter().any(|spec| has_existing_ids(spec, &old_ids)) {
            let mut retained = &mut specs[reuse_path[0]];
            for &index in &reuse_path[1..] {
                retained = &mut retained.children[index];
            }
            rich_storage::retain_boundary_ids(&old.content, &mut retained.content);
        }
        let before = next_sibling(&sections, id);
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            for spec in &specs {
                validate_child(&entries, old.parent.as_deref(), &spec.kind)?;
            }
            let mut paths = Vec::new();
            let mut reuse = Some((id.to_owned(), reuse_path));
            let original_position = find_entry(&entries, id)?.position.as_str();
            let upper = before
                .as_ref()
                .map(|id| find_entry(&entries, id).map(|entry| entry.position.as_str()))
                .transpose()?;
            // Allocate every sibling interval before making any writes.
            let mut positions = Vec::with_capacity(specs.len());
            positions.push(original_position.to_owned());
            for _ in 1..specs.len() {
                positions.push(fractional_between(
                    positions.last().map(String::as_str),
                    upper,
                )?);
            }
            if let Some(kind) = &group_kind {
                document
                    .section_map(&txn, old.parent.as_deref().unwrap())?
                    .insert(&mut txn, "kind", encode_kind(kind));
            }
            for (index, (position, spec)) in positions.into_iter().zip(&specs).enumerate() {
                document.insert_spec(
                    &mut txn,
                    old.parent.as_deref(),
                    position,
                    spec,
                    &mut vec![index],
                    &mut paths,
                    &mut reuse,
                )?;
            }
            let mut result: Vec<_> = paths
                .into_iter()
                .filter_map(|(path, id)| translations.remove(&path).map(|original| (original, id)))
                .collect();
            if flatten_list {
                result.push((vec![0], old.parent.clone().unwrap()));
            }
            result.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Ok(result)
        })
    }

    /// Remove an empty text block without deleting its descendants.
    /// A final list item becomes the document's remaining typing paragraph.
    pub(crate) fn remove_empty_block(&mut self, id: &str) -> Result<bool> {
        let sections = self.sections();
        let section = sections
            .iter()
            .find(|section| section.id == id)
            .ok_or_else(|| anyhow!("unknown section: {id}"))?;
        if !matches!(
            section.kind,
            SectionKind::Text
                | SectionKind::ListItem { .. }
                | SectionKind::CollapsibleListItem { .. }
        ) || !section.content.blocks.iter().all(|node| {
            matches!(node.kind, RichKind::Paragraph | RichKind::Heading)
                && node.children.is_empty()
                && node
                    .runs
                    .iter()
                    .all(|run| run.atom.is_none() && run.text.is_empty())
        }) {
            return Ok(false);
        }
        let in_list = section.kind.is_list_item();
        if sections
            .iter()
            .filter(|section| section.kind.is_editable())
            .take(2)
            .count()
            == 1
        {
            return if in_list {
                self.exit_list_item(id)
            } else {
                Ok(false)
            };
        }
        let has_children = sections
            .iter()
            .any(|child| child.parent.as_deref() == Some(id));
        let parent = if in_list && has_children {
            sections
                .iter()
                .find(|group| Some(&group.id) == section.parent.as_ref())
                .ok_or_else(|| anyhow!("list item has no list"))?
                .parent
                .as_deref()
        } else {
            section.parent.as_deref()
        };
        self.command(|document| {
            if in_list && has_children {
                document.exit_list_item(id)?;
            }
            for child in sections
                .iter()
                .filter(|child| child.parent.as_deref() == Some(id))
            {
                document.reparent_section(&child.id, parent, Some(id))?;
            }
            document.remove_section(id)?;
            Ok(true)
        })
    }

    pub fn remove_section(&mut self, id: &str) -> Result<()> {
        self.command(|document| {
            let mut txn = document.doc.transact_mut_with(LOCAL_ORIGIN);
            let entries = document.entries(&txn);
            let index = entries
                .iter()
                .position(|entry| entry.id == id)
                .ok_or_else(|| anyhow!("unknown section: {id}"))?;
            let depth = entries[index].depth;
            let count = 1 + entries[index + 1..]
                .iter()
                .take_while(|entry| entry.depth > depth)
                .count();
            for entry in &entries[index..index + count] {
                document.sections.remove(&mut txn, entry.id.as_str());
            }
            let parent = entries[index].parent.clone();
            drop(txn);
            document.prune_empty_ancestors(parent.as_deref())?;
            Ok(())
        })
    }

    /// Serialize structural markers from the actual tree, never from bodies.
    pub fn markdown(&self) -> String {
        let sections = self.sections();
        serialize_children(&sections, None)
    }

    /// An unframed Yjs V1 state vector, suitable for Y.encodeStateVector.
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// An unframed Yjs V1 update relative to an encoded remote state vector.
    /// This is NOT a sync protocol message; use apply_update at the receiver.
    pub fn encode_update(&self, remote_state_vector: &[u8]) -> Result<Vec<u8>> {
        let vector = decode_exact::<StateVector>(remote_state_vector)?;
        Ok(self.doc.transact().encode_state_as_update_v1(&vector))
    }

    /// Apply an unframed Yjs V1 update. Repeated/out-of-order updates are safe;
    /// Yrs retains unresolved dependencies until their preceding updates arrive.
    pub fn apply_update(&mut self, bytes: &[u8]) -> Result<()> {
        self.preflight_updates(&[bytes])?;
        let update = decode_exact::<Update>(bytes)?;
        self.doc
            .transact_mut_with(REMOTE_ORIGIN)
            .apply_update(update)
            .map_err(|error| anyhow!("cannot apply Yjs update: {error}"))
    }

    /// An unframed complete Yjs V1 update, including deletion information.
    pub fn encode_state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    /// Start standard bidirectional sync: BOTH peers send this on connection.
    ///
    /// Framing is yrs/y-websocket Message::Sync: varuint(0), varuint(subtype),
    /// varuint(payload length), payload. Subtypes are y-protocols sync-step-1
    /// (0), sync-step-2 (1), and update (2). Raw updates above lack this frame.
    pub fn start_sync(&self) -> Vec<u8> {
        Message::Sync(SyncMessage::SyncStep1(self.doc.transact().state_vector())).encode_v1()
    }

    /// Consume one or more concatenated sync frames and return reply frames.
    /// Only document sync is supported, not awareness/authentication messages.
    /// Truncated frames and invalid payloads return errors rather than being
    /// silently treated as end-of-stream. Decode everything before mutating.
    pub fn receive_sync(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        let incoming = decode_sync_frames(message)?;
        let updates: Vec<_> = incoming
            .iter()
            .filter_map(|message| match message {
                Incoming::Update(update) => Some(update.encode_v1()),
                _ => None,
            })
            .collect();
        self.preflight_updates(&updates.iter().map(Vec::as_slice).collect::<Vec<_>>())?;
        let mut replies = Vec::new();
        for message in incoming {
            match message {
                Incoming::Step1(vector) => {
                    let update = self.doc.transact().encode_state_as_update_v1(&vector);
                    replies.push(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1());
                }
                Incoming::Update(update) => {
                    self.doc
                        .transact_mut_with(REMOTE_ORIGIN)
                        .apply_update(update)
                        .map_err(|error| anyhow!("cannot apply sync update: {error}"))?;
                }
            }
        }
        Ok(replies)
    }

    /// Reply to sync handshakes while a caller temporarily queues remote writes
    /// (for example during native IME composition). Updates are validated but
    /// not applied; pass the original frames to `receive_sync` when ready.
    pub fn reply_sync_without_applying(&self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        let incoming = decode_sync_frames(message)?;
        let updates: Vec<_> = incoming
            .iter()
            .filter_map(|message| match message {
                Incoming::Update(update) => Some(update.encode_v1()),
                _ => None,
            })
            .collect();
        self.preflight_updates(&updates.iter().map(Vec::as_slice).collect::<Vec<_>>())?;
        let txn = self.doc.transact();
        Ok(incoming
            .into_iter()
            .filter_map(|message| match message {
                Incoming::Step1(vector) => Some(
                    Message::Sync(SyncMessage::SyncStep2(
                        txn.encode_state_as_update_v1(&vector),
                    ))
                    .encode_v1(),
                ),
                Incoming::Update(_) => None,
            })
            .collect())
    }

    fn preflight_updates(&self, updates: &[&[u8]]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let shadow = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        shadow
            .transact_mut()
            .apply_update(decode_exact::<Update>(&self.encode_state())?)
            .map_err(|error| anyhow!("cannot copy Yjs state: {error}"))?;
        for bytes in updates {
            let isolated = Doc::with_options(Options {
                offset_kind: OffsetKind::Utf16,
                ..Options::default()
            });
            isolated
                .transact_mut()
                .apply_update(decode_exact::<Update>(bytes)?)
                .map_err(|error| anyhow!("cannot preflight Yjs update: {error}"))?;
            rich_storage::validate_schema(&isolated, false)?;
            shadow
                .transact_mut()
                .apply_update(decode_exact::<Update>(bytes)?)
                .map_err(|error| anyhow!("cannot preflight Yjs update: {error}"))?;
            rich_storage::validate_schema(&shadow, true)?;
        }
        Ok(())
    }

    fn section_map<T: ReadTxn>(&self, txn: &T, id: &str) -> Result<MapRef> {
        match self.sections.get(txn, id) {
            Some(Out::YMap(section)) => Ok(section),
            _ => bail!("unknown section: {id}"),
        }
    }

    fn entries<T: ReadTxn>(&self, txn: &T) -> Vec<Entry> {
        let mut entries: Vec<_> = self
            .sections
            .iter(txn)
            .filter_map(|(id, value)| {
                let Out::YMap(section) = value else {
                    return None;
                };
                let Some(Out::Any(Any::String(position))) = section.get(txn, "position") else {
                    return None;
                };
                let Some(Out::YMap(body)) = section.get(txn, "body") else {
                    return None;
                };
                let (parent, rank) = position
                    .rsplit_once('|')
                    .map_or((None, position.as_ref()), |(parent, rank)| {
                        (Some(parent.to_owned()), rank)
                    });
                Some(Entry {
                    id: id.to_owned(),
                    position: rank.to_owned(),
                    parent,
                    kind: read_kind(&section, txn),
                    depth: 0,
                    body,
                })
            })
            .collect();
        // The ID provides a total-order tie-break even for foreign documents
        // containing equal keys. Locally generated full keys are unique.
        entries.sort_unstable_by(|left, right| {
            left.position
                .cmp(&right.position)
                .then_with(|| left.id.cmp(&right.id))
        });
        project_tree(entries)
    }
}

enum Incoming {
    Step1(StateVector),
    Update(Update),
}

fn decode_sync_frames(message: &[u8]) -> Result<Vec<Incoming>> {
    ensure!(!message.is_empty(), "empty sync message");
    let mut decoder = DecoderV1::from(message);
    let mut incoming = Vec::new();
    while !decoder.read_to_end()?.is_empty() {
        let tag: u8 = decoder.read_var()?;
        ensure!(tag == 0, "unsupported sync envelope type {tag}");
        let subtype: u8 = decoder.read_var()?;
        let payload = decoder.read_buf()?;
        incoming.push(match subtype {
            0 => Incoming::Step1(decode_exact::<StateVector>(payload)?),
            1 | 2 => Incoming::Update(decode_exact::<Update>(payload)?),
            _ => bail!("unsupported sync message subtype {subtype}"),
        });
    }
    Ok(incoming)
}

fn flush_splice(
    source: &TextRef,
    txn: &mut yrs::TransactionMut<'_>,
    offset: &mut u32,
    removed: &mut u32,
    inserted: &str,
) {
    if *removed != 0 {
        source.remove_range(txn, *offset, *removed);
        *removed = 0;
    }
    if !inserted.is_empty() {
        source.insert(txn, *offset, inserted);
        *offset += inserted.encode_utf16().count() as u32;
    }
}

fn decode_exact<T: Decode>(bytes: &[u8]) -> Result<T> {
    let mut decoder = DecoderV1::from(bytes);
    let value = T::decode(&mut decoder)?;
    ensure!(
        decoder.read_to_end()?.is_empty(),
        "trailing bytes in Yjs payload"
    );
    Ok(value)
}

fn placement(parent: Option<&str>, rank: &str) -> String {
    match parent {
        Some(parent) => format!("{parent}|{rank}"),
        None => rank.to_owned(),
    }
}

fn find_entry<'a>(entries: &'a [Entry], id: &str) -> Result<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| anyhow!("unknown section: {id}"))
}

fn validate_placement(
    entries: &[Entry],
    parent: Option<&str>,
    before: Option<&str>,
    moving: Option<&str>,
) -> Result<()> {
    if let Some(before) = before {
        ensure!(
            find_entry(entries, before)?.parent.as_deref() == parent,
            "target has another parent"
        );
    }
    let mut ancestor = parent;
    while let Some(id) = ancestor {
        ensure!(
            Some(id) != moving,
            "cannot move a section inside its own subtree"
        );
        ancestor = find_entry(entries, id)?.parent.as_deref();
    }
    Ok(())
}

fn validate_kind_child(parent: &SectionKind, child: &SectionKind) -> Result<()> {
    ensure!(
        matches!(parent, SectionKind::List { .. }) == child.is_list_item(),
        "list groups contain only list items, and list items require a list group"
    );
    Ok(())
}

fn validate_child(entries: &[Entry], parent: Option<&str>, kind: &SectionKind) -> Result<()> {
    match parent {
        Some(parent) => validate_kind_child(&find_entry(entries, parent)?.kind, kind),
        None => {
            ensure!(!kind.is_list_item(), "list items require a list group");
            Ok(())
        }
    }
}

fn validate_spec(spec: &BlockSpec) -> Result<()> {
    spec.content
        .validate()
        .map_err(|error| anyhow!("{error}"))?;
    crate::media::validate_media_content(&spec.content)?;
    ensure!(
        spec.kind.is_editable() || spec.content.plain_text().is_empty(),
        "container text must be empty"
    );
    for child in &spec.children {
        validate_kind_child(&spec.kind, &child.kind)?;
        validate_spec(child)?;
    }
    Ok(())
}

fn has_editable(spec: &BlockSpec) -> bool {
    spec.kind.is_editable() || spec.children.iter().any(has_editable)
}

fn map_spec_paths(
    spec: &BlockSpec,
    original: &mut Vec<usize>,
    normalized: &mut Vec<usize>,
    translations: &mut HashMap<Vec<usize>, Vec<usize>>,
    first_editable: &mut Option<Vec<usize>>,
) {
    translations.insert(normalized.clone(), original.clone());
    if first_editable.is_none() && spec.kind.is_editable() {
        *first_editable = Some(normalized.clone());
    }
    for (index, child) in spec.children.iter().enumerate() {
        original.push(index);
        normalized.push(index);
        map_spec_paths(child, original, normalized, translations, first_editable);
        original.pop();
        normalized.pop();
    }
}

fn sibling_position(
    entries: &[Entry],
    parent: Option<&str>,
    before: Option<&str>,
    excluding: Option<&str>,
) -> Result<String> {
    let siblings: Vec<_> = entries
        .iter()
        .filter(|entry| entry.parent.as_deref() == parent && Some(entry.id.as_str()) != excluding)
        .collect();
    let index = match before {
        Some(id) => siblings
            .iter()
            .position(|entry| entry.id == id)
            .ok_or_else(|| anyhow!("unknown sibling: {id}"))?,
        None => siblings.len(),
    };
    fractional_between(
        index
            .checked_sub(1)
            .map(|index| siblings[index].position.as_str()),
        siblings.get(index).map(|entry| entry.position.as_str()),
    )
}

fn next_sibling(sections: &[Section], id: &str) -> Option<SectionId> {
    let index = sections.iter().position(|section| section.id == id)?;
    let parent = &sections[index].parent;
    sections[index + 1..]
        .iter()
        .find(|section| &section.parent == parent)
        .map(|section| section.id.clone())
}

/// Placement determines list role; disclosure state survives either direction.
fn list_role(kind: &SectionKind, into_list: bool, task: bool) -> SectionKind {
    match (kind, into_list) {
        (SectionKind::Text, true) => SectionKind::ListItem {
            checked: task.then_some(false),
        },
        (SectionKind::ListItem { .. }, false) => SectionKind::Text,
        (SectionKind::Collapsible { default_collapsed }, true) => {
            SectionKind::CollapsibleListItem {
                default_collapsed: *default_collapsed,
            }
        }
        (SectionKind::CollapsibleListItem { default_collapsed }, false) => {
            SectionKind::Collapsible {
                default_collapsed: *default_collapsed,
            }
        }
        _ => kind.clone(),
    }
}

// Kind is one register too: an ordered list's flag and starting number cannot
// tear under concurrent list-type changes.
fn encode_kind(kind: &SectionKind) -> String {
    match kind {
        SectionKind::Text => "text".to_owned(),
        SectionKind::Quote => "quote".to_owned(),
        SectionKind::Collapsible { default_collapsed } => format!(
            "collapsible:{}",
            if *default_collapsed { "closed" } else { "open" }
        ),
        SectionKind::CollapsibleListItem { default_collapsed } => format!(
            "collapsible-item:{}",
            if *default_collapsed { "closed" } else { "open" }
        ),
        SectionKind::List { ordered, start } => format!(
            "list:{}:{start}",
            if *ordered { "ordered" } else { "bullet" }
        ),
        SectionKind::ListItem { checked } => match checked {
            None => "item".to_owned(),
            Some(false) => "task:unchecked".to_owned(),
            Some(true) => "task:checked".to_owned(),
        },
    }
}

fn read_kind<T: ReadTxn>(map: &MapRef, txn: &T) -> SectionKind {
    let Some(Out::Any(Any::String(kind))) = map.get(txn, "kind") else {
        return SectionKind::Text;
    };
    match kind.as_ref() {
        "quote" => SectionKind::Quote,
        "collapsible:open" => SectionKind::Collapsible {
            default_collapsed: false,
        },
        "collapsible:closed" => SectionKind::Collapsible {
            default_collapsed: true,
        },
        "collapsible-item:open" => SectionKind::CollapsibleListItem {
            default_collapsed: false,
        },
        "collapsible-item:closed" => SectionKind::CollapsibleListItem {
            default_collapsed: true,
        },
        "item" => SectionKind::ListItem { checked: None },
        "task:unchecked" => SectionKind::ListItem {
            checked: Some(false),
        },
        "task:checked" => SectionKind::ListItem {
            checked: Some(true),
        },
        value => {
            let mut parts = value.split(':');
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some("list"), Some(style @ ("ordered" | "bullet")), Some(start), None) => start
                    .parse()
                    .map_or(SectionKind::Text, |start| SectionKind::List {
                        ordered: style == "ordered",
                        start,
                    }),
                _ => SectionKind::Text,
            }
        }
    }
}

fn project_tree(mut entries: Vec<Entry>) -> Vec<Entry> {
    let indexes: HashMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect();
    // Missing parents are deliberately not roots. Their entire subtree stays
    // hidden, including children concurrently inserted after a deletion.
    let mut parents: Vec<_> = entries
        .iter()
        .map(|entry| {
            entry
                .parent
                .as_ref()
                .map(|parent| indexes.get(parent.as_str()).copied().unwrap_or(usize::MAX))
        })
        .collect();
    drop(indexes);
    let mut visited = vec![0u8; entries.len()];
    for start in 0..entries.len() {
        if visited[start] != 0 {
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut current = Some(start);
        while let Some(index) = current.filter(|index| *index != usize::MAX) {
            if visited[index] == 2 {
                break;
            }
            if visited[index] == 1 {
                let cycle_start = path.iter().position(|other| *other == index).unwrap();
                let root = *path[cycle_start..]
                    .iter()
                    .min_by_key(|index| &entries[**index].id)
                    .unwrap();
                parents[root] = None;
                entries[root].parent = None;
                break;
            }
            visited[index] = 1;
            path.push(index);
            current = parents[index];
        }
        for index in path {
            visited[index] = 2;
        }
    }
    // Kind and placement are separate CRDT registers. A concurrent checkbox
    // edit or cross-list move can win the kind register independently; derive
    // editable list roles from the surviving parent without rewriting state.
    for (index, parent) in parents.iter().enumerate() {
        let into_list = parent.is_some_and(|parent| {
            parent != usize::MAX && matches!(entries[parent].kind, SectionKind::List { .. })
        });
        entries[index].kind = list_role(&entries[index].kind, into_list, false);
    }
    let mut children = vec![Vec::new(); entries.len()];
    let mut roots = Vec::new();
    for (index, parent) in parents.into_iter().enumerate() {
        match parent {
            None => roots.push(index),
            Some(parent) if parent != usize::MAX => children[parent].push(index),
            _ => {}
        }
    }
    let mut stack: Vec<_> = roots.into_iter().rev().map(|index| (index, 0)).collect();
    let mut entries: Vec<_> = entries.into_iter().map(Some).collect();
    let mut visible = Vec::with_capacity(entries.len());
    while let Some((index, depth)) = stack.pop() {
        let mut entry = entries[index].take().unwrap();
        entry.depth = depth;
        visible.push(entry);
        stack.extend(
            children[index]
                .iter()
                .rev()
                .map(|child| (*child, depth + 1)),
        );
    }
    visible
}

fn splice_text(
    source: &TextRef,
    txn: &mut yrs::TransactionMut<'_>,
    old: &str,
    markdown: &str,
) -> Result<()> {
    let diff = TextDiff::from_chars(old, markdown);
    let (mut offset, mut removed, mut new_bytes, mut insert_start) = (0, 0, 0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                flush_splice(
                    source,
                    txn,
                    &mut offset,
                    &mut removed,
                    &markdown[insert_start..new_bytes],
                );
                offset += u32::try_from(change.value().encode_utf16().count())?;
                new_bytes += change.value().len();
                insert_start = new_bytes;
            }
            ChangeTag::Delete => removed += u32::try_from(change.value().encode_utf16().count())?,
            ChangeTag::Insert => new_bytes += change.value().len(),
        }
    }
    flush_splice(
        source,
        txn,
        &mut offset,
        &mut removed,
        &markdown[insert_start..new_bytes],
    );
    Ok(())
}

fn serialize_children(sections: &[Section], parent: Option<&str>) -> String {
    let mut children: HashMap<Option<&str>, Vec<&Section>> = HashMap::new();
    for section in sections {
        children
            .entry(section.parent.as_deref())
            .or_default()
            .push(section);
    }
    fn siblings(children: &HashMap<Option<&str>, Vec<&Section>>, parent: Option<&str>) -> String {
        children
            .get(&parent)
            .into_iter()
            .flatten()
            .map(|section| node(children, section))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    fn node(children: &HashMap<Option<&str>, Vec<&Section>>, section: &Section) -> String {
        match section.kind {
            SectionKind::List { ordered, start } => children
                .get(&Some(section.id.as_str()))
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, item)| {
                    let marker = if ordered {
                        format!("{}. ", start.saturating_add(index as u64))
                    } else {
                        "- ".to_owned()
                    };
                    let checkbox = match item.kind {
                        SectionKind::ListItem {
                            checked: Some(true),
                        } => "[x] ",
                        SectionKind::ListItem {
                            checked: Some(false),
                        } => "[ ] ",
                        _ => "",
                    };
                    let body = node(children, item);
                    let mut lines = body.split('\n');
                    let mut output = format!("{marker}{checkbox}{}", lines.next().unwrap_or(""));
                    let indent = " ".repeat(marker.len());
                    for line in lines {
                        output.push('\n');
                        if !line.is_empty() {
                            output.push_str(&indent);
                            output.push_str(line);
                        }
                    }
                    output
                })
                .collect::<Vec<_>>()
                .join("\n"),
            SectionKind::Quote => siblings(children, Some(&section.id))
                .split('\n')
                .map(|line| {
                    if line.is_empty() {
                        ">".to_owned()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            SectionKind::Collapsible { default_collapsed }
            | SectionKind::CollapsibleListItem { default_collapsed } => {
                crate::block_markdown::collapsible_markdown(
                    &section.content,
                    &siblings(children, Some(&section.id)),
                    default_collapsed,
                    children.get(&Some(section.id.as_str())).map_or(0, Vec::len),
                )
            }
            _ => {
                let nested = siblings(children, Some(&section.id));
                let body = crate::block::content_markdown(&section.content);
                if nested.is_empty() {
                    return body;
                }
                if body.is_empty() {
                    return nested;
                }
                let list_first = children
                    .get(&Some(section.id.as_str()))
                    .and_then(|items| items.first())
                    .is_some_and(|child| matches!(child.kind, SectionKind::List { .. }));
                format!(
                    "{}{}{}",
                    body,
                    if list_first { "\n" } else { "\n\n" },
                    nested
                )
            }
        }
    }
    siblings(&children, parent)
}

/// Dense lexicographic base-16 fractions. Each full key contains a fresh UUID,
/// rather than adding UUID only as a secondary sort key: concurrent inserts in
/// the same interval are therefore distinct fractional positions, and another
/// key can always be allocated between them without rebalancing other sections.
fn fractional_between(lower: Option<&str>, upper: Option<&str>) -> Result<String> {
    fn valid(key: &str) -> bool {
        !key.is_empty()
            && key
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && !key.ends_with('0')
    }
    ensure!(
        lower.is_none_or(valid) && upper.is_none_or(valid),
        "invalid fractional position"
    );
    ensure!(
        !matches!((lower, upper), (Some(left), Some(right)) if left >= right),
        "fractional position bounds must be distinct and ordered"
    );
    let lower = lower.unwrap_or("").as_bytes();
    let mut upper = upper.map(str::as_bytes);
    let mut key = String::with_capacity(lower.len().max(upper.map_or(0, |bound| bound.len())) + 34);
    let mut index = 0;
    loop {
        let left = lower.get(index).map_or(0, |byte| hex_value(*byte));
        let right = upper.map_or(16, |bound| {
            bound.get(index).map_or(0, |byte| hex_value(*byte))
        });
        if right - left > 1 {
            key.push(hex_digit(left + (right - left) / 2));
            // This digit differs from the upper bound, so every suffix stays
            // inside the interval. The final nonzero digit keeps keys dense.
            let mut discriminator = [0; 32];
            key.push_str(Uuid::new_v4().simple().encode_lower(&mut discriminator));
            key.push('8');
            return Ok(key);
        }
        key.push(hex_digit(left));
        if left != right {
            upper = None;
        }
        index += 1;
    }
}

fn hex_value(byte: u8) -> u8 {
    if byte <= b'9' {
        byte - b'0'
    } else {
        byte - b'a' + 10
    }
}

fn hex_digit(value: u8) -> char {
    b"0123456789abcdef"[usize::from(value)] as char
}

#[cfg(test)]
fn node(document: &Document, section: &str) -> String {
    document
        .sections()
        .into_iter()
        .find(|entry| entry.id == section)
        .and_then(|entry| entry.content.blocks.first().map(|node| node.id.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
fn edit_text(document: &mut Document, id: &str, text: &str) -> Result<()> {
    let mut content = document
        .sections()
        .into_iter()
        .find(|section| section.id == id)
        .ok_or_else(|| anyhow!("unknown section"))?
        .content;
    let block = content
        .blocks
        .first_mut()
        .ok_or_else(|| anyhow!("missing semantic leaf"))?;
    let marks = block
        .runs
        .first()
        .map(|run| run.marks.clone())
        .unwrap_or_default();
    block.runs = if text.is_empty() {
        vec![]
    } else {
        vec![crate::native_editor::rich::RichRun {
            text: text.to_owned(),
            marks,
            atom: None,
        }]
    };
    document.set_content(id, &content)
}

#[cfg(test)]
#[path = "atom_storage_tests.rs"]
mod atom_storage_tests;

#[cfg(test)]
#[path = "concurrent_code_tests.rs"]
mod concurrent_code_tests;

#[cfg(test)]
#[path = "concurrent_table_tests.rs"]
mod concurrent_table_tests;

#[cfg(test)]
#[path = "json_ancestor_tests.rs"]
mod json_ancestor_tests;

#[cfg(test)]
#[path = "nested_document_tests.rs"]
mod nested_tests;

#[cfg(test)]
#[path = "document_tests.rs"]
mod collaboration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn replica(original: &Document) -> Document {
        let mut replica = Document::new();
        replica.apply_update(&original.encode_state()).unwrap();
        replica
    }

    fn exchange(left: &mut Document, right: &mut Document) {
        let left_update = left.encode_update(&right.state_vector()).unwrap();
        let right_update = right.encode_update(&left.state_vector()).unwrap();
        left.apply_update(&right_update).unwrap();
        right.apply_update(&left_update).unwrap();
        assert_eq!(left.sections(), right.sections());
    }

    fn ids(document: &Document) -> Vec<SectionId> {
        document
            .sections()
            .into_iter()
            .map(|section| section.id)
            .collect()
    }

    #[test]
    fn concurrent_text_edits_preserve_unchanged_items() {
        let mut alice = Document::new();
        let id = alice.insert_section(None, "hello world").unwrap();
        let mut bob = replica(&alice);
        alice.set_markdown(&id, "hello brave world").unwrap();
        bob.set_markdown(&id, "hello world!").unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(alice.markdown(), "hello brave world!");
    }

    #[test]
    fn concurrent_move_and_edit_keep_one_stable_section() {
        let mut alice = Document::new();
        let first = alice.insert_section(None, "first").unwrap();
        let second = alice.insert_section(None, "second").unwrap();
        let mut bob = replica(&alice);
        alice.move_section(&second, Some(&first)).unwrap();
        bob.set_markdown(&second, "edited second").unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(ids(&alice), [second.clone(), first.clone()]);
        assert_eq!(alice.sections()[0].content.plain_text(), "edited second");
        alice.move_section(&first, Some(&second)).unwrap();
        bob.move_section(&first, None).unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(alice.sections().len(), 2);
        assert_eq!(ids(&alice).iter().filter(|id| **id == first).count(), 1);
    }

    #[test]
    fn concurrent_same_interval_inserts_allow_subsequent_between_insert() {
        let mut alice = Document::new();
        let start = alice.insert_section(None, "start").unwrap();
        let end = alice.insert_section(None, "end").unwrap();
        let mut bob = replica(&alice);
        alice.insert_section(Some(&end), "alice").unwrap();
        bob.insert_section(Some(&end), "bob").unwrap();
        exchange(&mut alice, &mut bob);
        let order = ids(&alice);
        assert_eq!(order[0], start);
        assert_eq!(order[3], end);
        let middle = alice.insert_section(Some(&order[2]), "between").unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(
            ids(&alice),
            [
                order[0].clone(),
                order[1].clone(),
                middle,
                order[2].clone(),
                order[3].clone()
            ]
        );
    }

    #[test]
    fn duplicate_and_out_of_order_updates_resolve_dependencies() {
        let mut alice = Document::new();
        let empty = alice.state_vector();
        let id = alice.insert_section(None, "one").unwrap();
        let creation = alice.encode_update(&empty).unwrap();
        let checkpoint = alice.state_vector();
        alice.set_markdown(&id, "one two").unwrap();
        let edit = alice.encode_update(&checkpoint).unwrap();
        let mut bob = Document::new();
        bob.apply_update(&edit).unwrap();
        bob.apply_update(&edit).unwrap();
        assert!(bob.sections().is_empty());
        bob.apply_update(&creation).unwrap();
        bob.apply_update(&creation).unwrap();
        bob.apply_update(&edit).unwrap();
        assert_eq!(alice.sections(), bob.sections());
    }

    #[test]
    fn deletion_wins_over_concurrent_edit_and_move() {
        let mut alice = Document::new();
        let id = alice.insert_section(None, "doomed").unwrap();
        let survivor = alice.insert_section(None, "survivor").unwrap();
        let mut bob = replica(&alice);
        alice.remove_section(&id).unwrap();
        bob.set_markdown(&id, "must not resurrect").unwrap();
        bob.move_section(&id, None).unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(ids(&alice), [survivor]);
        assert!(alice.set_markdown(&id, "no").is_err());
        assert!(alice.move_section(&id, None).is_err());
        assert!(alice.remove_section(&id).is_err());
        let mut restored = Document::new();
        restored.apply_update(&alice.encode_state()).unwrap();
        restored.apply_update(&bob.encode_state()).unwrap();
        assert_eq!(restored.sections(), alice.sections());
    }

    #[test]
    fn unicode_splices_use_utf16_and_preserve_concurrent_text() {
        let mut alice = Document::new();
        let id = alice.insert_section(None, "a😀e\u{301}終").unwrap();
        let mut bob = replica(&alice);
        alice.set_markdown(&id, "a😀É終").unwrap();
        bob.set_markdown(&id, "a😀e\u{301}終🚀").unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(alice.markdown(), "a😀É終🚀");
        alice.set_markdown(&id, "a😁É終🚀").unwrap();
        alice.set_markdown(&id, "😀").unwrap();
        alice.set_markdown(&id, "").unwrap();
        alice.set_markdown(&id, "🦀漢字").unwrap();
        exchange(&mut alice, &mut bob);
        assert_eq!(bob.markdown(), "🦀漢字");
        let vector = alice.state_vector();
        alice.set_markdown(&id, "🦀漢字").unwrap();
        assert_eq!(vector, alice.state_vector());
    }

    #[test]
    fn sync_handshake_and_live_updates_use_standard_framing() {
        let mut alice = Document::new();
        let id = alice.insert_section(None, "alice").unwrap();
        let mut bob = Document::new();
        bob.insert_section(None, "bob").unwrap();
        let alice_hello = alice.start_sync();
        let bob_hello = bob.start_sync();
        assert_eq!(&alice_hello[..2], &[0, 0]);
        let to_alice = bob.receive_sync(&alice_hello).unwrap();
        let to_bob = alice.receive_sync(&bob_hello).unwrap();
        for message in to_alice {
            assert!(alice.receive_sync(&message).unwrap().is_empty());
        }
        for message in to_bob {
            assert!(bob.receive_sync(&message).unwrap().is_empty());
        }
        assert_eq!(alice.sections(), bob.sections());
        let vector = bob.state_vector();
        alice.set_markdown(&id, "alice edited").unwrap();
        let raw_update = alice.encode_update(&vector).unwrap();
        let framed = Message::Sync(SyncMessage::Update(raw_update)).encode_v1();
        assert!(bob.receive_sync(&framed).unwrap().is_empty());
        assert_eq!(alice.sections(), bob.sections());
        let mut batched = alice.start_sync();
        batched.extend(alice.start_sync());
        assert_eq!(bob.receive_sync(&batched).unwrap().len(), 2);
    }

    #[test]
    fn malformed_sync_and_raw_payloads_are_rejected_without_partial_decode() {
        let mut document = Document::new();
        for malformed in [
            vec![],
            vec![0],
            vec![0, 0],
            vec![0, 0, 1],
            vec![0, 0, 2, 0],
            vec![0, 3, 0],
            vec![1, 0],
            vec![0, 2, 1, 255],
            vec![0, 0, 2, 0, 0],
        ] {
            assert!(
                document.receive_sync(&malformed).is_err(),
                "accepted {malformed:?}"
            );
        }
        assert!(document.apply_update(&[]).is_err());
        assert!(document.apply_update(&[0, 0, 0]).is_err());
        assert!(document.encode_update(&[]).is_err());
        assert!(document.encode_update(&[0, 0]).is_err());
        let mut other = Document::new();
        other.insert_section(None, "not partially applied").unwrap();
        let mut frame = Message::Sync(SyncMessage::Update(other.encode_state())).encode_v1();
        frame.push(0);
        assert!(document.receive_sync(&frame).is_err());
        assert!(document.sections().is_empty());
    }

    #[test]
    fn fractional_positions_are_dense_at_prefixes_and_extremes() {
        for (lower, upper) in [
            (None, None),
            (None, Some("0001")),
            (Some("f"), None),
            (Some("1"), Some("1001")),
            (Some("1fff"), Some("2")),
            (Some("abc1"), Some("abc2")),
        ] {
            let key = fractional_between(lower, upper).unwrap();
            assert!(lower.is_none_or(|lower| lower < key.as_str()));
            assert!(upper.is_none_or(|upper| key.as_str() < upper));
        }
        assert!(fractional_between(Some("8"), Some("8")).is_err());
        assert!(fractional_between(Some("f"), Some("1")).is_err());
    }
}
