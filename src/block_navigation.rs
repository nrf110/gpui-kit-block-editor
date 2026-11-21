use super::*;

impl BlockEditor {
    pub(super) fn handle_vertical_block_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.modifiers.number_of_modifiers() != 0
            || !matches!(key.key.as_str(), "up" | "down")
            || self.completion.is_some()
            || self.selection_dropdown.is_some()
            || self.selection_link.is_some()
            || self.is_composing(cx)
        {
            return false;
        }
        let down = key.key == "down";
        let focused = self.order.iter().position(|id| {
            self.views[id]
                .editor
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        });
        // Attachment cards have no rendered native input. Their visible outer
        // selection, with focus on the document scope, is the navigation caret.
        let focused = focused.or_else(|| {
            if !self.focus_scope.is_focused(window) || self.block_selection.len() != 1 {
                return None;
            }
            self.order
                .iter()
                .position(|id| self.block_selection.contains(id) && self.views[id].media.is_some())
        });
        let Some(index) = focused else {
            return false;
        };
        let source = &self.order[index];
        let state = self.views[source].editor.read(cx);
        if state.table_menu_open() {
            return false;
        }
        let Some(x) = state.vertical_boundary_x(down) else {
            return false;
        };
        let target = if down {
            self.order.get(index + 1)
        } else {
            index.checked_sub(1).and_then(|index| self.order.get(index))
        };
        let Some(target) = target.cloned() else {
            // Keep native behavior at the document's own edges.
            return false;
        };
        let source = source.clone();
        let result = (|| -> anyhow::Result<bool> {
            self.cache_selection(&source, cx)?;
            let view = self
                .views
                .get_mut(&target)
                .expect("visible section has a view");
            // Match focus_block_at: rendered diagram cards become editable
            // source rather than focusing an input that is not in the tree.
            let show_source = view.show_source;
            if view.previewable {
                view.show_source = true;
            }
            if !view.editor.update(cx, |state, cx| {
                state.focus_vertical_entry(down, x, window, cx)
            }) {
                view.show_source = show_source;
                return Ok(false);
            }
            let media = view.media.is_some();
            self.active_block_drag = None;
            self.block_selection.clear();
            self.block_anchor = None;
            if media {
                self.block_selection.insert(target.clone());
                self.block_anchor = Some(target.clone());
                self.focus_scope.focus(window, cx);
                self.views[&target].navigation_anchor.scroll_to(window, cx);
            }
            self.cache_selection(&target, cx)?;
            Ok(true)
        })();
        match result {
            Ok(false) => return false,
            Err(error) => self.error = Some(error.to_string()),
            Ok(true) => {}
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
        true
    }
}
