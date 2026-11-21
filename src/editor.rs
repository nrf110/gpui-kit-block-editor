use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use markdown_editor::rich::{RichDocument, RichKind, RichNode};
use markdown_editor::{
    BlockEditorEvent, BlockEditorState, BlockKind as NativeBlockKind, BlockSelection,
    CapturedRange, EditOrigin, EditorCommand,
};
use yrs::Assoc;

use crate::{
    Document, SectionId,
    block::{BlockSpec, SectionKind, specs_from_rich},
    block_markdown::parse_blocks,
    diagram_viewer::DiagramViewer,
    document::RelativePosition,
    extension::{CommandAction, ExtensionRegistry, SlashCommand},
    rich_map::{RichMap, RichPoint, SelectionKind, SelectionOffsets},
};
use crate::{media::MediaAttachment, media_view::MediaView};

#[path = "selection_toolbar.rs"]
mod selection_toolbar;
use selection_toolbar::{SelectionDropdown, SelectionLink};

#[path = "media_editor.rs"]
mod media_editor;

#[path = "block_navigation.rs"]
mod block_navigation;

/// Raw Yjs v1 updates. Remote application never echoes a local event.
#[derive(Clone, Debug)]
pub enum DocumentEvent {
    Changed { update: Vec<u8> },
}

struct RelativeSelection {
    kind: SelectionKind,
    anchor: RelativePosition,
    head: RelativePosition,
    fallback: SelectionOffsets,
}

impl RelativeSelection {
    fn capture(
        document: &Document,
        id: &str,
        content: &RichDocument,
        map: &RichMap,
        selection: &BlockSelection,
    ) -> anyhow::Result<Self> {
        let offsets = map
            .selection_offsets(selection)
            .context("selection has no semantic mapping")?;
        fn node_order(nodes: &[RichNode], id: &str, index: &mut usize) -> Option<usize> {
            for node in nodes {
                if node.id == id {
                    return Some(*index);
                }
                *index += 1;
                if let Some(found) = node_order(&node.children, id, index) {
                    return Some(found);
                }
            }
            None
        }
        let forward = if offsets.anchor.node == offsets.head.node {
            (offsets.anchor.offset, offsets.anchor.after_atom)
                <= (offsets.head.offset, offsets.head.after_atom)
        } else {
            node_order(&content.blocks, &offsets.anchor.node, &mut 0)
                <= node_order(&content.blocks, &offsets.head.node, &mut 0)
        };
        let (anchor_assoc, head_assoc) = if offsets.anchor == offsets.head {
            (Assoc::After, Assoc::After)
        } else if forward {
            (Assoc::After, Assoc::Before)
        } else {
            (Assoc::Before, Assoc::After)
        };
        Ok(Self {
            kind: offsets.kind,
            anchor: document.relative_position(
                id,
                &offsets.anchor.node,
                offsets.anchor.offset,
                anchor_assoc,
            )?,
            head: document.relative_position(
                id,
                &offsets.head.node,
                offsets.head.offset,
                head_assoc,
            )?,
            fallback: offsets,
        })
    }

    fn resolve(&self, document: &Document, map: &RichMap) -> Option<BlockSelection> {
        let point = |relative: &RelativePosition, fallback: &RichPoint| {
            document
                .resolve_position(relative)
                .map(|(_, node, offset)| RichPoint {
                    node,
                    offset,
                    after_atom: fallback.after_atom,
                })
                .unwrap_or_else(|| fallback.clone())
        };
        map.restore_selection(SelectionOffsets {
            kind: self.kind,
            anchor: point(&self.anchor, &self.fallback.anchor),
            head: point(&self.head, &self.fallback.head),
        })
    }
}

enum PendingRemote {
    Update(Vec<u8>),
    Sync(Vec<u8>),
}

#[derive(Clone)]
struct ListMarker {
    label: SharedString,
    checked: Option<bool>,
}

struct ListEnd {
    group: SectionId,
    element: SharedString,
    indent: usize,
}

struct OpenList {
    depth: usize,
    group: SectionId,
    indent: usize,
    last: Option<SectionId>,
}

impl OpenList {
    fn finish(self, ends: &mut HashMap<SectionId, Vec<ListEnd>>) {
        if let Some(last) = self.last {
            ends.entry(last).or_default().push(ListEnd {
                element: format!("list-end-{}", self.group).into(),
                group: self.group,
                indent: self.indent,
            });
        }
    }
}

struct SectionView {
    editor: Entity<BlockEditorState>,
    content: RichDocument,
    preview: Option<Arc<Image>>,
    preview_error: Option<String>,
    previewable: bool,
    show_source: bool,
    revision: u64,
    rich_map: Option<RichMap>,
    selection: Option<RelativeSelection>,
    native_selection: BlockSelection,
    parent: Option<SectionId>,
    kind: SectionKind,
    indent: usize,
    marker: Option<ListMarker>,
    last_origin: Option<EditOrigin>,
    _subscription: Subscription,
    preview_task: Option<Task<()>>,
    media: Option<(MediaAttachment, Entity<MediaView>)>,
    navigation_anchor: ScrollAnchor,
}

struct Completion {
    section: SectionId,
    // Button-opened menus have no slash text to remove.
    range: Option<CapturedRange>,
    anchor_to_button: bool,
    query: String,
    commands: Vec<SlashCommand>,
    selected: usize,
    scroll: ScrollHandle,
}

impl Completion {
    fn scroll_to_selected(&self) {
        // Headings occupy scroll children but are not keyboard-selectable commands.
        let headings = self.commands[..=self.selected]
            .iter()
            .enumerate()
            .filter(|(index, command)| {
                *index == 0 || self.commands[index - 1].group != command.group
            })
            .count();
        self.scroll.scroll_to_item(self.selected + headings);
    }
}

#[derive(Clone)]
struct DraggedRoot {
    id: SectionId,
    parent: Option<SectionId>,
}

#[derive(Clone)]
struct DraggedSection {
    owner: EntityId,
    // GPUI retains the value supplied at render time. Fill this shared snapshot
    // only in its drag-start callback, after grip selection has been processed.
    roots: Rc<RefCell<Vec<DraggedRoot>>>,
}

#[derive(Clone)]
struct DropTarget {
    parent: Option<SectionId>,
    before: Option<SectionId>,
}

struct TreeNode {
    parent: Option<SectionId>,
    kind: SectionKind,
}
impl Render for DraggedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_4()
            .py_2()
            .rounded_md()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().foreground)
            .child("Move block")
    }
}

/// Retained, embeddable editor entity with independently editable nested blocks.
/// Construct inside `cx.new`, then mount the resulting entity in any GPUI layout.
pub struct BlockEditor {
    document: Document,
    extensions: Arc<ExtensionRegistry>,
    order: Vec<SectionId>,
    views: HashMap<SectionId, SectionView>,
    list_ends: HashMap<SectionId, Vec<ListEnd>>,
    list_starts: HashMap<SectionId, Vec<SectionId>>,
    tree: HashMap<SectionId, TreeNode>,
    tree_order: Vec<SectionId>,
    collapsed: HashMap<SectionId, bool>,
    block_selection: HashSet<SectionId>,
    block_anchor: Option<SectionId>,
    active_block_drag: Option<EntityId>,
    grip_pressed: bool,
    completion: Option<Completion>,
    expanded_diagram: Option<(SectionId, WeakEntity<DiagramViewer>)>,
    selection_dropdown: Option<SelectionDropdown>,
    selection_link: Option<SelectionLink>,
    selection_toolbar_target: Option<SectionId>,
    selection_toolbar_focus: FocusHandle,
    selection_link_icon: Icon,
    selection_list_icon: Icon,
    selection_collapsible_icon: Icon,
    selection_quote_icon: Icon,
    error: Option<String>,
    scroll: ScrollHandle,
    trash_icon: Icon,
    focus_scope: FocusHandle,
    pending_remote: Vec<PendingRemote>,
    pending_task: Option<Task<()>>,
    composition_group: Option<SectionId>,
    media_imports: usize,
    _keystrokes: Subscription,
}
impl EventEmitter<DocumentEvent> for BlockEditor {}

impl BlockEditor {
    pub fn new(
        document: Document,
        extensions: ExtensionRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = cx.weak_entity();
        let keystrokes = cx.intercept_keystrokes(move |event, window, cx| {
            if let Some(owner) = owner.upgrade() {
                let handled = owner.update(cx, |this, cx| {
                    if this.handle_selection_key(&event.keystroke, window, cx) {
                        return true;
                    }
                    if event.keystroke.key == "escape"
                        && this.focus_scope.contains_focused(window, cx)
                        && !this.block_selection.is_empty()
                    {
                        this.block_selection.clear();
                        this.block_anchor = None;
                        cx.stop_propagation();
                        cx.notify();
                        return true;
                    }
                    this.handle_history_key(&event.keystroke, window, cx)
                });
                if handled {
                    return;
                }
                let handled = owner.update(cx, |this, cx| {
                    this.handle_vertical_block_key(&event.keystroke, window, cx)
                        || this.handle_block_key(&event.keystroke, window, cx)
                });
                if handled {
                    return;
                }
            }
            if !matches!(
                event.keystroke.key.as_str(),
                "escape" | "down" | "up" | "enter" | "tab"
            ) || event.keystroke.modifiers.number_of_modifiers() != 0
            {
                return;
            }
            let _ = owner.update(cx, |this, cx| {
                this.handle_completion_key(&event.keystroke.key, window, cx)
            });
        });
        let mut this = Self {
            document,
            extensions: Arc::new(extensions),
            order: Vec::new(),
            views: HashMap::new(),
            list_ends: HashMap::new(),
            list_starts: HashMap::new(),
            tree: HashMap::new(),
            tree_order: Vec::new(),
            collapsed: HashMap::new(),
            block_selection: HashSet::new(),
            block_anchor: None,
            active_block_drag: None,
            grip_pressed: false,
            completion: None,
            expanded_diagram: None,
            selection_dropdown: None,
            selection_link: None,
            selection_toolbar_target: None,
            selection_toolbar_focus: cx.focus_handle().tab_stop(false),
            selection_link_icon: Icon::default()
                .data(include_bytes!("../assets/selection-link.svg")),
            selection_list_icon: Icon::default()
                .data(include_bytes!("../assets/selection-list.svg")),
            selection_collapsible_icon: Icon::default()
                .data(include_bytes!("../assets/selection-list-collapse.svg")),
            selection_quote_icon: Icon::default()
                .data(include_bytes!("../assets/selection-quote.svg")),
            error: None,
            scroll: ScrollHandle::new(),
            trash_icon: Icon::default().data(include_bytes!("../assets/trash.svg")),
            focus_scope: cx.focus_handle(),
            pending_remote: Vec::new(),
            pending_task: None,
            composition_group: None,
            media_imports: 0,
            _keystrokes: keystrokes,
        };
        this.reconcile(window, cx);
        this
    }

    fn handle_block_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.completion.is_some()
            || self.is_composing(cx)
            || key.modifiers.platform
            || key.modifiers.control
            || key.modifiers.alt
            || !matches!(key.key.as_str(), "tab" | "enter" | "backspace" | "delete")
        {
            return false;
        }
        let Some(id) = self
            .order
            .iter()
            .find(|id| {
                self.views[*id]
                    .editor
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            })
            .cloned()
        else {
            return false;
        };
        let view = &self.views[&id];
        if matches!(
            view.editor.read(cx).block_kind_at_caret(),
            Some(NativeBlockKind::CodeBlock | NativeBlockKind::TableCell)
        ) {
            return false;
        }
        if matches!(key.key.as_str(), "backspace" | "delete") {
            let state = view.editor.read(cx);
            let native = state.model().document();
            if state.is_readonly()
                || !(view.kind == SectionKind::Text || view.kind.is_list_item())
                || !matches!(state.selection(), BlockSelection::Text { anchor, head }
                    if anchor == head && *head == native.first_caret())
                || !native.leaves().all(|leaf| {
                    matches!(
                        leaf.kind(),
                        NativeBlockKind::Paragraph | NativeBlockKind::Heading { .. }
                    ) && leaf.text().is_some_and(str::is_empty)
                })
            {
                return false;
            }
            match self.remove_empty_block(&id, key.key == "backspace", window, cx) {
                Ok(false) => return false,
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                }
                Ok(true) => {}
            }
            cx.stop_propagation();
            window.prevent_default();
            return true;
        }
        let exit_list = key.key == "enter"
            && !key.modifiers.shift
            && view.kind.is_list_item()
            && view.content.blocks.iter().all(|node| {
                node.kind == RichKind::Paragraph
                    && node.children.is_empty()
                    && node
                        .runs
                        .iter()
                        .all(|run| run.atom.is_none() && run.text.trim().is_empty())
            });
        if key.key == "enter" && !exit_list {
            return false;
        }
        let result = (|| -> anyhow::Result<bool> {
            self.flush_local_edits(window, cx)?;
            self.capture_selections(cx)?;
            let vector = self.document.state_vector();
            let changed = if exit_list {
                self.document.exit_list_item(&id)?
            } else if key.modifiers.shift {
                self.document.outdent_section(&id)?
            } else {
                self.document.indent_section(&id)?
            };
            if changed {
                self.reconcile(window, cx);
                self.views[&id]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
                self.publish(&vector, cx);
            }
            Ok(changed)
        })();
        match result {
            Ok(false) => return false,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
            Ok(true) => {}
        }
        cx.stop_propagation();
        window.prevent_default();
        true
    }

    fn remove_empty_block(
        &mut self,
        id: &str,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<bool> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let index = self
            .order
            .iter()
            .position(|section| section == id)
            .context("focused block is not visible")?;
        let previous = if backward {
            index.checked_sub(1).map(|index| self.order[index].clone())
        } else {
            None
        };
        let vector = self.document.state_vector();
        if !self.document.remove_empty_block(id)? {
            return Ok(false);
        }
        self.completion = None;
        self.selection_dropdown = None;
        self.selection_link = None;
        self.reconcile(window, cx);
        let target = if self.views.contains_key(id) {
            Some((id.to_owned(), false))
        } else if backward {
            previous
                .map(|id| (id, true))
                .or_else(|| self.order.get(index).cloned().map(|id| (id, false)))
        } else {
            self.order
                .get(index)
                .cloned()
                .map(|id| (id, false))
                .or_else(|| self.order.last().cloned().map(|id| (id, true)))
        };
        if let Some((target, at_end)) = target {
            let view = self
                .views
                .get_mut(&target)
                .context("neighboring block is not editable")?;
            view.show_source = true;
            view.editor.update(cx, |state, cx| -> anyhow::Result<()> {
                let document = state.model().document();
                let selection = if document.leaves().next().is_some() {
                    BlockSelection::caret(if at_end {
                        document.last_caret()
                    } else {
                        document.first_caret()
                    })
                } else {
                    state.selection().clone()
                };
                state
                    .set_selection(selection, cx)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                state.focus(window, cx);
                Ok(())
            })?;
            self.cache_selection(&target, cx)?;
        } else {
            self.focus_scope.focus(window, cx);
        }
        self.publish(&vector, cx);
        Ok(true)
    }

    fn is_collapsed(&self, id: &str) -> bool {
        let Some(default_collapsed) = self
            .tree
            .get(id)
            .and_then(|node| node.kind.default_collapsed())
        else {
            return false;
        };
        self.collapsed.get(id).copied().unwrap_or(default_collapsed)
    }

    fn toggle_collapsed(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.flush_local_edits(window, cx) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let collapsed = !self.is_collapsed(id);
        if collapsed {
            let descendant_focused = self.views.iter().any(|(candidate, view)| {
                candidate != id
                    && self.is_within(candidate, id)
                    && view.editor.read(cx).focus_handle(cx).is_focused(window)
            });
            if descendant_focused {
                if let Some(view) = self.views.get(id) {
                    view.editor.update(cx, |state, cx| state.focus(window, cx));
                }
            }
            if self.completion.as_ref().is_some_and(|completion| {
                completion.section != id && self.is_within(&completion.section, id)
            }) {
                self.completion = None;
            }
        }
        self.collapsed.insert(id.to_owned(), collapsed);
        self.reconcile(window, cx);
    }

    fn expand_ancestors(&mut self, id: &str) -> bool {
        let mut parent = self.tree.get(id).and_then(|node| node.parent.clone());
        let mut changed = false;
        while let Some(id) = parent {
            if self.is_collapsed(&id) {
                self.collapsed.insert(id.clone(), false);
                changed = true;
            }
            parent = self.tree.get(&id).and_then(|node| node.parent.clone());
        }
        changed
    }

    fn focus_block_at(
        &mut self,
        id: &str,
        point: Option<RichPoint>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.expand_ancestors(id) {
            self.reconcile(window, cx);
        }
        let view = self
            .views
            .get_mut(id)
            .context("new block is not editable")?;
        let map = view
            .rich_map
            .as_ref()
            .context("new block has no semantic mapping")?;
        let position = point
            .as_ref()
            .and_then(|point| map.position(point, Assoc::After))
            .or_else(|| map.first_position());
        if view.previewable {
            view.show_source = true;
        }
        view.editor.update(cx, |state, cx| -> anyhow::Result<()> {
            let selection = position
                .map(BlockSelection::caret)
                .unwrap_or_else(|| state.selection().clone());
            state
                .set_selection(selection, cx)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            state.focus(window, cx);
            Ok(())
        })?;
        self.cache_selection(id, cx)?;
        cx.notify();
        Ok(())
    }

    fn normalize_edit(
        &mut self,
        id: &str,
        content: &RichDocument,
        caret: RichPoint,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        fn contains(nodes: &[RichNode], id: &str) -> bool {
            nodes
                .iter()
                .any(|node| node.id == id || contains(&node.children, id))
        }
        fn locate(specs: &[BlockSpec], node: &str, path: &mut Vec<usize>) -> bool {
            for (index, spec) in specs.iter().enumerate() {
                path.push(index);
                if contains(&spec.content.blocks, node) || locate(&spec.children, node, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut specs = specs_from_rich(content)?;
        if matches!(self.views[id].kind, SectionKind::Collapsible { .. }) {
            let mut roots = specs.into_iter();
            let first = roots
                .next()
                .context("collapsible title has no semantic content")?;
            let mut title = if first.kind == SectionKind::Text {
                first
            } else {
                BlockSpec {
                    kind: SectionKind::Text,
                    content: RichDocument::paragraph("Details"),
                    children: vec![first],
                }
            };
            title.kind = self.views[id].kind.clone();
            title.children.extend(roots);
            specs = vec![title];
        }
        let mut path = Vec::new();
        anyhow::ensure!(
            locate(&specs, &caret.node, &mut path),
            "split block has no semantic caret target"
        );
        let vector = self.document.state_vector();
        self.document.begin_history_group();
        let result = (|| -> anyhow::Result<()> {
            let identities = self.document.replace_section(id, &specs)?;
            let target = identities
                .into_iter()
                .find(|(candidate, _)| *candidate == path)
                .map(|(_, id)| id)
                .context("split block identity was not retained")?;
            if matches!(self.views[id].kind, SectionKind::CollapsibleListItem { .. }) {
                self.collapsed.insert(target.clone(), false);
            }
            self.reconcile(window, cx);
            if focused {
                self.focus_block_at(&target, Some(caret), window, cx)?;
            }
            self.publish(&vector, cx);
            Ok(())
        })();
        self.document.end_history_group();
        result
    }

    fn set_checked(
        &mut self,
        id: &str,
        checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let vector = self.document.state_vector();
        self.document.set_kind(
            id,
            SectionKind::ListItem {
                checked: Some(checked),
            },
        )?;
        self.reconcile(window, cx);
        self.publish(&vector, cx);
        Ok(())
    }

    fn insert_markup_after(
        &mut self,
        id: &str,
        markdown: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let parent = self.views[id].parent.clone();
        let siblings = self.document.children(parent.as_deref());
        let index = siblings
            .iter()
            .position(|section| section.id == id)
            .context("block is not in its parent")?;
        let before = siblings.get(index + 1).map(|section| section.id.as_str());
        let in_list = self.views[id].kind.is_list_item();
        let parsed = parse_blocks(markdown)?;
        let vector = self.document.state_vector();
        for mut spec in parsed.blocks {
            if in_list {
                if spec.kind == SectionKind::Text {
                    spec.kind = SectionKind::ListItem { checked: None };
                } else if let SectionKind::Collapsible { default_collapsed } = spec.kind {
                    spec.kind = SectionKind::CollapsibleListItem { default_collapsed };
                } else {
                    spec = BlockSpec {
                        kind: SectionKind::ListItem { checked: None },
                        content: RichDocument::paragraph(""),
                        children: vec![spec],
                    };
                }
            }
            self.document
                .insert_block(parent.as_deref(), before, &spec)?;
        }
        self.reconcile(window, cx);
        self.publish(&vector, cx);
        Ok(())
    }

    fn insert_divider(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let view = &self.views[id];
        // A list/disclosure title remains a title; insert into its body instead.
        let nested = view.kind.is_list_item() || view.kind.default_collapsed().is_some();
        let parent = if nested {
            Some(id.to_owned())
        } else {
            view.parent.clone()
        };
        let mut siblings = self
            .tree_order
            .iter()
            .filter(|candidate| self.tree[*candidate].parent == parent);
        let before = if nested {
            siblings.next().cloned()
        } else {
            siblings.find(|candidate| candidate.as_str() == id);
            siblings.next().cloned()
        };
        let replace = !nested
            && !self
                .tree
                .values()
                .any(|node| node.parent.as_deref() == Some(id))
            && matches!(view.content.blocks.as_slice(), [node]
                if matches!(node.kind, RichKind::Paragraph | RichKind::Heading)
                    && node.children.is_empty()
                    && node.runs.iter().all(|run| run.atom.is_none() && run.text.is_empty()));
        let mut rule = RichDocument::paragraph("");
        rule.blocks[0].kind = RichKind::HorizontalRule;
        let vector = self.document.state_vector();
        if replace {
            self.document.set_content(id, &rule)?;
        } else {
            self.document.insert_block(
                parent.as_deref(),
                before.as_deref(),
                &BlockSpec {
                    kind: SectionKind::Text,
                    content: rule,
                    children: vec![],
                },
            )?;
        }
        let continuation = self.document.insert_block(
            parent.as_deref(),
            before.as_deref(),
            &BlockSpec {
                kind: SectionKind::Text,
                content: RichDocument::paragraph(""),
                children: vec![],
            },
        )?;
        if nested {
            self.collapsed.insert(id.to_owned(), false);
        }
        self.reconcile(window, cx);
        self.focus_block_at(&continuation, None, window, cx)?;
        self.publish(&vector, cx);
        Ok(())
    }

    fn handle_completion_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.as_mut() else {
            return;
        };
        let Some(view) = self.views.get(&completion.section) else {
            return;
        };
        let state = view.editor.read(cx);
        if state.is_composing() || !state.focus_handle(cx).is_focused(window) {
            return;
        }
        match key {
            "escape" => self.completion = None,
            "down" if !completion.commands.is_empty() => {
                completion.selected = (completion.selected + 1) % completion.commands.len();
                completion.scroll_to_selected();
            }
            "up" if !completion.commands.is_empty() => {
                completion.selected = (completion.selected + completion.commands.len() - 1)
                    % completion.commands.len();
                completion.scroll_to_selected();
            }
            "enter" | "tab" if !completion.commands.is_empty() => {
                let index = completion.selected;
                self.accept_completion(index, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        window.prevent_default();
        cx.notify();
    }

    pub fn document(&self) -> &Document {
        &self.document
    }
    pub fn markdown(&self) -> String {
        self.document.markdown()
    }
    pub fn start_sync(&self) -> Vec<u8> {
        self.document.start_sync()
    }

    /// Handle a y-protocols sync message; send all returned messages to its sender.
    pub fn receive_sync(
        &mut self,
        message: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        if self.is_composing(cx) {
            let replies = self.document.reply_sync_without_applying(message)?;
            self.pending_remote
                .push(PendingRemote::Sync(message.to_vec()));
            self.schedule_pending_remote(window, cx);
            return Ok(replies);
        }
        self.drain_pending_remote(window, cx)?;
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let replies = self.document.receive_sync(message)?;
        self.completion = None;
        self.reconcile(window, cx);
        Ok(replies)
    }

    /// Apply a raw update, not a framed sync message. Safe for duplicate delivery.
    pub fn apply_update(
        &mut self,
        update: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.is_composing(cx) {
            self.pending_remote
                .push(PendingRemote::Update(update.to_vec()));
            self.schedule_pending_remote(window, cx);
            return Ok(());
        }
        self.drain_pending_remote(window, cx)?;
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        self.document.apply_update(update)?;
        self.completion = None;
        self.reconcile(window, cx);
        Ok(())
    }

    pub fn insert_section(
        &mut self,
        before: Option<&str>,
        markdown: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<SectionId> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let vector = self.document.state_vector();
        let id = self.document.insert_section(before, markdown)?;
        self.reconcile(window, cx);
        self.publish(&vector, cx);
        Ok(id)
    }

    pub fn move_section(
        &mut self,
        id: &str,
        before: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let vector = self.document.state_vector();
        self.document.move_section(id, before)?;
        self.reconcile(window, cx);
        self.publish(&vector, cx);
        Ok(())
    }

    pub fn remove_section(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let index = self
            .order
            .iter()
            .position(|section| section == id)
            .unwrap_or(0);
        let restore_focus = self
            .views
            .get(id)
            .is_some_and(|view| view.editor.read(cx).focus_handle(cx).is_focused(window))
            || (self.focus_scope.contains_focused(window, cx)
                && !self
                    .views
                    .values()
                    .any(|view| view.editor.read(cx).focus_handle(cx).is_focused(window)));
        let vector = self.document.state_vector();
        self.document.remove_section(id)?;
        self.completion = None;
        self.reconcile(window, cx);
        if restore_focus {
            if let Some(id) = self
                .order
                .get(index.min(self.order.len().saturating_sub(1)))
            {
                self.views[id]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
            } else {
                self.focus_scope.focus(window, cx);
            }
        }
        self.publish(&vector, cx);
        Ok(())
    }

    fn publish(&mut self, vector: &[u8], cx: &mut Context<Self>) {
        match self.document.encode_update(vector) {
            Ok(update) => cx.emit(DocumentEvent::Changed { update }),
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn is_composing(&self, cx: &App) -> bool {
        self.views
            .values()
            .any(|view| view.editor.read(cx).is_composing())
    }

    fn finish_composition_group(&mut self, cx: &App) {
        if self.composition_group.is_some() && !self.is_composing(cx) {
            self.composition_group = None;
            self.document.end_history_group();
        }
    }

    fn flush_local_edits(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let pending: Vec<_> = self
            .views
            .iter()
            .filter(|(_, view)| view.revision != view.editor.read(cx).revision())
            .map(|(id, _)| id.clone())
            .collect();
        for id in pending {
            self.sync_native_change(&id, None, window, cx)?;
        }
        self.finish_composition_group(cx);
        Ok(())
    }

    fn sync_native_change(
        &mut self,
        id: &str,
        origin: Option<EditOrigin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Some(view) = self.views.get(id) else {
            return Ok(());
        };
        let state = view.editor.read(cx);
        if state.revision() == view.revision {
            return Ok(());
        }
        let content = state
            .model()
            .rich_document()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let map = RichMap::new(&content, state.model())?;
        let revision = state.revision();
        let native_selection = state.selection().clone();
        let composing = state.is_composing();
        let structural = {
            let mut roots = content
                .blocks
                .iter()
                .filter(|node| node.kind != RichKind::Definition);
            roots
                .next()
                .is_some_and(|node| matches!(node.kind, RichKind::List | RichKind::Quote))
                || roots.next().is_some()
        };
        if structural && !composing {
            let caret = map
                .selection_offsets(&native_selection)
                .context("split selection has no semantic position")?
                .head;
            let focused = state.focus_handle(cx).is_focused(window);
            return self.normalize_edit(id, &content, caret, focused, window, cx);
        }
        let changed = content != view.content;
        let origin_changed = origin.is_some() && origin != view.last_origin;
        let isolated = origin.is_none_or(|origin| {
            !matches!(
                origin,
                EditOrigin::Typing
                    | EditOrigin::Backspace
                    | EditOrigin::DeleteForward
                    | EditOrigin::WordBackspace
                    | EditOrigin::WordDelete
                    | EditOrigin::Ime
            )
        });
        if !composing && origin.is_some_and(|origin| origin != EditOrigin::Ime) {
            self.finish_composition_group(cx);
        }
        if composing && self.composition_group.is_none() {
            self.document.begin_history_group();
            self.composition_group = Some(id.to_owned());
        }
        if changed && (isolated || origin_changed) {
            self.document.stop_capturing();
        }
        let vector = changed.then(|| self.document.state_vector());
        if changed {
            self.document.set_content(id, &content)?;
        }
        let selection =
            RelativeSelection::capture(&self.document, id, &content, &map, &native_selection)?;
        let view = self
            .views
            .get_mut(id)
            .expect("section retained during local update");
        view.content = content;
        view.revision = revision;
        view.rich_map = Some(map);
        view.selection = Some(selection);
        view.native_selection = native_selection;
        view.last_origin = origin;
        if let Some(vector) = vector {
            self.schedule_preview(id, cx);
            self.sync_media_view(id, window, cx);
            self.publish(&vector, cx);
        }
        if changed && isolated {
            self.document.stop_capturing();
        }
        self.finish_composition_group(cx);
        Ok(())
    }

    fn capture_selections(&mut self, cx: &App) -> anyhow::Result<()> {
        for (id, view) in &mut self.views {
            let map = view
                .rich_map
                .as_ref()
                .context("section semantic map is unavailable")?;
            view.selection = Some(RelativeSelection::capture(
                &self.document,
                id,
                &view.content,
                map,
                view.editor.read(cx).selection(),
            )?);
        }
        Ok(())
    }

    fn cache_selection(&mut self, id: &str, cx: &App) -> anyhow::Result<()> {
        let Some(view) = self.views.get_mut(id) else {
            return Ok(());
        };
        let state = view.editor.read(cx);
        if view.native_selection != *state.selection() && !state.is_composing() {
            self.document.stop_capturing();
        }
        let map = view
            .rich_map
            .as_ref()
            .context("section semantic map is unavailable")?;
        view.selection = Some(RelativeSelection::capture(
            &self.document,
            id,
            &view.content,
            map,
            state.selection(),
        )?);
        view.native_selection = state.selection().clone();
        Ok(())
    }

    fn drain_pending_remote(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.pending_remote.is_empty() || self.is_composing(cx) {
            return Ok(());
        }
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let mut error = None;
        for pending in std::mem::take(&mut self.pending_remote) {
            let result = match pending {
                PendingRemote::Update(update) => self.document.apply_update(&update),
                PendingRemote::Sync(message) => self.document.receive_sync(&message).map(|_| ()),
            };
            if let Err(failure) = result {
                error = Some(failure);
            }
        }
        self.completion = None;
        self.reconcile(window, cx);
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn schedule_pending_remote(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.pending_task.is_some() {
            return;
        }
        // Upstream unmark_text clears IME composition without emitting an event.
        // Check only while a remote update is waiting for that composition.
        self.pending_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let done = this
                    .update_in(cx, |this, window, cx| {
                        if this.is_composing(cx) {
                            return false;
                        }
                        this.finish_composition_group(cx);
                        if let Err(error) = this.drain_pending_remote(window, cx) {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                        this.pending_task = None;
                        true
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    /// Undo this replica's last local operation, never a remote operation.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<bool> {
        self.history(false, window, cx)
    }

    /// Redo this replica's last undone local operation.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<bool> {
        self.history(true, window, cx)
    }

    fn history(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<bool> {
        if self.is_composing(cx) {
            return Ok(false);
        }
        self.drain_pending_remote(window, cx)?;
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let vector = self.document.state_vector();
        let changed = if redo {
            self.document.redo()
        } else {
            self.document.undo()
        };
        if changed {
            self.completion = None;
            self.reconcile(window, cx);
            self.publish(&vector, cx);
        }
        Ok(changed)
    }

    fn handle_history_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !(key.modifiers.platform || key.modifiers.control)
            || self.selection_input_focused(window, cx)
            || key.modifiers.alt
            || !matches!(key.key.as_str(), "z" | "y")
            || !self.focus_scope.contains_focused(window, cx)
            || self.is_composing(cx)
        {
            return false;
        }
        let redo = key.key == "y" || key.modifiers.shift;
        if let Err(error) = self.history(redo, window, cx) {
            self.error = Some(error.to_string());
            cx.notify();
        }
        cx.stop_propagation();
        window.prevent_default();
        true
    }

    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.order.iter().enumerate().find_map(|(index, id)| {
            self.views
                .get(id)
                .filter(|view| view.editor.read(cx).focus_handle(cx).is_focused(window))
                .map(|_| (index, id.clone()))
        });
        let sections = self.document.sections();
        self.tree_order = sections.iter().map(|section| section.id.clone()).collect();
        self.tree = sections
            .iter()
            .map(|section| {
                (
                    section.id.clone(),
                    TreeNode {
                        parent: section.parent.clone(),
                        kind: section.kind.clone(),
                    },
                )
            })
            .collect();
        self.collapsed.retain(|id, _| self.tree.contains_key(id));
        if let Some((_, id)) = focused.as_ref() {
            self.expand_ancestors(id);
        }
        let mut hidden = HashSet::new();
        for id in &self.tree_order {
            if self.tree[id]
                .parent
                .as_ref()
                .is_some_and(|parent| hidden.contains(parent) || self.is_collapsed(parent))
            {
                hidden.insert(id.clone());
            }
        }
        self.block_selection
            .retain(|id| self.tree.contains_key(id) && !hidden.contains(id));
        if self
            .block_anchor
            .as_ref()
            .is_some_and(|id| !self.tree.contains_key(id))
        {
            self.block_anchor = None;
        }
        self.order = sections
            .iter()
            .filter(|section| section.kind.is_editable() && !hidden.contains(&section.id))
            .map(|section| section.id.clone())
            .collect();
        self.views.retain(|id, _| {
            self.tree
                .get(id)
                .is_some_and(|node| node.kind.is_editable())
        });
        if let Some((id, _)) = &self.expanded_diagram {
            if !self.views.contains_key(id) {
                let id = id.clone();
                self.sync_expanded_diagram(&id, cx);
            }
        }
        let mut changed = Vec::new();
        self.list_ends.clear();
        self.list_starts.clear();
        let mut layout = HashMap::<SectionId, (SectionKind, usize)>::new();
        let mut counters = HashMap::<SectionId, u64>::new();
        let mut lists = Vec::<OpenList>::new();
        for section in sections {
            while lists.last().is_some_and(|list| list.depth >= section.depth) {
                lists
                    .pop()
                    .expect("list stack is not empty")
                    .finish(&mut self.list_ends);
            }
            let parent_layout = section.parent.as_ref().and_then(|id| layout.get(id));
            let indent = parent_layout.map_or(0, |(kind, indent)| {
                if matches!(kind, SectionKind::List { .. }) {
                    *indent
                } else {
                    indent + 1
                }
            });
            let list_style = parent_layout.and_then(|(kind, _)| match kind {
                SectionKind::List { ordered, start } => Some((*ordered, *start)),
                _ => None,
            });
            layout.insert(section.id.clone(), (section.kind.clone(), indent));
            if !hidden.contains(&section.id) && matches!(section.kind, SectionKind::List { .. }) {
                lists.push(OpenList {
                    depth: section.depth,
                    group: section.id.clone(),
                    indent,
                    last: None,
                });
            }
            if !section.kind.is_editable() {
                continue;
            }
            if !hidden.contains(&section.id) {
                for list in &mut lists {
                    if list.last.is_none() {
                        self.list_starts
                            .entry(section.id.clone())
                            .or_default()
                            .push(list.group.clone());
                    }
                    list.last = Some(section.id.clone());
                }
            }
            let ordinal = if section.kind.is_list_item() {
                if let Some(parent) = &section.parent {
                    if let Some(count) = counters.get_mut(parent) {
                        let ordinal = *count;
                        *count += 1;
                        ordinal
                    } else {
                        counters.insert(parent.clone(), 1);
                        0
                    }
                } else {
                    0
                }
            } else {
                0
            };
            let marker = if let SectionKind::ListItem { checked } = section.kind {
                let label = match list_style {
                    Some((true, start)) => format!("{}.", start.saturating_add(ordinal)).into(),
                    _ if checked.is_some() => SharedString::default(),
                    _ => "•".into(),
                };
                Some(ListMarker { label, checked })
            } else {
                None
            };
            if let Some(view) = self.views.get_mut(&section.id) {
                view.parent = section.parent;
                view.kind = section.kind;
                view.indent = indent;
                view.marker = marker;
                if view.content != section.content
                    || view.revision != view.editor.read(cx).revision()
                {
                    let result = view
                        .editor
                        .update(cx, |state, cx| -> anyhow::Result<RichMap> {
                            state
                                .set_document(&section.content, cx)
                                .map_err(|error| anyhow::anyhow!("{error}"))?;
                            let map = RichMap::new(&section.content, state.model())?;
                            if let Some(selection) = view
                                .selection
                                .as_ref()
                                .and_then(|selection| selection.resolve(&self.document, &map))
                            {
                                state
                                    .set_selection(selection, cx)
                                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                            }
                            Ok(map)
                        });
                    match result {
                        Ok(map) => {
                            view.rich_map = Some(map);
                            view.content = section.content;
                            view.revision = view.editor.read(cx).revision();
                            view.native_selection = view.editor.read(cx).selection().clone();
                            changed.push(section.id);
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            } else {
                let mut import_error = None;
                let editor = cx.new(|cx| {
                    let mut state = BlockEditorState::new(window, cx);
                    if let Err(error) = state.set_document(&section.content, cx) {
                        import_error = Some(error.to_string());
                    }
                    state.set_scroll_handle(self.scroll.clone());
                    state
                });
                if let Some(error) = import_error {
                    self.error = Some(error);
                    continue;
                }
                let state = editor.read(cx);
                let map = RichMap::new(&section.content, state.model());
                let (map, selection) = match map.and_then(|map| {
                    let selection = RelativeSelection::capture(
                        &self.document,
                        &section.id,
                        &section.content,
                        &map,
                        state.selection(),
                    )?;
                    Ok((map, selection))
                }) {
                    Ok((map, selection)) => (Some(map), Some(selection)),
                    Err(error) => {
                        self.error = Some(error.to_string());
                        (None, None)
                    }
                };
                let revision = state.revision();
                let native_selection = state.selection().clone();
                let id = section.id.clone();
                let subscription = cx.subscribe_in(
                    &editor,
                    window,
                    move |this, editor, event: &BlockEditorEvent, window, cx| {
                        let result = match event {
                            BlockEditorEvent::Changed { origin, .. } => {
                                this.sync_native_change(&id, Some(*origin), window, cx)
                            }
                            BlockEditorEvent::SelectionChanged => this
                                .sync_native_change(&id, None, window, cx)
                                .and_then(|_| this.cache_selection(&id, cx)),
                            BlockEditorEvent::Focus | BlockEditorEvent::Blur => {
                                cx.notify();
                                return;
                            }
                            _ => return,
                        };
                        if let Err(error) = result {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                        if let Err(error) = this.drain_pending_remote(window, cx) {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                        this.update_completion(&id, &editor, cx);
                    },
                );
                self.views.insert(
                    section.id.clone(),
                    SectionView {
                        editor,
                        content: section.content,
                        preview: None,
                        preview_error: None,
                        previewable: false,
                        show_source: false,
                        revision,
                        rich_map: map,
                        selection,
                        native_selection,
                        parent: section.parent,
                        kind: section.kind,
                        indent,
                        marker,
                        last_origin: None,
                        _subscription: subscription,
                        preview_task: None,
                        media: None,
                        navigation_anchor: ScrollAnchor::for_handle(self.scroll.clone()),
                    },
                );
                changed.push(section.id);
            }
        }
        while let Some(list) = lists.pop() {
            list.finish(&mut self.list_ends);
        }
        for id in changed {
            self.schedule_preview(&id, cx);
            self.sync_media_view(&id, window, cx);
        }
        if let Some((index, _)) = focused.filter(|(_, id)| !self.views.contains_key(id)) {
            if let Some(id) = self
                .order
                .get(index.min(self.order.len().saturating_sub(1)))
            {
                self.views[id]
                    .editor
                    .update(cx, |state, cx| state.focus(window, cx));
            } else {
                self.focus_scope.focus(window, cx);
            }
        }
        cx.notify();
    }

    fn schedule_preview(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        let Some((extension, source)) = self.extensions.preview_source(&view.content) else {
            view.previewable = false;
            view.preview_task = None;
            view.preview = None;
            view.preview_error = None;
            self.sync_expanded_diagram(id, cx);
            return;
        };
        view.previewable = true;
        view.preview = None;
        view.preview_error = None;
        let extension = Arc::clone(extension);
        let section = id.to_owned();
        let render = cx.background_executor().spawn(async move {
            extension
                .render_svg(&source)
                .map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())))
                .map_err(|error| format!("{error:#}"))
        });
        view.preview_task = Some(cx.spawn(async move |this, cx| {
            let preview = render.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.views.get_mut(&section) {
                    view.preview = None;
                    view.preview_error = None;
                    match preview {
                        Ok(image) => view.preview = Some(image),
                        Err(error) => view.preview_error = Some(error),
                    }
                    this.sync_expanded_diagram(&section, cx);
                    cx.notify();
                }
            });
        }));
        self.sync_expanded_diagram(id, cx);
    }

    fn sync_expanded_diagram(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((section, viewer)) = &self.expanded_diagram else {
            return;
        };
        if section != id {
            return;
        }
        let Some(viewer) = viewer.upgrade() else {
            self.expanded_diagram = None;
            return;
        };
        let (image, error) = match self.views.get(id) {
            Some(view) if view.previewable => (view.preview.clone(), view.preview_error.clone()),
            _ => (
                None,
                Some("This diagram is no longer in the document.".to_owned()),
            ),
        };
        viewer.update(cx, |viewer, cx| viewer.set_preview(image, error, cx));
    }

    fn expand_diagram(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(image) = self.views.get(id).and_then(|view| view.preview.clone()) else {
            return;
        };
        if window.has_active_dialog(cx) {
            return;
        }
        self.completion = None;
        self.selection_dropdown = None;
        self.selection_link = None;
        self.selection_toolbar_target = None;
        let viewer = cx.new(|cx| DiagramViewer::new(image, window, cx));
        self.expanded_diagram = Some((id.to_owned(), viewer.downgrade()));
        let owner = cx.weak_entity();
        let viewer_id = viewer.entity_id();
        window.open_dialog(cx, move |dialog, window, _| {
            let viewport = window.viewport_size();
            let viewer = viewer.clone();
            dialog
                .width((viewport.width - px(48.)).max(px(1.)))
                .h((viewport.height - px(48.)).max(px(1.)))
                .min_h_0()
                .centered()
                .p_0()
                .on_close({
                    let owner = owner.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this
                                .expanded_diagram
                                .as_ref()
                                .is_some_and(|(_, active)| active.entity_id() == viewer_id)
                            {
                                this.expanded_diagram = None;
                                cx.notify();
                            }
                        });
                    }
                })
                .content(move |content, _, _| {
                    content.p_0().min_h_0().size_full().child(viewer.clone())
                })
        });
        cx.notify();
    }

    fn toggle_preview(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        if !view.previewable {
            return;
        }
        view.show_source = !view.show_source;
        if view.show_source {
            view.editor.update(cx, |state, cx| state.focus(window, cx));
        } else if self
            .completion
            .as_ref()
            .is_some_and(|completion| completion.section == id)
        {
            self.completion = None;
        }
        cx.notify();
    }

    fn open_command_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        self.selection_dropdown = None;
        self.selection_link = None;
        view.show_source = true;
        view.editor.update(cx, |state, cx| state.focus(window, cx));
        if !self
            .completion
            .as_ref()
            .is_some_and(|menu| menu.section == id)
        {
            self.completion = Some(Completion {
                section: id.to_owned(),
                range: None,
                anchor_to_button: true,
                query: String::new(),
                commands: self.extensions.commands.search(""),
                selected: 0,
                scroll: ScrollHandle::new(),
            });
        } else if let Some(menu) = &mut self.completion {
            menu.anchor_to_button = true;
        }
        cx.notify();
    }

    fn update_completion(
        &mut self,
        id: &str,
        editor: &Entity<BlockEditorState>,
        cx: &mut Context<Self>,
    ) {
        let state = editor.read(cx);
        let candidate = state.caret_prefix().and_then(|prefix| {
            let slash = prefix.rfind('/')?;
            if slash > 0 && !prefix[..slash].ends_with(char::is_whitespace) {
                return None;
            }
            let query = &prefix[slash + 1..];
            if query.contains(char::is_whitespace) {
                return None;
            }
            Some((
                query.to_owned(),
                state.capture_suffix(prefix.len() - slash).ok()?,
            ))
        });
        self.completion = candidate.map(|(query, range)| {
            let selected = self
                .completion
                .as_ref()
                .filter(|c| c.section == id && c.query == query)
                .map(|c| c.selected);
            let commands = self.extensions.commands.search(&query);
            let scroll = self
                .completion
                .as_ref()
                .filter(|c| c.section == id && c.query == query)
                .map_or_else(ScrollHandle::new, |c| c.scroll.clone());
            let completion = Completion {
                section: id.to_owned(),
                range: Some(range),
                anchor_to_button: false,
                selected: selected
                    .unwrap_or_else(|| {
                        commands
                            .iter()
                            .position(|command| command.id.eq_ignore_ascii_case(&query))
                            .unwrap_or(0)
                    })
                    .min(commands.len().saturating_sub(1)),
                query,
                commands,
                scroll,
            };
            if selected.is_none() && !completion.commands.is_empty() {
                completion.scroll_to_selected();
            }
            completion
        });
        cx.notify();
    }

    fn accept_completion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(action) = completion
            .commands
            .get(index)
            .map(|command| command.action.clone())
        else {
            return;
        };
        self.execute_command(&completion.section, action, completion.range, window, cx);
    }

    fn execute_command(
        &mut self,
        id: &str,
        action: CommandAction,
        mut range: Option<CapturedRange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.views.get(id) else {
            return;
        };
        let editor = view.editor.clone();
        let structural = matches!(
            &action,
            CommandAction::Editor(
                EditorCommand::ToggleList { .. }
                    | EditorCommand::ToggleTask
                    | EditorCommand::ToggleBlockquote
                    | EditorCommand::Indent
                    | EditorCommand::Outdent
                    | EditorCommand::InsertBlock(NativeBlockKind::HorizontalRule)
            )
        );
        self.document.begin_history_group();
        let result = (|| -> anyhow::Result<()> {
            if !matches!(&action, CommandAction::InsertMedia(_)) {
                let native = editor.update(cx, |state, cx| -> anyhow::Result<()> {
                    if let Some(range) = &mut range {
                        state
                            .replace_captured(range, "", cx)
                            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                    }
                    if let CommandAction::Editor(command) = &action {
                        if !structural {
                            state
                                .run(command.clone(), cx)
                                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                        }
                    }
                    state.focus(window, cx);
                    Ok(())
                });
                self.sync_native_change(id, Some(EditOrigin::Command("format")), window, cx)?;
                native?;
            }
            match action {
                CommandAction::InsertMarkdown(markdown) => {
                    self.insert_markup_after(id, &markdown, window, cx)?
                }
                CommandAction::Editor(EditorCommand::InsertBlock(
                    NativeBlockKind::HorizontalRule,
                )) => {
                    self.insert_divider(id, window, cx)?;
                }
                CommandAction::Editor(command) if structural => {
                    let vector = self.document.state_vector();
                    match command {
                        EditorCommand::ToggleList { ordered } => {
                            self.document.wrap_list(id, ordered, false)?
                        }
                        EditorCommand::ToggleTask => self.document.wrap_list(id, false, true)?,
                        EditorCommand::ToggleBlockquote => self.document.wrap_quote(id)?,
                        EditorCommand::Indent => {
                            self.document.indent_section(id)?;
                        }
                        EditorCommand::Outdent => {
                            self.document.outdent_section(id)?;
                        }
                        _ => unreachable!("structural command matched above"),
                    }
                    self.reconcile(window, cx);
                    self.publish(&vector, cx);
                }
                CommandAction::Collapsible => {
                    let vector = self.document.state_vector();
                    let child = self.document.wrap_collapsible(id)?;
                    self.collapsed.insert(id.to_owned(), false);
                    self.reconcile(window, cx);
                    self.focus_block_at(&child, None, window, cx)?;
                    self.publish(&vector, cx);
                }
                CommandAction::CollapsibleList => {
                    let vector = self.document.state_vector();
                    self.document.wrap_collapsible_list(id)?;
                    self.collapsed.insert(id.to_owned(), false);
                    self.reconcile(window, cx);
                    // Like other list conversions, retain the selected title text.
                    self.publish(&vector, cx);
                }
                CommandAction::Editor(_) => {}
                CommandAction::InsertMedia(kind) => {
                    self.prompt_media(id, kind, range.take(), window, cx)?;
                }
            }
            Ok(())
        })();
        self.document.end_history_group();
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        cx.notify();
    }

    fn menu(&self, id: &str, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let completion = self.completion.as_ref().filter(|c| c.section == id)?;
        let selected = completion.selected;
        let mut items = div()
            .id("slash-menu-items")
            .w_full()
            .max_h(px(320.))
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&completion.scroll)
            .flex()
            .flex_col()
            .gap_1();
        let menu = div()
            .id("slash-menu")
            .w_full()
            .max_w(px(420.))
            .when(completion.anchor_to_button, |menu| {
                menu.w(px(420.).min((window.viewport_size().width - px(16.)).max(px(0.))))
            })
            .occlude()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .text_color(cx.theme().popover_foreground)
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if completion.range.is_some() {
                        "Scroll to browse · Type to filter · ↑ ↓ · Enter · Esc"
                    } else {
                        "Scroll to browse · Type / to filter · ↑ ↓ · Enter · Esc"
                    }),
            );
        if completion.commands.is_empty() {
            items = items.child(div().p_2().child("No matching commands"));
        }
        for (index, command) in completion.commands.iter().enumerate() {
            if index == 0 || completion.commands[index - 1].group != command.group {
                let heading = div()
                    .id(SharedString::from(format!("slash-group-{}", command.group)))
                    .flex_shrink_0()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .text_base()
                    .font_semibold()
                    .text_color(cx.theme().popover_foreground)
                    .child(command.group.clone());
                #[cfg(test)]
                let heading = {
                    use gpui_kit::test::TestSupportExt;
                    heading.test_support()
                };
                items = items.child(heading);
            }
            let icon = div()
                .id("slash-command-icon")
                .size_6()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(self.extensions.commands.icon(command.icon).size(px(20.)));
            let label = div()
                .id("slash-command-label")
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_sm().font_medium().child(command.title.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(command.description.clone()),
                );
            #[cfg(test)]
            let (icon, label) = {
                use gpui_kit::test::TestSupportExt;
                (icon.test_support(), label.test_support())
            };
            let row =
                div()
                    .id(SharedString::from(command.id.clone()))
                    .flex_shrink_0()
                    .pl_6()
                    .pr_3()
                    .py_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(index == selected, |el| el.bg(cx.theme().accent))
                    .hover(|el| el.bg(cx.theme().accent))
                    .cursor_pointer()
                    // Preserve the native capture until the command is accepted.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_completion(index, window, cx)
                    }))
                    .child(icon)
                    .child(label);
            #[cfg(test)]
            let row = {
                use gpui_kit::test::TestSupportExt;
                row.test_support()
            };
            items = items.child(row);
        }
        #[cfg(test)]
        let items = {
            use gpui_kit::test::TestSupportExt;
            items.test_support()
        };
        #[cfg(test)]
        let menu = {
            use gpui_kit::test::TestSupportExt;
            menu.test_support()
        };
        Some(menu.child(items).into_any_element())
    }

    fn is_within(&self, id: &str, ancestor: &str) -> bool {
        let mut current = Some(id);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.tree.get(id).and_then(|node| node.parent.as_deref());
        }
        false
    }

    fn block_is_selected(&self, id: &str) -> bool {
        self.block_selection
            .iter()
            .any(|selected| self.is_within(id, selected))
    }

    fn select_block(&mut self, id: &str, modifiers: Modifiers, cx: &mut Context<Self>) {
        if !self.tree.contains_key(id) {
            return;
        }
        // A new explicit selection supersedes deferred cleanup from an older drag.
        self.active_block_drag = None;
        if modifiers.alt {
            let mut current = Some(id);
            while let Some(candidate) = current {
                let node = &self.tree[candidate];
                if matches!(node.kind, SectionKind::List { .. }) {
                    let selected = candidate.to_owned();
                    self.block_selection.clear();
                    self.block_selection.insert(selected);
                    self.block_anchor = Some(id.to_owned());
                    cx.notify();
                    return;
                }
                current = node.parent.as_deref();
            }
        }
        if modifiers.shift {
            let anchor = self
                .block_anchor
                .as_ref()
                .and_then(|anchor| self.order.iter().position(|candidate| candidate == anchor));
            let head = self.order.iter().position(|candidate| candidate == id);
            if let (Some(anchor), Some(head)) = (anchor, head) {
                self.block_selection.clear();
                self.block_selection.extend(
                    self.order[anchor.min(head)..=anchor.max(head)]
                        .iter()
                        .cloned(),
                );
            } else {
                self.block_selection.insert(id.to_owned());
                self.block_anchor = Some(id.to_owned());
            }
        } else if modifiers.platform || modifiers.control {
            if !self.block_selection.remove(id) {
                self.block_selection.insert(id.to_owned());
            }
            self.block_anchor = Some(id.to_owned());
        } else {
            if !self.block_is_selected(id) {
                self.block_selection.clear();
                self.block_selection.insert(id.to_owned());
            }
            self.block_anchor = Some(id.to_owned());
        }
        cx.notify();
    }

    fn drag_roots(&self, grip: &str) -> Vec<DraggedRoot> {
        if !self.tree.contains_key(grip) {
            return Vec::new();
        }
        let mut selected = if self.block_is_selected(grip) {
            self.block_selection.clone()
        } else {
            HashSet::from([grip.to_owned()])
        };
        // Promote complete sibling selections, but never a plain one-item drag.
        if selected.len() > 1 {
            for group in self.tree_order.iter().rev() {
                if !matches!(self.tree[group].kind, SectionKind::List { .. }) {
                    continue;
                }
                let mut children = self
                    .tree_order
                    .iter()
                    .filter(|id| self.tree[*id].parent.as_ref() == Some(group))
                    .peekable();
                if children.peek().is_some() && children.all(|id| selected.contains(id)) {
                    selected.insert(group.clone());
                }
            }
        }
        self.tree_order
            .iter()
            .filter(|id| {
                selected.contains(*id)
                    && !selected
                        .iter()
                        .any(|ancestor| ancestor != *id && self.is_within(id, ancestor))
            })
            .map(|id| DraggedRoot {
                id: id.clone(),
                parent: self.tree[id].parent.clone(),
            })
            .collect()
    }

    fn accepts_drop(&self, dragged: &DraggedSection, target: &DropTarget, owner: EntityId) -> bool {
        if dragged.owner != owner {
            return false;
        }
        let roots = dragged.roots.borrow();
        if roots.is_empty()
            || target
                .parent
                .as_ref()
                .is_some_and(|parent| !self.tree.contains_key(parent))
            || target.before.as_ref().is_some_and(|before| {
                self.tree
                    .get(before)
                    .is_none_or(|node| node.parent != target.parent)
            })
        {
            return false;
        }
        let into_list = target
            .parent
            .as_ref()
            .is_some_and(|parent| matches!(self.tree[parent].kind, SectionKind::List { .. }));
        roots.iter().all(|root| {
            self.tree.get(&root.id).is_some_and(|node| {
                node.parent == root.parent
                    && (!into_list || node.kind.is_editable())
                    && !target
                        .parent
                        .as_ref()
                        .is_some_and(|parent| self.is_within(parent, &root.id))
                    && !target
                        .before
                        .as_ref()
                        .is_some_and(|before| self.is_within(before, &root.id))
            })
        })
    }

    fn list_boundary(&self, group: &str, after: bool) -> DropTarget {
        let list = &self.tree[group];
        let anchor = list
            .parent
            .as_ref()
            .filter(|owner| self.tree[*owner].kind.is_list_item())
            .map_or(group, String::as_str);
        let parent = self.tree[anchor].parent.clone();
        let before = if after {
            self.tree_order
                .iter()
                .skip_while(|id| id.as_str() != anchor)
                .skip(1)
                .find(|id| self.tree[*id].parent == parent)
                .cloned()
        } else {
            Some(anchor.to_owned())
        };
        DropTarget { parent, before }
    }

    fn with_drop_target(
        &self,
        element: Stateful<Div>,
        target: DropTarget,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let media_target = target.clone();
        let owner = cx.entity_id();
        let hover_owner = cx.weak_entity();
        let hover_target = target.clone();
        let drop_owner = cx.weak_entity();
        let drop_target = target.clone();
        element
            .drag_over::<DraggedSection>(move |style, dragged, _, cx| {
                if hover_owner.upgrade().is_some_and(|editor| {
                    editor.read(cx).accepts_drop(dragged, &hover_target, owner)
                }) {
                    style.bg(cx.theme().accent)
                } else {
                    style
                }
            })
            .can_drop(move |value, _, cx| {
                if value.downcast_ref::<ExternalPaths>().is_some() {
                    return true;
                }
                value
                    .downcast_ref::<DraggedSection>()
                    .is_some_and(|dragged| {
                        drop_owner.upgrade().is_some_and(|editor| {
                            editor.read(cx).accepts_drop(dragged, &drop_target, owner)
                        })
                    })
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSection, window, cx| {
                    this.drop_sections(dragged, &target, window, cx);
                }),
            )
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().secondary))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                cx.stop_propagation();
                this.import_dropped_media(paths.paths().to_vec(), media_target.clone(), window, cx);
            }))
    }

    fn drop_zone(
        &self,
        element: SharedString,
        target: DropTarget,
        indent: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zone = self.with_drop_target(
            div()
                .id(element)
                .w_full()
                .h(px(8.))
                .pl(px(24. + indent as f32 * 24.)),
            target,
            cx,
        );
        #[cfg(test)]
        let zone = {
            use gpui_kit::test::TestSupportExt;
            zone.test_support()
        };
        zone.into_any_element()
    }

    fn clear_block_selection(&mut self, cx: &mut Context<Self>) {
        let changed = !self.block_selection.is_empty() || self.block_anchor.is_some();
        self.block_selection.clear();
        self.block_anchor = None;
        self.active_block_drag = None;
        if changed {
            cx.notify();
        }
    }

    fn finish_block_drag(&mut self, cx: &mut Context<Self>) {
        if self.active_block_drag.is_some() {
            self.clear_block_selection(cx);
        }
    }

    fn drop_sections(
        &mut self,
        dragged: &DraggedSection,
        target: &DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_drop(dragged, target, cx.entity_id()) {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            self.flush_local_edits(window, cx)?;
            self.capture_selections(cx)?;
            if !self.accepts_drop(dragged, target, cx.entity_id()) {
                return Ok(());
            }
            let ids: Vec<_> = dragged
                .roots
                .borrow()
                .iter()
                .map(|root| root.id.clone())
                .collect();
            let vector = self.document.state_vector();
            self.document.move_sections(
                &ids,
                target.parent.as_deref(),
                target.before.as_deref(),
            )?;
            self.completion = None;
            self.block_selection.clear();
            self.block_anchor = None;
            self.active_block_drag = None;
            self.reconcile(window, cx);
            self.publish(&vector, cx);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error.to_string());
            cx.notify();
        }
    }
}

impl Render for BlockEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .max_w(px(860.))
            .mx_auto();
        for (index, id) in self.order.iter().enumerate() {
            let id = id.clone();
            let view = &self.views[&id];
            let editor = view.editor.clone();
            let media = view.media.as_ref().map(|(_, view)| view.clone());
            let preview = view.preview.clone();
            let preview_error = view.preview_error.clone();
            let previewable = view.previewable;
            let can_expand = preview.is_some();
            let show_source = !previewable || view.show_source;
            let pending = preview.is_none() && preview_error.is_none();
            let indent = view.indent;
            let parent = view.parent.clone();
            let marker = view.marker.clone();
            let collapsible = view.kind.default_collapsed().is_some();
            let collapsed = self.is_collapsed(&id);
            let collapse_id = id.clone();
            let toggle_id = id.clone();
            let expand_id = id.clone();
            let select_id = id.clone();
            let drag_id = id.clone();
            let drag_owner = cx.weak_entity();
            let remove_id = id.clone();
            let command_id = id.clone();
            let menu = if show_source {
                self.menu(&id, window, cx)
            } else {
                None
            };
            let (button_menu, menu) = if self
                .completion
                .as_ref()
                .is_some_and(|menu| menu.section == id && menu.anchor_to_button)
            {
                (menu, None)
            } else {
                (None, menu)
            };
            if let Some(groups) = self.list_starts.get(&id) {
                for group in groups {
                    content = content.child(self.drop_zone(
                        format!("list-before-{group}").into(),
                        self.list_boundary(group, false),
                        indent.saturating_sub(1),
                        cx,
                    ));
                }
            }
            let grip = div()
                .id(("grip", index))
                .self_center()
                .w(px(24.))
                .h(px(28.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .invisible()
                .group_hover("block-section", |style| style.visible())
                .cursor(CursorStyle::OpenHand)
                .text_color(cx.theme().muted_foreground)
                .child("⠿")
                .tooltip(|window, cx| {
                    Tooltip::new(
                        "Drag blocks · Shift-click: range · Cmd/Ctrl-click: toggle · Alt/Option-click: whole list",
                    )
                    .build(window, cx)
                })
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.grip_pressed = true;
                    window.prevent_default();
                    if !this.focus_scope.contains_focused(window, cx) {
                        this.focus_scope.focus(window, cx);
                    }
                    this.select_block(&select_id, event.modifiers, cx);
                }))
                .on_drag(
                    DraggedSection {
                        owner: cx.entity_id(),
                        roots: Rc::new(RefCell::new(Vec::new())),
                    },
                    move |dragged, _, _, cx| {
                        let roots = drag_owner
                            .upgrade()
                            .map_or_else(Vec::new, |editor| editor.read(cx).drag_roots(&drag_id));
                        *dragged.roots.borrow_mut() = roots;
                        let preview = cx.new(|cx| {
                            let owner = drag_owner.clone();
                            let preview_id = cx.entity_id();
                            cx.on_release(move |_, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if this.active_block_drag == Some(preview_id) {
                                        this.finish_block_drag(cx);
                                    }
                                });
                            })
                            .detach();
                            dragged.clone()
                        });
                        let _ = drag_owner.update(cx, |this, _| {
                            this.active_block_drag = Some(preview.entity_id());
                        });
                        preview
                    },
                );
            #[cfg(test)]
            let grip = {
                use gpui_kit::test::TestSupportExt;
                grip.test_support()
            };
            let body = if let Some(media) = media {
                div()
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .py_1()
                    .child(media)
                    .when_some(menu, |el, menu| el.child(menu))
            } else if show_source {
                let source = div()
                    .id(("section-source", index))
                    .px_3()
                    .py_1()
                    .child(markdown_editor::BlockEditor::new(&editor).min_h(px(24.)));
                #[cfg(test)]
                let source = {
                    use gpui_kit::test::TestSupportExt;
                    source.test_support()
                };
                div()
                    .flex_1()
                    .min_w_0()
                    .child(source)
                    .when_some(menu, |el, menu| el.child(div().px_3().pb_3().child(menu)))
            } else {
                let rendered = div()
                    .id(("section-preview", index))
                    .p_4()
                    .min_h(px(64.))
                    .when_some(preview, |el, image| {
                        el.child(
                            img(image)
                                .w_full()
                                .max_h(px(420.))
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    .when_some(preview_error, |el, error| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(format!("Diagram: {error}")),
                        )
                    })
                    .when(pending, |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Rendering diagram…"),
                        )
                    });
                #[cfg(test)]
                let rendered = {
                    use gpui_kit::test::TestSupportExt;
                    rendered.test_support()
                };
                div().flex_1().min_w_0().child(rendered)
            };
            let section = self
                .with_drop_target(
                    div()
                        .id(SharedString::from(id.clone()))
                        .group("block-section")
                        .anchor_scroll(Some(view.navigation_anchor.clone()))
                        .w_full()
                        .pl(px(indent as f32 * 24.))
                        .flex()
                        .items_start()
                        .when(self.block_is_selected(&id), |row| {
                            row.bg(cx.theme().secondary)
                        }),
                    DropTarget {
                        parent,
                        before: Some(id.clone()),
                    },
                    cx,
                )
                .child(
                    div()
                        .relative()
                        .self_center()
                        .w(px(24.))
                        .h(px(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .child(
                            div()
                                .invisible()
                                .group_hover("block-section", |style| style.visible())
                                .child(
                                    Button::new(("block-menu", index))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Plus)
                                        .accessibility_label("Open block commands")
                                        .tooltip("Open block commands")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_command_menu(&command_id, window, cx);
                                        })),
                                ),
                        )
                        .when_some(button_menu, |el, menu| {
                            el.child(
                                div().absolute().top_0().left_0().child(
                                    deferred(
                                        anchored()
                                            .anchor(Anchor::TopLeft)
                                            .position_mode(AnchoredPositionMode::Local)
                                            .attach(
                                                Bounds::new(
                                                    point(px(0.), px(0.)),
                                                    size(px(24.), px(28.)),
                                                ),
                                                Anchor::BottomLeft,
                                            )
                                            .offset(point(px(0.), px(4.)))
                                            .child(menu),
                                    )
                                    .with_priority(1),
                                ),
                            )
                        }),
                )
                .child(grip)
                .when(collapsible, |row| {
                    row.child(
                        Button::new(("toggle-collapse", index))
                            .ghost()
                            .small()
                            .icon(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .accessibility_label(if collapsed {
                                "Expand section"
                            } else {
                                "Collapse section"
                            })
                            .tooltip(if collapsed {
                                "Expand section"
                            } else {
                                "Collapse section"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_collapsed(&collapse_id, window, cx);
                            })),
                    )
                })
                .when_some(marker, |row, marker| {
                    row.child(
                        div()
                            .min_w(px(24.))
                            .h(px(32.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(!marker.label.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(marker.label),
                                )
                            })
                            .when_some(marker.checked, |el, checked| {
                                let task_id = id.clone();
                                el.child(Checkbox::new(("task", index)).checked(checked).on_change(
                                    cx.listener(move |this, checked, window, cx| {
                                        if let Err(error) =
                                            this.set_checked(&task_id, *checked, window, cx)
                                        {
                                            this.error = Some(error.to_string());
                                        }
                                    }),
                                ))
                            }),
                    )
                })
                .child(body)
                .child(
                    div()
                        .mt_2()
                        .w(px(if previewable { 92. } else { 60. }))
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_1()
                        .flex_shrink_0()
                        .invisible()
                        .group_hover("block-section", |style| style.visible())
                        .when(previewable, |el| {
                            el.child(
                                Button::new(("expand-preview", index))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Maximize)
                                    .disabled(!can_expand)
                                    .accessibility_label("Expand diagram")
                                    .tooltip("Expand diagram")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.expand_diagram(&expand_id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("toggle-preview", index))
                                    .ghost()
                                    .small()
                                    .icon(if show_source {
                                        IconName::Eye
                                    } else {
                                        IconName::SquareTerminal
                                    })
                                    .accessibility_label(if show_source {
                                        "Show diagram"
                                    } else {
                                        "Edit diagram source"
                                    })
                                    .tooltip(if show_source {
                                        "Show diagram"
                                    } else {
                                        "Edit diagram source"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.toggle_preview(&toggle_id, window, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new(("remove", index))
                                .ghost()
                                .small()
                                .icon(self.trash_icon.clone())
                                .accessibility_label("Delete block")
                                .tooltip("Delete block")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Err(error) = this.remove_section(&remove_id, window, cx)
                                    {
                                        this.error = Some(error.to_string());
                                    }
                                })),
                        ),
                );
            #[cfg(test)]
            let section = {
                use gpui_kit::test::TestSupportExt;
                section.test_support()
            };
            content = content.child(section);
            if let Some(ends) = self.list_ends.get(&id) {
                for end in ends {
                    content = content.child(self.drop_zone(
                        end.element.clone(),
                        DropTarget {
                            parent: Some(end.group.clone()),
                            before: None,
                        },
                        end.indent,
                        cx,
                    ));
                    content = content.child(self.drop_zone(
                        format!("list-after-{}", end.group).into(),
                        self.list_boundary(&end.group, true),
                        end.indent.saturating_sub(1),
                        cx,
                    ));
                }
            }
        }
        let append = self.with_drop_target(
                div()
                    .id("append-section")
                    .p_3()
                    .rounded_lg(),
                DropTarget {
                    parent: None,
                    before: None,
                },
                cx,
            )
                .child(
                    Button::new("add-section")
                        .label("+ Add block")
                        .on_click(cx.listener(|this, _, window, cx| {
                            match this.insert_section(None, "", window, cx) {
                                Ok(id) => {
                                    if let Some(view) = this.views.get(&id) {
                                        view.editor.update(cx, |state, cx| state.focus(window, cx));
                                    }
                                }
                                Err(error) => this.error = Some(error.to_string()),
                            }
                        })),
                )
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Enter: new block · Tab / Shift+Tab: nesting · Shift-click grip: range · Cmd/Ctrl-click: toggle · Alt/Option-click: whole list · Drag across lists or to list edges / bottom to escape · Esc: clear selection"),
                );
        #[cfg(test)]
        let append = {
            use gpui_kit::test::TestSupportExt;
            append.test_support()
        };
        content = content.child(append);
        div()
            .id("block-document")
            .track_focus(&self.focus_scope)
            .size_full()
            .flex()
            .flex_col()
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Left {
                    // The grip's bubbling handler marks only actual grip presses.
                    this.grip_pressed = false;
                }
            }))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.grip_pressed = false;
                    this.clear_block_selection(cx);
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.finish_block_drag(cx);
                    if !std::mem::take(&mut this.grip_pressed) {
                        this.clear_block_selection(cx);
                    }
                }
            }))
            .when_some(self.error.clone(), |el, error| {
                el.child(div().p_3().text_color(cx.theme().danger).child(error))
            })
            .when(self.media_imports > 0, |el| {
                el.child(
                    div()
                        .id("media-import-status")
                        .p_3()
                        .text_color(cx.theme().muted_foreground)
                        .child("Importing attachments…"),
                )
            })
            .child(
                div()
                    .id("document-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_5()
                    .can_drop(|value, _, _| value.downcast_ref::<ExternalPaths>().is_some())
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        cx.stop_propagation();
                        this.import_dropped_media(
                            paths.paths().to_vec(),
                            DropTarget {
                                parent: None,
                                before: None,
                            },
                            window,
                            cx,
                        );
                    }))
                    .child(content),
            )
            .when_some(self.selection_toolbar(window, cx), |el, toolbar| {
                el.child(toolbar)
            })
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "collaboration_tests.rs"]
mod collaboration_tests;

#[cfg(test)]
#[path = "nested_editor_tests.rs"]
mod nested_tests;

#[cfg(test)]
#[path = "drag_editor_tests.rs"]
mod drag_tests;

#[cfg(test)]
#[path = "collapsible_editor_tests.rs"]
mod collapsible_tests;

#[cfg(test)]
#[path = "selection_toolbar_tests.rs"]
mod selection_toolbar_tests;

#[cfg(test)]
#[path = "merge_editor_tests.rs"]
mod merge_editor_tests;

#[cfg(test)]
#[path = "table_editor_tests.rs"]
mod table_editor_tests;

#[cfg(test)]
#[path = "media_editor_tests.rs"]
mod media_editor_tests;

#[cfg(test)]
#[path = "block_navigation_tests.rs"]
mod block_navigation_tests;

#[cfg(test)]
#[path = "diagram_editor_tests.rs"]
mod diagram_editor_tests;
