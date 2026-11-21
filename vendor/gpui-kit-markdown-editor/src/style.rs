use std::sync::Arc;

use gpui::{Hsla, Pixels, Refineable, StyleRefinement};
use gpui_kit::base::text::TextViewStyle;

use gpui_kit::base::SemanticThemeTokens;

/// Per-view presentation, never document formatting. Unset properties inherit
/// the current theme and the view's ordinary `Styled` typography.
///
/// The body refines inherited and view-local `Styled` values. Block-specific
/// refinements follow the active app defaults. Table cells refine the table
/// and header row, matching GPUI's native table renderer.
///
/// Code uses the framework's real input: its font, size and line height shape
/// glyphs and caret geometry. The input's IME underline follows its foreground;
/// syntax-token colors remain owned by the active framework highlighter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorTextStyle {
    pub body: StyleRefinement,
    pub headings: [StyleRefinement; 6],
    pub code: StyleRefinement,
    pub quote: StyleRefinement,
    pub table: StyleRefinement,
    pub table_head: StyleRefinement,
    pub table_cell: StyleRefinement,
    pub caret: Option<Hsla>,
    pub selection: Option<Hsla>,
}

/// Cache the merged heading closure, not the theme: each render compares the
/// freshly resolved app defaults before reusing it.
#[derive(Default)]
pub(crate) struct PresentationCache {
    source: Option<Arc<EditorTextStyle>>,
    base: Option<TextViewStyle>,
    resolved: Option<TextViewStyle>,
}

impl PresentationCache {
    pub(crate) fn resolve(
        &mut self,
        source: Option<&Arc<EditorTextStyle>>,
        base: &TextViewStyle,
    ) -> (TextViewStyle, bool) {
        let same_source = match (&self.source, source) {
            (None, None) => true,
            (Some(old), Some(new)) => Arc::ptr_eq(old, new) || old == new,
            _ => false,
        };
        if same_source && self.base.as_ref() == Some(base) {
            if let Some(resolved) = &self.resolved {
                return (resolved.clone(), false);
            }
        }
        let mut resolved = base.clone();
        if let Some(source) = source {
            if source
                .headings
                .iter()
                .any(|style| *style != StyleRefinement::default())
            {
                let headings: [StyleRefinement; 6] = std::array::from_fn(|ix| {
                    let mut heading = base.heading(ix as u8 + 1);
                    heading.refine(&source.headings[ix]);
                    heading
                });
                resolved = resolved.with_heading(move |level| {
                    headings[usize::from(level.saturating_sub(1)).min(5)].clone()
                });
            }
            let merge = |base: &StyleRefinement, refinement: &StyleRefinement| {
                let mut result = base.clone();
                result.refine(refinement);
                result
            };
            resolved = resolved
                .with_code_block(merge(base.code_block(), &source.code))
                .with_blockquote_style(merge(base.blockquote_style(), &source.quote))
                .with_table(merge(base.table(), &source.table))
                .with_table_head(merge(base.table_head(), &source.table_head))
                .with_table_cell(merge(base.table_cell(), &source.table_cell));
            if let Some(color) = source.body.text.color {
                resolved = resolved.with_foreground(color);
            }
            if let Some(color) = source.selection {
                resolved = resolved.with_selection(color);
            }
        }
        let changed = self.resolved.is_some();
        self.source = source.cloned();
        self.base = Some(base.clone());
        self.resolved = Some(resolved.clone());
        (resolved, changed)
    }
}

/// Presentation seam for [`super::BlockEditor`]. Private fields, `with_*`
/// setters and readers (ARCHITECTURE.md invariant 12). Unset colours are
/// transparent and are filled from the theme tokens by [`Self::resolved`],
/// mirroring `InputEditorStyle::resolved`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEditorStyle {
    foreground: Hsla,
    muted_foreground: Hsla,
    caret: Hsla,
    caret_width: Pixels,
    selection: Hsla,
    composition_underline: Hsla,
}

impl Default for BlockEditorStyle {
    fn default() -> Self {
        let unset = Hsla::transparent_black();
        Self {
            foreground: unset,
            muted_foreground: unset,
            caret: unset,
            caret_width: crate::support::CURSOR_WIDTH,
            selection: unset,
            composition_underline: unset,
        }
    }
}

macro_rules! style_field {
    ($with:ident, $get:ident, $ty:ty) => {
        pub fn $with(mut self, value: $ty) -> Self {
            self.$get = value;
            self
        }
        pub fn $get(&self) -> $ty {
            self.$get
        }
    };
}

impl BlockEditorStyle {
    style_field!(with_foreground, foreground, Hsla);
    style_field!(with_muted_foreground, muted_foreground, Hsla);
    style_field!(with_caret, caret, Hsla);
    style_field!(with_caret_width, caret_width, Pixels);
    style_field!(with_selection, selection, Hsla);
    style_field!(with_composition_underline, composition_underline, Hsla);

    /// Whether no selection colour was set, so the view uses the text
    /// renderer's own (`TextViewStyle::selection`).
    pub(crate) fn selection_is_default(&self) -> bool {
        self.selection.a == 0.
    }

    /// Fill unset colours from the active palette.
    pub fn resolved(&self, tokens: &SemanticThemeTokens) -> Self {
        let colors = &tokens.colors;
        let or = |value: Hsla, fallback: Hsla| if value.a == 0. { fallback } else { value };
        let foreground = or(self.foreground, colors.foreground);
        Self {
            foreground,
            muted_foreground: or(self.muted_foreground, colors.muted_foreground),
            caret: or(self.caret, foreground),
            composition_underline: or(self.composition_underline, foreground),
            ..self.clone()
        }
    }
}
