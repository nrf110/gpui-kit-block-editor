use std::rc::Rc;

use gpui_kit::base::LogicalStyledExt;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, StyledExt, ThemeStyled, scroll::ScrollableElement,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

gpui_kit::actions!(editor_popup, [Cancel, Confirm, SelectDown, SelectUp]);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm, Some("EditorPopupMenu")),
        KeyBinding::new("space", Confirm, Some("EditorPopupMenu")),
        KeyBinding::new("escape", Cancel, Some("EditorPopupMenu")),
        KeyBinding::new("up", SelectUp, Some("EditorPopupMenu")),
        KeyBinding::new("down", SelectDown, Some("EditorPopupMenu")),
    ]);
}

pub(crate) struct EditorPopupItem {
    label: SharedString,
    checked: bool,
    disabled: bool,
    handler: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl EditorPopupItem {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        checked: bool,
        disabled: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            checked,
            disabled,
            handler: Rc::new(handler),
        }
    }
}

/// The editor's flat option menus. Presentation is local to this entity; focus,
/// keyboard highlight and scroll remain intact when its surface is restyled.
pub(crate) struct EditorPopupMenu {
    focus: FocusHandle,
    items: Vec<EditorPopupItem>,
    selected: Option<usize>,
    scroll: ScrollHandle,
    checked_scroll_frames: u8,
    max_height: Pixels,
    style: StyleRefinement,
}

impl EditorPopupMenu {
    pub(crate) fn new(
        items: Vec<EditorPopupItem>,
        max_height: Pixels,
        style: StyleRefinement,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            items,
            selected: None,
            scroll: ScrollHandle::default(),
            checked_scroll_frames: 4,
            max_height,
            style,
        }
    }

    pub(crate) fn set_style(&mut self, style: StyleRefinement, cx: &mut Context<Self>) {
        self.style = style;
        cx.notify();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = Some(index);
        self.scroll.scroll_to_item(index);
        cx.notify();
    }

    fn up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let start = self.selected.unwrap_or(0);
        for step in 1..=self.items.len() {
            let index = (start + self.items.len() - step) % self.items.len();
            if !self.items[index].disabled {
                self.select(index, cx);
                break;
            }
        }
    }

    fn down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        for step in 0..self.items.len() {
            let index = self
                .selected
                .map_or(step, |index| (index + step + 1) % self.items.len());
            if !self.items[index].disabled {
                self.select(index, cx);
                break;
            }
        }
    }

    fn dismiss(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
    fn activate(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self
            .selected
            .and_then(|index| self.items.get(index))
            .filter(|item| !item.disabled)
        else {
            return;
        };
        let handler = item.handler.clone();
        handler(event, window, cx);
        cx.emit(DismissEvent);
    }

    fn scroll_to_checked(&mut self, cx: &mut Context<Self>) {
        if self.checked_scroll_frames == 0 {
            return;
        }
        let Some(index) = self
            .items
            .iter()
            .position(|item| item.checked)
            .filter(|index| *index > 0)
        else {
            self.checked_scroll_frames = 0;
            return;
        };
        match (
            self.scroll.bounds_for_item(0),
            self.scroll.bounds_for_item(index),
        ) {
            (Some(first), Some(checked)) => {
                self.checked_scroll_frames = 0;
                let mut offset = self.scroll.offset();
                offset.y = first.top() - checked.top();
                self.scroll.set_offset(offset);
            }
            _ => {
                self.checked_scroll_frames -= 1;
                cx.notify();
            }
        }
    }
}

impl Focusable for EditorPopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EventEmitter<DismissEvent> for EditorPopupMenu {}

impl Render for EditorPopupMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scroll_to_checked(cx);
        let has_check = self.items.iter().any(|item| item.checked);
        let radius = cx.theme().radius.min(px(8.));
        let rows = self.items.iter().enumerate().map(|(index, item)| {
            let row = div()
                .id(index)
                .role(Role::MenuItem)
                .aria_label(item.label.clone())
                .aria_selected(self.selected == Some(index))
                .aria_toggled(if item.checked {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                })
                .flex()
                .flex_row_dir(cx)
                .items_center()
                .gap_1()
                .h(rems(1.625))
                .px_2()
                .text_sm()
                .rounded(radius)
                .text_color(cx.theme().foreground)
                .map(|mut row| {
                    row.style().text.refine(&self.style.text);
                    row
                })
                .when(!item.disabled, |row| {
                    row.hover(|style| {
                        style
                            .bg(cx.theme().tokens.accent)
                            .text_color(cx.theme().accent_foreground)
                    })
                    .when(self.selected == Some(index), |row| {
                        row.bg(cx.theme().tokens.accent)
                            .text_color(cx.theme().accent_foreground)
                    })
                    .on_hover(cx.listener(move |this, hovered, window, cx| {
                        if *hovered {
                            this.selected = Some(index);
                        } else if this.selected == Some(index) && !window.last_input_was_keyboard()
                        {
                            this.selected = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, event, window, cx| {
                        cx.stop_propagation();
                        window.prevent_default();
                        this.selected = Some(index);
                        this.activate(event, window, cx);
                    }))
                })
                .when(item.disabled, |row| {
                    row.text_color(cx.theme().muted_foreground)
                })
                .when(has_check, |row| {
                    row.child(
                        if item.checked {
                            Icon::new(IconName::Check)
                        } else {
                            Icon::empty()
                        }
                        .xsmall(),
                    )
                })
                .child(item.label.clone());
            #[cfg(test)]
            let row = {
                use gpui_kit::test::TestSupportExt;
                row.test_support()
            };
            row
        });
        let menu =
            div()
                .id("popup-menu")
                .role(Role::Menu)
                .key_context("EditorPopupMenu")
                .track_focus(&self.focus)
                .on_action(cx.listener(Self::up))
                .on_action(cx.listener(Self::down))
                .on_action(cx.listener(Self::dismiss))
                .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                    this.activate(&ClickEvent::default(), window, cx)
                }))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.dismiss(&Cancel, window, cx)
                }))
                .popover_style(cx)
                .text_color(cx.theme().popover_foreground)
                .relative()
                .occlude()
                .flex()
                .flex_col()
                .min_w(px(180.))
                .max_w(rems(31.25))
                .max_h(self.max_height)
                .refine_style(&self.style)
                .child(
                    div()
                        .id("items")
                        .flex()
                        .flex_col()
                        .min_h_0()
                        .p_1()
                        .gap_y_0p5()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children(rows),
                )
                .vertical_scrollbar(&self.scroll);
        #[cfg(test)]
        let menu = {
            use gpui_kit::test::TestSupportExt;
            menu.test_support()
        };
        menu
    }
}
