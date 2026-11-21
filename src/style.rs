use std::sync::Arc;

use crate::native_editor::EditorTextStyle;
use gpui_kit::{Refineable as _, StyleRefinement};

/// Local presentation overrides. Unset properties retain the current GPUI theme
/// and the editor's standard layout. None of these values are document data.
///
/// Style refinements are applied after defaults, just like GPUI's `Styled`
/// methods. For example, `content.max_w` controls the readable document width,
/// while `document.padding` controls the scroll viewport's internal padding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorStyle {
    /// Scroll viewport: padding, background, and inherited body typography.
    pub document: StyleRefinement,
    /// Centered block column: width, maximum width, and spacing between blocks.
    pub content: StyleRefinement,
    /// Each outer block's normal surface and spacing.
    pub block: StyleRefinement,
    /// Refinements applied after the selected-block background.
    pub selected_block: StyleRefinement,
    /// Native rich text and source-code presentation, including headings,
    /// code typography, tables, quotes, caret, and text-selection colors.
    pub text: Arc<EditorTextStyle>,
    /// Buttons throughout editor chrome and its overlays.
    pub controls: StyleRefinement,
    pub controls_hover: StyleRefinement,
    pub drag_handle: StyleRefinement,
    pub drag_handle_hover: StyleRefinement,
    /// Slash/+ command menu surface, group headings, and command rows.
    pub slash_menu: StyleRefinement,
    pub slash_menu_heading: StyleRefinement,
    pub slash_menu_item: StyleRefinement,
    pub slash_menu_item_hover: StyleRefinement,
    /// Style/list/language popup surfaces. Checked/disabled/hover states keep
    /// GPUI theme colors; keyboard, focus, and scrolling are retained on updates.
    pub popup_menu: StyleRefinement,
    pub selection_toolbar: StyleRefinement,
    pub link_editor: StyleRefinement,
    /// Attachment card/image surface.
    pub media: StyleRefinement,
    /// Inline diagram preview surface.
    pub diagram: StyleRefinement,
    /// Expanded diagram dialog surface and preferred dimensions.
    pub diagram_viewer: StyleRefinement,
    pub diagram_toolbar: StyleRefinement,
    pub diagram_canvas: StyleRefinement,
}

impl EditorStyle {
    pub(crate) fn quoted_text(&self) -> Arc<EditorTextStyle> {
        if self.text.quote.text == Default::default() {
            return self.text.clone();
        }
        let mut text = self.text.as_ref().clone();
        text.body.text.refine(&self.text.quote.text);
        Arc::new(text)
    }

    /// Set rich-text/code presentation without requiring an explicit Arc.
    pub fn with_text(mut self, text: EditorTextStyle) -> Self {
        self.text = Arc::new(text);
        self
    }
}

pub(crate) fn diagram_dialog_size(
    style: Option<&StyleRefinement>,
    viewport: gpui_kit::Size<gpui_kit::Pixels>,
    rem: gpui_kit::Pixels,
) -> gpui_kit::Size<gpui_kit::Pixels> {
    use gpui_kit::{Length, Pixels, px, size};
    let resolve = |length: Option<Length>, available: Pixels| match length {
        Some(Length::Definite(value)) => Some(value.to_pixels(available.into(), rem)),
        _ => None,
    };
    let dimension = |preferred, minimum, maximum, available: Pixels| {
        let mut value = resolve(preferred, available).unwrap_or(available - px(48.));
        if let Some(minimum) = resolve(minimum, available) {
            value = value.max(minimum);
        }
        if let Some(maximum) = resolve(maximum, available) {
            value = value.min(maximum);
        }
        value.max(px(1.))
    };
    size(
        dimension(
            style.and_then(|s| s.size.width),
            style.and_then(|s| s.min_size.width),
            style.and_then(|s| s.max_size.width),
            viewport.width,
        ),
        dimension(
            style.and_then(|s| s.size.height),
            style.and_then(|s| s.min_size.height),
            style.and_then(|s| s.max_size.height),
            viewport.height,
        ),
    )
}
