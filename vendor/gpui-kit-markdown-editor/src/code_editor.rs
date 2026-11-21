//! Opt-in code presentation. The block model remains the only document/history owner.
use gpui::{
    App, AppContext, Bounds, Context, Entity, EntityInputHandler, Focusable, Pixels, Subscription,
    Window,
};
use gpui_kit::component::input::{EditorState, InputEditsEvent, InputEvent, Rope, RopeExt as _};
use std::{cell::Cell, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

use crate::kind::BlockKindData;
use crate::{
    BlockEditorEvent, BlockEditorState, BlockId, BlockPosition, BlockSelection, EditOrigin,
};

pub(crate) struct CodeEditor {
    pub(crate) input: Entity<EditorState>,
    revision: u64,
    text: Rope,
    selection: (std::ops::Range<usize>, bool),
    pub(crate) composing: bool,
    pending: bool,
    reload: bool,
    pub(crate) cursor: Rc<Cell<Option<Bounds<Pixels>>>>,
    selection_geometry: Rc<Cell<Option<(usize, usize, Bounds<Pixels>)>>>,
    pending_entry: Rc<Cell<Option<(bool, Pixels)>>>,
    pub(crate) rows: usize,
    next_origin: Option<EditOrigin>,
    _subscriptions: Vec<Subscription>,
}

impl BlockEditorState {
    /// Use GPUI Kit's editable source editor when the entire document is one code block.
    /// This changes presentation only; mixed documents keep the native renderer.
    pub fn with_code_editor(mut self) -> Self {
        self.code_editor_enabled = true;
        self
    }

    /// The rendered code input, when this opt-in presentation is active.
    pub fn code_editor_state(&self) -> Option<Entity<EditorState>> {
        self.single_code_block()?;
        self.code_editor.as_ref().map(|code| code.input.clone())
    }

    pub(crate) fn single_code_block(&self) -> Option<BlockId> {
        if !self.code_editor_enabled {
            return None;
        }
        let children = self.model.doc.children(self.model.doc.root);
        let [block] = children else {
            return None;
        };
        matches!(
            self.model.doc.kind(*block),
            Some(BlockKindData::CodeBlock { .. })
        )
        .then_some(*block)
    }

    pub(crate) fn sync_code_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.single_code_block() else {
            if let Some(old) = self.code_editor.take() {
                if old.input.focus_handle(cx).is_focused(window) {
                    window.focus(&self.focus_handle, cx);
                }
            }
            return;
        };
        // Child input notifications may still be queued when its owner renders.
        // Read its live selection first; an independently moved model selection wins.
        if let Some(input) = self.code_editor.as_ref().map(|code| code.input.clone()) {
            self.code_input_observed(&input, window, cx);
        }
        let revision = self.model.revision();
        if self
            .code_editor
            .as_ref()
            .is_some_and(|code| code.composing || (code.pending && code.revision == revision))
        {
            return;
        }
        let text = self.model.doc.plain(block).unwrap_or_default();
        let lang = match self.model.doc.kind(block) {
            Some(BlockKindData::CodeBlock { lang, .. }) => {
                lang.clone().unwrap_or_else(|| "plain".into())
            }
            _ => unreachable!(),
        };
        // A new entity fences all already-queued events from the previous external
        // document. In particular a late IME/full-buffer event cannot restore peers' text.
        let replace = self.code_editor.as_ref().is_none_or(|code| {
            code.reload || (code.revision != revision && code.text != text)
        });
        if replace {
            let old = self.code_editor.take();
            let focused = old
                .as_ref()
                .is_some_and(|code| code.input.focus_handle(cx).is_focused(window))
                || self.focus_handle.is_focused(window);
            let scroll = old.as_ref().map(|code| code.input.read(cx).scroll_offset());
            let input = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(lang.clone())
                    .line_number(true)
                    .folding(false)
                    .soft_wrap(false)
                    .default_value(text.to_owned())
            });
            let edits = cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEditsEvent, window, cx| {
                    this.code_input_edits(input, event, window, cx);
                },
            );
            let events =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if !this
                        .code_editor
                        .as_ref()
                        .is_some_and(|code| code.input == *input)
                    {
                        return;
                    }
                    match event {
                        InputEvent::Focus => cx.emit(BlockEditorEvent::Focus),
                        InputEvent::Blur => {
                            this.model.break_coalescing();
                            cx.emit(BlockEditorEvent::Blur);
                        }
                        _ => {}
                    }
                });
            let observe = cx.observe_in(&input, window, |this, input, window, cx| {
                this.code_input_observed(&input, window, cx);
            });
            let rope = input.read(cx).text().clone();
            let rows = rope.lines_len().clamp(3, 24);
            self.code_editor = Some(CodeEditor {
                input: input.clone(),
                revision,
                text: rope,
                selection: (0..0, false),
                composing: false,
                pending: false,
                reload: false,
                cursor: Rc::default(),
                rows,
                selection_geometry: Rc::default(),
                pending_entry: Rc::default(),
                next_origin: None,
                _subscriptions: vec![edits, events, observe],
            });
            input.update(cx, |input, cx| {
                input.set_readonly(self.is_readonly(), cx);
                if let Some(scroll) = scroll {
                    input.set_scroll_offset(scroll, cx);
                }
            });
            if focused {
                window.focus(&input.focus_handle(cx), cx);
            }
        }
        let Some(code) = self.code_editor.as_mut() else {
            return;
        };
        code.revision = revision;
        let input = code.input.clone();
        let selection = match *self.model.selection() {
            BlockSelection::Text { anchor, head }
                if anchor.block == block && head.block == block =>
            {
                (
                    anchor.offset.min(head.offset)..anchor.offset.max(head.offset),
                    head.offset < anchor.offset,
                )
            }
            _ => (0..text.len(), false),
        };
        // The model's last acknowledged selection is the mirror baseline, not
        // the input's live selection. A render can run between a user movement
        // and its queued observation; it must not push the old caret back.
        let mirror_selection = !code.pending && code.selection != selection;
        let readonly = self.is_readonly();
        input.update(cx, |input, cx| {
            if input.language_name() != lang {
                input.set_highlighter(lang, cx);
            }
            input.set_readonly(readonly, cx);
            if mirror_selection {
                input.set_selected_range_with_direction(selection.0.clone(), selection.1, cx);
            }
        });
        if let Some(code) = self.code_editor.as_mut() {
            if !code.pending {
                code.selection = selection;
            }
        }
    }

    fn code_input_edits(
        &mut self,
        input: &Entity<EditorState>,
        event: &InputEditsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(block) = self.single_code_block() else {
            return;
        };
        if !self
            .code_editor
            .as_ref()
            .is_some_and(|code| code.input == *input)
        {
            return;
        }
        if self.is_readonly()
            || self.code_editor.as_ref().is_some_and(|code| {
                code.revision != self.model.revision()
                    && code.text.to_string() != self.model.doc.plain(block).unwrap_or_default()
            })
        {
            let code = self.code_editor.as_mut().unwrap();
            code.reload = true;
            code.pending = false;
            self.sync_code_editor(window, cx);
            return;
        }
        let Some((_, after)) = event.edits().last() else {
            return;
        };
        let new_text = after.to_string();
        let old_text = self.model.doc.plain(block).unwrap_or_default();
        // Diff graphemes, not scalar values: an edit can extend a combining cluster
        // or replace part of a ZWJ emoji. Native transaction offsets stay valid.
        let (range, replacement) = grapheme_diff(old_text, &new_text);
        let current = input.read(cx);
        let settled = current.text() == after;
        let (range_selection, reversed) = if settled {
            (current.selected_range(), current.is_selection_reversed())
        } else {
            let end = event
                .edits()
                .last()
                .unwrap()
                .0
                .new_end_byte
                .min(new_text.len());
            (end..end, false)
        };
        let origin = {
            let code = self.code_editor.as_mut().unwrap();
            code.next_origin.take().unwrap_or(if code.composing {
                EditOrigin::Ime
            } else {
                EditOrigin::Typing
            })
        };
        let changed = self.model.transact(origin, |tx| {
            tx.replace_plain(block, range, replacement)?;
            let start = tx
                .doc
                .clamp(BlockPosition::new(block, range_selection.start));
            let end = tx.doc.clamp(BlockPosition::new(block, range_selection.end));
            Ok(Some(if reversed {
                BlockSelection::Text {
                    anchor: end,
                    head: start,
                }
            } else {
                BlockSelection::Text {
                    anchor: start,
                    head: end,
                }
            }))
        });
        let changed = match changed {
            Ok(changed) => changed,
            Err(_) => {
                self.code_editor.as_mut().unwrap().reload = true;
                self.sync_code_editor(window, cx);
                return;
            }
        };
        if let Some(code) = self.code_editor.as_mut() {
            code.text = after.clone();
            code.revision = self.model.revision();
            code.pending = !settled;
            code.composing = false;
            code.selection = (range_selection, reversed);
            code.rows = after.lines_len().clamp(3, 24);
        }
        if changed {
            self.after_edit(origin, cx);
        }
        self.code_input_observed(input, window, cx);
    }

    fn code_input_observed(
        &mut self,
        input: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(block) = self.single_code_block() else {
            return;
        };
        if !self
            .code_editor
            .as_ref()
            .is_some_and(|code| code.input == *input && code.revision == self.model.revision())
        {
            return;
        }
        let composing = input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        });
        let state = input.read(cx);
        let selection = (state.selected_range(), state.is_selection_reversed());
        let focused = state.focus_handle(cx).is_focused(window);
        let code = self.code_editor.as_mut().unwrap();
        let was_composing = std::mem::replace(&mut code.composing, composing);
        code.cursor
            .set(state.cursor_layout().map(|(bounds, _)| bounds));
        if composing || code.pending {
            return;
        }
        // Release the input borrow before notifying its owner.
        let model_selection_matches = matches!(self.model.selection(),
            BlockSelection::Text { anchor, head }
                if anchor.block == block && head.block == block
                    && (anchor.offset.min(head.offset)..anchor.offset.max(head.offset),
                        head.offset < anchor.offset) == code.selection);
        let unchanged = !focused
            || !model_selection_matches
            || code.selection == selection
            || state.text() != &code.text;
        if was_composing {
            cx.notify();
        }
        // Blink, scroll, and theme notifications are not selection events.
        if unchanged {
            return;
        }
        code.selection = selection.clone();
        let start = self
            .model
            .doc
            .clamp(BlockPosition::new(block, selection.0.start));
        let end = self
            .model
            .doc
            .clamp(BlockPosition::new(block, selection.0.end));
        let selection = if selection.1 {
            BlockSelection::Text {
                anchor: end,
                head: start,
            }
        } else {
            BlockSelection::Text {
                anchor: start,
                head: end,
            }
        };
        if self.model.selection() != &selection && self.model.set_selection(selection).is_ok() {
            self.selection_changed(cx);
        }
    }

    pub(crate) fn code_selection_bounds(&self) -> Option<Bounds<Pixels>> {
        let block = self.single_code_block()?;
        let code = self.code_editor.as_ref()?;
        if code.pending || code.revision != self.model.revision() {
            return None;
        }
        let BlockSelection::Text { anchor, head } = *self.model.selection() else {
            return None;
        };
        if anchor.block != block || head.block != block || anchor == head {
            return None;
        }
        let (start, end, bounds) = code.selection_geometry.get()?;
        (start == anchor.offset.min(head.offset) && end == anchor.offset.max(head.offset))
            .then_some(bounds)
    }

    pub(crate) fn code_vertical_boundary_x(&self, down: bool) -> Option<Pixels> {
        let block = self.single_code_block()?;
        let code = self.code_editor.as_ref()?;
        if code.composing || code.pending {
            return None;
        }
        let BlockSelection::Text { anchor, head } = *self.model.selection() else {
            return None;
        };
        if anchor != head {
            return None;
        }
        let text = self.model.doc.plain(block)?;
        let edge = if down {
            !text[head.offset..].contains('\n')
        } else {
            !text[..head.offset].contains('\n')
        };
        edge.then(|| {
            code.cursor
                .get()
                .map(|bounds| bounds.left())
                .unwrap_or(gpui::px(0.))
        })
    }

    pub(crate) fn code_vertical_entry(
        &mut self,
        down: bool,
        x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let block = self.single_code_block()?;
        let code = self.code_editor.as_ref()?;
        if code.composing {
            return Some(false);
        }
        let input = code.input.clone();
        let exact = self.code_entry_offset(down, x, cx);
        let offset = exact.unwrap_or_else(|| {
            if down {
                0
            } else {
                self.model.doc.plain(block).unwrap_or_default().len()
            }
        });
        let changed = self
            .set_selection(BlockSelection::caret(BlockPosition::new(block, offset)), cx)
            .is_ok();
        if changed {
            input.update(cx, |input, cx| input.set_selected_range(offset..offset, cx));
            window.focus(&input.focus_handle(cx), cx);
            if exact.is_none() {
                self.code_editor
                    .as_ref()
                    .unwrap()
                    .pending_entry
                    .set(Some((down, x)));
            }
        }
        Some(changed)
    }

    fn code_entry_offset(&self, down: bool, x: Pixels, cx: &App) -> Option<usize> {
        let block = self.single_code_block()?;
        let code = self.code_editor.as_ref()?;
        let input = code.input.read(cx);
        let text = self.model.doc.plain(block)?;
        let (start, end) = if down {
            (0, text.find('\n').unwrap_or(text.len()))
        } else {
            (text.rfind('\n').map_or(0, |i| i + 1), text.len())
        };
        text[start..end]
            .grapheme_indices(true)
            .map(|(i, _)| start + i)
            .chain(std::iter::once(end))
            .filter_map(|i| {
                input
                    .range_to_bounds(&(i..i))
                    .map(|bounds| (i, (bounds.left() - x).abs()))
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    }
}

fn grapheme_diff<'a>(old: &str, new: &'a str) -> (std::ops::Range<usize>, &'a str) {
    let prefix = old
        .graphemes(true)
        .zip(new.graphemes(true))
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len())
        .sum::<usize>();
    let suffix = old[prefix..]
        .graphemes(true)
        .rev()
        .zip(new[prefix..].graphemes(true).rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len())
        .sum::<usize>();
    (prefix..old.len() - suffix, &new[prefix..new.len() - suffix])
}

pub(crate) fn render(
    state: &Entity<BlockEditorState>,
    style: &gpui::StyleRefinement,
    _window: &mut Window,
    cx: &mut App,
) -> Option<gpui::AnyElement> {
    use gpui::{
        InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _, canvas, div,
        rems,
    };
    use gpui_kit::base::StyledExt as _;
    use gpui_kit::component::input;
    let native = state.read(cx);
    let code = native.code_editor.as_ref()?;
    native.single_code_block()?;
    let input = code.input.clone();
    let rows = code.rows;
    let readonly = native.is_readonly();
    let geometry = code.cursor.clone();
    let selection_geometry = code.selection_geometry.clone();
    let geometry_input = input.clone();
    let pending_entry = code.pending_entry.clone();
    let entry_owner = state.downgrade();
    let owner = state.clone();
    let undo = state.clone();
    let redo = state.clone();
    let paste = state.clone();
    let cut = state.clone();
    let backspace = state.clone();
    let delete = state.clone();
    Some(
        div()
            .id("code-input")
            .relative()
            .w_full()
            .min_w_0()
            .refine_style(style)
            // Hosts can capture one level earlier to use collaborative history.
            .capture_action(move |_: &input::Undo, _, cx| {
                undo.update(cx, |state, cx| {
                    if !state.is_composing() {
                        state.undo(cx);
                    }
                });
                cx.stop_propagation();
            })
            .capture_action(move |_: &input::Redo, _, cx| {
                redo.update(cx, |state, cx| {
                    if !state.is_composing() {
                        state.redo(cx);
                    }
                });
                cx.stop_propagation();
            })
            .capture_action(move |_: &input::Paste, _, cx| {
                paste.update(cx, |state, _| {
                    if let Some(code) = state.code_editor.as_mut() {
                        code.next_origin = Some(EditOrigin::Paste);
                    }
                });
                cx.propagate();
            })
            .capture_action(move |_: &input::Cut, _, cx| {
                cut.update(cx, |state, _| {
                    if let Some(code) = state.code_editor.as_mut() {
                        code.next_origin = Some(EditOrigin::Cut);
                    }
                });
                cx.propagate();
            })
            .capture_action(move |_: &input::Backspace, _, cx| {
                backspace.update(cx, |state, _| {
                    if let Some(code) = state.code_editor.as_mut() {
                        code.next_origin = Some(EditOrigin::Backspace);
                    }
                });
                cx.propagate();
            })
            .capture_action(move |_: &input::Delete, _, cx| {
                delete.update(cx, |state, _| {
                    if let Some(code) = state.code_editor.as_mut() {
                        code.next_origin = Some(EditOrigin::DeleteForward);
                    }
                });
                cx.propagate();
            })
            .capture_key_down(move |event, window, cx| {
                let handled = owner.update(cx, |state, cx| {
                    if let Some(code) = state.code_editor.as_mut() {
                        code.next_origin = None;
                    }
                    if state.is_readonly() || state.is_composing() {
                        return false;
                    }
                    let exit = event.keystroke.key == "enter"
                        && crate::support::secondary_modifier()
                            .is_subset_of(&event.keystroke.modifiers);
                    let empty_backspace = event.keystroke.key == "backspace"
                        && state
                            .single_code_block()
                            .is_some_and(|block| state.model.doc.plain(block) == Some(""));
                    let changed = if exit {
                        state.model.exit_code_block().unwrap_or(false)
                    } else if empty_backspace {
                        let revision = state.model.revision();
                        let _ = state.model.delete(false);
                        state.model.revision() != revision
                    } else {
                        false
                    };
                    if changed {
                        state.after_edit(
                            if exit {
                                EditOrigin::Command("exit_code_block")
                            } else {
                                EditOrigin::Backspace
                            },
                            cx,
                        );
                        state.sync_code_editor(window, cx);
                    }
                    changed
                });
                if handled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .child(
                input::Editor::new(&input)
                    .aria_label("Code")
                    .readonly(readonly)
                    .h(rems(rows as f32 * 1.5 + 1.))
                    .appearance(false)
                    .bordered(false),
            )
            // A passive cache only: painting never mutates an entity or selection.
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        let state = geometry_input.read(cx);
                        geometry.set(state.cursor_layout().map(|(bounds, _)| bounds));
                        let range = state.selected_range();
                        let next_selection =
                            visible_selection_bounds(state, window.content_mask().bounds)
                                .map(|bounds| (range.start, range.end, bounds));
                        if selection_geometry.replace(next_selection) != next_selection {
                            let owner = entry_owner.clone();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |_, cx| cx.notify());
                            });
                        }
                        if let Some((down, x)) = pending_entry.take() {
                            let expected = geometry_input.read(cx).selected_range();
                            let input = geometry_input.clone();
                            let owner = entry_owner.clone();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |state, cx| {
                                    if state.is_composing()
                                        || !state.code_editor.as_ref().is_some_and(|code| {
                                            code.input == input
                                                && code.revision == state.model.revision()
                                        })
                                        || input.read(cx).selected_range() != expected
                                    {
                                        return;
                                    }
                                    let Some(offset) = state.code_entry_offset(down, x, cx) else {
                                        return;
                                    };
                                    let Some(block) = state.single_code_block() else {
                                        return;
                                    };
                                    let _ = state.set_selection(
                                        BlockSelection::caret(BlockPosition::new(block, offset)),
                                        cx,
                                    );
                                    input.update(cx, |input, cx| {
                                        input.set_selected_range(offset..offset, cx)
                                    });
                                });
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element(),
    )
}

fn visible_selection_bounds(input: &EditorState, mask: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    use gpui_kit::component::input::RopeExt as _;
    let range = input.selected_range();
    if range.is_empty() {
        return None;
    }
    let bounds = input.input_bounds();
    let viewport = Bounds::from_corners(
        gpui::point(
            bounds.left() + input.text_origin_offset().unwrap_or(gpui::px(0.)),
            bounds.top(),
        ),
        gpui::point(bounds.right(), bounds.bottom()),
    )
    .intersect(&mask);
    let text = input.text();
    input
        .visible_row_range()?
        .filter_map(|row| {
            let start = range.start.max(text.line_start_offset(row));
            let end = range.end.min(text.line_end_offset(row));
            if start >= end {
                return None;
            }
            let a = input.range_to_bounds(&(start..start))?;
            let b = input.range_to_bounds(&(end..end))?;
            let bounds = Bounds::from_corners(
                gpui::point(a.left().min(b.left()), a.top()),
                gpui::point(a.left().max(b.left()), a.bottom()),
            )
            .intersect(&viewport);
            (bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.))
                .then_some(bounds)
        })
        .reduce(|a, b| a.union(&b))
}
