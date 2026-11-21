use std::sync::Arc;

use gpui_kit::base::{Button as BaseButton, GlobalState, LogicalStyledExt};
use gpui_kit::component::{
    ActiveTheme, Icon, Selectable, Sizable, Size as ControlSize, StyledExt, ThemeStyled,
    select::Caret, tooltip::Tooltip,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::EditorStyle;

/// Editor chrome over GPUI's button behavior. Resolve local presentation once,
/// after theme defaults, so there is exactly one hover rule on the real button.
#[derive(IntoElement)]
pub(crate) struct EditorButton {
    id: ElementId,
    base: BaseButton,
    style: StyleRefinement,
    appearance: Option<Arc<EditorStyle>>,
    ghost: bool,
    small: bool,
    label: Option<SharedString>,
    icon: Option<Icon>,
    accessibility_label: Option<SharedString>,
    tooltip: Option<SharedString>,
    disabled: bool,
    selected: bool,
    checked: Option<bool>,
    dropdown_caret: bool,
}

impl EditorButton {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            base: BaseButton::new(id.clone()),
            id,
            style: StyleRefinement::default(),
            appearance: None,
            ghost: false,
            small: false,
            label: None,
            icon: None,
            accessibility_label: None,
            tooltip: None,
            disabled: false,
            selected: false,
            checked: None,
            dropdown_caret: false,
        }
    }

    pub(crate) fn appearance(mut self, style: Arc<EditorStyle>) -> Self {
        self.appearance = Some(style);
        self
    }

    pub(crate) fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }

    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub(crate) fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub(crate) fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub(crate) fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    pub(crate) fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(crate) fn dropdown_caret(mut self, caret: bool) -> Self {
        self.dropdown_caret = caret;
        self
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.base = self.base.on_click(handler);
        self
    }
}

impl Styled for EditorButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Selectable for EditorButton {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for EditorButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id, cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);
        let theme = cx.theme();
        let toggle = self.checked.is_some();
        let selected = self.selected || self.checked == Some(true);
        let normal_bg: Background = if self.ghost {
            theme.transparent.into()
        } else {
            theme.tokens.button.into()
        };
        let normal_fg = if self.ghost {
            theme.secondary_foreground
        } else {
            theme.button_foreground
        };
        let border = if self.ghost {
            theme.transparent
        } else {
            theme.input
        };
        let hover_bg: Background = if self.ghost {
            let accent: Background = theme.tokens.accent.into();
            if theme.mode.is_dark() && !toggle {
                accent.opacity(0.5)
            } else {
                accent
            }
        } else {
            theme.tokens.button_hover.into()
        };
        let hover_fg = if self.ghost {
            theme.accent_foreground
        } else {
            normal_fg
        };
        let selected_bg: Background = if toggle {
            theme.tokens.accent.into()
        } else if self.ghost {
            theme.tokens.secondary_active.into()
        } else {
            theme.tokens.button_active.into()
        };
        let selected_fg = if toggle {
            theme.accent_foreground
        } else {
            normal_fg
        };
        let disabled_bg: Background = if self.ghost {
            theme.transparent.into()
        } else {
            theme.input_background().opacity(0.5).into()
        };
        let disabled_fg = theme.muted_foreground.opacity(0.5);
        let disabled_border = border.opacity(0.5);
        let active_bg: Background = theme.tokens.button_active.into();
        let size = if self.small {
            ControlSize::Small
        } else {
            ControlSize::Medium
        };
        let mut overrides = self.style;
        if let Some(appearance) = &self.appearance {
            overrides.refine(&appearance.controls);
        }
        let mut hover = StyleRefinement::default();
        if !self.disabled {
            if !selected {
                hover = hover.bg(hover_bg).text_color(hover_fg).border_color(border);
            }
            hover.refine(&overrides);
            if let Some(appearance) = &self.appearance {
                hover.refine(&appearance.controls_hover);
            }
        }
        let content = div()
            .id("label")
            .flex()
            .flex_row_dir(cx)
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .items_center()
            .justify_center()
            .gap(if self.small { px(4.) } else { px(8.) })
            .when_some(self.icon, |content, icon| {
                content.child(icon.with_size(size))
            })
            .when_some(self.label.clone(), |content, label| {
                content.child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
            })
            .when(self.dropdown_caret, |content| {
                content
                    .justify_between()
                    .child(Caret::new(size).text_color(normal_fg.opacity(0.75)))
            });
        self.base
            .cursor_default()
            .flex_shrink_0()
            .rounded(theme.radius)
            .line_height(relative(1.25))
            .text_sm()
            .map(|button| {
                let height = if self.small { px(24.) } else { px(32.) };
                if toggle {
                    button.h(height).min_w(height).px_1()
                } else if self.label.is_none() {
                    button.size(height)
                } else {
                    button
                        .h(height)
                        .px(if self.small { px(8.) } else { px(10.) })
                }
            })
            .when(!self.ghost, |button| button.border_1())
            .bg(normal_bg)
            .text_color(normal_fg)
            .border_color(border)
            .selected(selected)
            .disabled(self.disabled)
            .styles(|styles| {
                styles
                    .selected(|style| {
                        style
                            .bg(selected_bg)
                            .text_color(selected_fg)
                            .border_color(border)
                            .refine_style(&overrides)
                    })
                    .disabled(|style| {
                        style
                            .bg(disabled_bg)
                            .text_color(disabled_fg)
                            .border_color(disabled_border)
                            .refine_style(&overrides)
                    })
            })
            .hover(|_| hover)
            .active(|style| {
                if selected {
                    style
                } else {
                    style
                        .bg(active_bg)
                        .text_color(normal_fg)
                        .refine_style(&overrides)
                }
            })
            .refine_style(&overrides)
            .when_some(self.accessibility_label.or(self.label), |button, label| {
                button.accessibility_label(label)
            })
            .when_some(self.checked, |button, checked| {
                button.aria_toggled(if checked {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                })
            })
            .when_some(self.tooltip, |button, tooltip| {
                button
                    .aria_description(tooltip.clone())
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
            .track_focus(&focus)
            .on_any_mouse_down(|event, window, cx| {
                window.prevent_default();
                if event.button == MouseButton::Left {
                    GlobalState::suppress_text_selection(cx);
                }
            })
            .when(focused, |button| button.focus_ring_style(window, cx))
            .child(content)
    }
}
