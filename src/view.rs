use gpui_kit::component::StyledExt as _;
use gpui_kit::{
    App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    StyleRefinement, Styled, Window, div,
};

use crate::BlockEditor;

/// A normal GPUI `Styled` layout wrapper around a retained editor.
///
/// Use `BlockEditor::with_style` / `set_style` for internal surfaces. View
/// refinements affect this mount only and are never copied into editor state.
#[derive(IntoElement)]
pub struct BlockEditorView {
    editor: Entity<BlockEditor>,
    style: StyleRefinement,
}

impl BlockEditorView {
    pub fn new(editor: &Entity<BlockEditor>) -> Self {
        Self {
            editor: editor.clone(),
            style: StyleRefinement::default(),
        }
    }
}

impl Styled for BlockEditorView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for BlockEditorView {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let view = div()
            .id(("block-editor-view", self.editor.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .refine_style(&self.style)
            .child(self.editor);
        #[cfg(test)]
        let view = {
            use gpui_kit::test::TestSupportExt;
            view.test_support()
        };
        view
    }
}
