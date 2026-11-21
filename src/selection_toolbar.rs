use super::*;
use gpui_kit::component::toolbar::Toolbar;
use gpui_kit::component::{
    Disableable,
    button::{Toggle, ToggleVariants},
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
};
use markdown_editor::{MarkChange, MarkKind, MarkState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionPanel {
    Styles,
    Lists,
}

pub(super) struct SelectionDropdown {
    section: SectionId,
    kind: SelectionPanel,
    menu: Entity<PopupMenu>,
    _subscription: Subscription,
}

impl SelectionPanel {
    fn choices(self) -> &'static [(&'static str, CommandAction)] {
        const STYLES: &[(&str, CommandAction)] = &[
            (
                "Paragraph",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Paragraph)),
            ),
            (
                "Heading 1",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 1,
                })),
            ),
            (
                "Heading 2",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 2,
                })),
            ),
            (
                "Heading 3",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 3,
                })),
            ),
            (
                "Heading 4",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 4,
                })),
            ),
            (
                "Heading 5",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 5,
                })),
            ),
            (
                "Heading 6",
                CommandAction::Editor(EditorCommand::SetBlockKind(NativeBlockKind::Heading {
                    level: 6,
                })),
            ),
        ];
        const LISTS: &[(&str, CommandAction)] = &[
            (
                "Bulleted list",
                CommandAction::Editor(EditorCommand::ToggleList { ordered: false }),
            ),
            (
                "Numbered list",
                CommandAction::Editor(EditorCommand::ToggleList { ordered: true }),
            ),
            (
                "Task list",
                CommandAction::Editor(EditorCommand::ToggleTask),
            ),
            ("Collapsible list", CommandAction::CollapsibleList),
        ];
        match self {
            Self::Styles => STYLES,
            Self::Lists => LISTS,
        }
    }
}

pub(super) struct SelectionLink {
    section: SectionId,
    selection: RelativeSelection,
    input: Entity<InputState>,
    error: Option<&'static str>,
    _subscription: Subscription,
}

fn text_selected(state: &BlockEditorState) -> bool {
    matches!(state.selection(), BlockSelection::Text { anchor, head } if anchor != head)
        && !state.is_readonly()
        && !state.is_composing()
}

fn link_url<'a>(nodes: &'a [RichNode], node_id: &str, offset: usize) -> Option<&'a str> {
    for node in nodes {
        if node.id == node_id {
            let mut start = 0;
            for run in &node.runs {
                let end = start + run.text.len();
                if offset < end {
                    return run.marks.link.as_ref().map(|link| link.url.as_str());
                }
                start = end;
            }
            return None;
        }
        if let Some(url) = link_url(&node.children, node_id, offset) {
            return Some(url);
        }
    }
    None
}

fn safe_link(url: &str) -> bool {
    if url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return false;
    }
    let prefix = url.split(['/', '?', '#']).next().unwrap_or_default();
    match prefix.split_once(':') {
        Some((scheme, _)) => ["http", "https", "mailto"]
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
        None => true,
    }
}

impl BlockEditor {
    pub(super) fn selection_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.selection_link
            .as_ref()
            .is_some_and(|link| link.input.focus_handle(cx).is_focused(window))
    }

    pub(super) fn handle_selection_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.key != "escape" || key.modifiers.number_of_modifiers() != 0 {
            return false;
        }
        if self.selection_input_focused(window, cx) {
            let input = self.selection_link.as_ref().unwrap().input.clone();
            if input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }) {
                return false;
            }
            self.cancel_selection_link(window, cx);
        } else {
            return false;
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
        true
    }

    fn run_selection_action(
        &mut self,
        id: &str,
        action: CommandAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .views
            .get(id)
            .is_some_and(|view| text_selected(view.editor.read(cx)))
        {
            return;
        }
        self.selection_dropdown = None;
        self.selection_link = None;
        self.completion = None;
        self.execute_command(id, action, None, window, cx);
    }

    fn set_selection_dropdown(
        &mut self,
        id: &str,
        kind: SelectionPanel,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !open {
            if self
                .selection_dropdown
                .as_ref()
                .is_some_and(|dropdown| dropdown.section == id && dropdown.kind == kind)
            {
                self.selection_dropdown = None;
                cx.notify();
            }
            return;
        }
        let Some(view) = self.views.get(id) else {
            return;
        };
        let block_kind = view.editor.read(cx).block_kind_at_caret();
        let task = matches!(view.kind, SectionKind::ListItem { checked: Some(_) });
        let list = view
            .parent
            .as_ref()
            .and_then(|parent| self.tree.get(parent));
        let checked = kind.choices().iter().position(|(_, command)| match command {
            CommandAction::Editor(EditorCommand::SetBlockKind(target)) => block_kind == Some(*target),
            CommandAction::Editor(EditorCommand::ToggleList { ordered }) => !task && matches!(view.kind, SectionKind::ListItem { .. }) && list.is_some_and(|node| matches!(node.kind, SectionKind::List { ordered: current, .. } if current == *ordered)),
            CommandAction::Editor(EditorCommand::ToggleTask) => task,
            CommandAction::CollapsibleList => matches!(view.kind, SectionKind::CollapsibleListItem { .. }),
            _ => false,
        });
        let owner = cx.weak_entity();
        let section: Arc<str> = Arc::from(id);
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.min_w(px(180.)).max_h(px(160.)).scrollable(true);
            for (index, (label, command)) in kind.choices().iter().enumerate() {
                let owner = owner.clone();
                let section = section.clone();
                let command = command.clone();
                menu = menu.item(
                    PopupMenuItem::new(*label)
                        .checked(checked == Some(index))
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.run_selection_action(&section, command.clone(), window, cx);
                            });
                        }),
                );
            }
            menu
        });
        let subscription =
            cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
                if this
                    .selection_dropdown
                    .as_ref()
                    .is_some_and(|dropdown| dropdown.menu == *menu)
                {
                    let restore_focus = menu.focus_handle(cx).contains_focused(window, cx);
                    let dropdown = this.selection_dropdown.take().unwrap();
                    if restore_focus {
                        if let Some(view) = this.views.get(&dropdown.section) {
                            view.editor.update(cx, |state, cx| state.focus(window, cx));
                        }
                    }
                    cx.notify();
                }
            });
        self.selection_dropdown = Some(SelectionDropdown {
            section: id.to_owned(),
            kind,
            menu: menu.clone(),
            _subscription: subscription,
        });
        menu.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn selection_dropdown_control(
        &self,
        trigger: Button,
        section: Arc<str>,
        kind: SelectionPanel,
        cx: &Context<Self>,
    ) -> Popover {
        let open = self
            .selection_dropdown
            .as_ref()
            .is_some_and(|dropdown| dropdown.section == section.as_ref() && dropdown.kind == kind);
        let target = section.clone();
        let owner = cx.weak_entity();
        Popover::new(match kind {
            SelectionPanel::Styles => "selection-style-popover",
            SelectionPanel::Lists => "selection-list-popover",
        })
        .appearance(false)
        .overlay_closable(false)
        .anchor(Anchor::TopLeft)
        .trigger(trigger)
        .open(open)
        .on_open_change(cx.listener(move |this, open, window, cx| {
            this.set_selection_dropdown(&target, kind, *open, window, cx);
        }))
        .content(move |_, window, cx| {
            let menu = owner.upgrade().and_then(|owner| {
                owner
                    .read(cx)
                    .selection_dropdown
                    .as_ref()
                    .filter(|dropdown| {
                        dropdown.section == section.as_ref() && dropdown.kind == kind
                    })
                    .map(|dropdown| dropdown.menu.clone())
            });
            // The popover initially focuses its own scope; the menu owns
            // arrow navigation, confirmation, and Escape dismissal.
            if let Some(menu) = &menu {
                let focus = menu.focus_handle(cx);
                if !focus.contains_focused(window, cx) {
                    focus.focus(window, cx);
                }
            }
            let surface = div()
                .id("selection-dropdown-menu")
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .when_some(menu, |el, menu| el.child(menu));
            #[cfg(test)]
            let surface = {
                use gpui_kit::test::TestSupportExt;
                surface.test_support()
            };
            surface
        })
    }

    fn open_selection_link(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.views.get(id) else {
            return;
        };
        let state = view.editor.read(cx);
        if !text_selected(state) {
            return;
        }
        let Some(map) = view.rich_map.as_ref() else {
            return;
        };
        let selection = match RelativeSelection::capture(
            &self.document,
            id,
            &view.content,
            map,
            state.selection(),
        ) {
            Ok(selection) => selection,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let offsets = map.selection_offsets(state.selection());
        let url: SharedString = offsets
            .as_ref()
            .and_then(|offsets| {
                let anchor = &offsets.anchor;
                let reversed =
                    anchor.node == offsets.head.node && anchor.offset > offsets.head.offset;
                link_url(
                    &view.content.blocks,
                    &anchor.node,
                    anchor.offset.saturating_sub(usize::from(reversed)),
                )
            })
            .unwrap_or_default()
            .to_owned()
            .into();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://example.com")
                .default_value(url)
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.apply_selection_link(false, window, cx),
                InputEvent::Change => {
                    if let Some(link) = &mut this.selection_link {
                        link.error = None;
                    }
                    cx.notify();
                }
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
            },
        );
        self.selection_dropdown = None;
        self.selection_link = Some(SelectionLink {
            section: id.to_owned(),
            selection,
            input: input.clone(),
            error: None,
            _subscription: subscription,
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn cancel_selection_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(link) = self.selection_link.take() {
            if let Some(view) = self.views.get(&link.section) {
                view.editor.update(cx, |state, cx| state.focus(window, cx));
            } else {
                self.focus_scope.focus(window, cx);
            }
        }
        cx.notify();
    }

    fn apply_selection_link(&mut self, remove: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.selection_link.as_ref() else {
            return;
        };
        let value = link.input.read(cx).value();
        let url = value.trim();
        if !remove && !safe_link(url) {
            self.selection_link.as_mut().unwrap().error =
                Some("Use an http, https, mailto, or relative URL without spaces.");
            cx.notify();
            return;
        }
        let selection = self.views.get(&link.section).and_then(|view| {
            link.selection.resolve(&self.document, view.rich_map.as_ref()?)
        }).filter(|selection| matches!(selection, BlockSelection::Text { anchor, head } if anchor != head));
        let Some(selection) = selection else {
            self.selection_link.as_mut().unwrap().error =
                Some("The selected text is no longer available.");
            cx.notify();
            return;
        };
        let mark = if remove || url.is_empty() {
            MarkChange::new().clear(MarkKind::Link)
        } else {
            MarkChange::new().with_link(url.to_owned(), None)
        };
        let id = link.section.clone();
        let editor = self.views[&id].editor.clone();
        if let Err(error) = editor.update(cx, |state, cx| state.set_selection(selection, cx)) {
            self.error = Some(format!("{error:?}"));
            cx.notify();
            return;
        }
        self.run_selection_action(
            &id,
            CommandAction::Editor(EditorCommand::ApplyMark(mark)),
            window,
            cx,
        );
    }

    pub(super) fn selection_toolbar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.completion.is_some() || !self.block_selection.is_empty() {
            return None;
        }
        let toolbar_focused = self.selection_toolbar_focus.contains_focused(window, cx);
        let id = self.order.iter().find(|id| {
            let view = &self.views[*id];
            let state = view.editor.read(cx);
            (!view.previewable || view.show_source)
                && text_selected(state)
                && (state.focus_handle(cx).is_focused(window)
                    || self.selection_link.as_ref().is_some_and(|link| {
                        link.section == **id && link.input.focus_handle(cx).is_focused(window)
                    })
                    || (toolbar_focused && self.selection_toolbar_target.as_ref() == Some(*id))
                    || self.selection_dropdown.as_ref().is_some_and(|dropdown| {
                        dropdown.section == **id
                            && dropdown.menu.focus_handle(cx).contains_focused(window, cx)
                    }))
        })?;
        if self.selection_toolbar_target.as_deref() != Some(id.as_str()) {
            self.selection_toolbar_target = Some(id.clone());
        }
        let view = &self.views[id];
        let editor = view.editor.clone();
        let state = editor.read(cx);
        let model = state.model();
        let kind = state.block_kind_at_caret();
        let marks_disabled = kind == Some(NativeBlockKind::CodeBlock);
        let styles_disabled = matches!(kind, Some(NativeBlockKind::TableCell) | None);
        let quote_disabled = kind == Some(NativeBlockKind::TableCell);
        let collapsible_disabled = view.kind != SectionKind::Text;
        let list_disabled = quote_disabled;
        let section: Arc<str> = Arc::from(id.as_str());
        let style_label = match kind {
            Some(NativeBlockKind::Heading { level: 1 }) => "H1",
            Some(NativeBlockKind::Heading { level: 2 }) => "H2",
            Some(NativeBlockKind::Heading { level: 3 }) => "H3",
            Some(NativeBlockKind::Heading { level: 4 }) => "H4",
            Some(NativeBlockKind::Heading { level: 5 }) => "H5",
            Some(NativeBlockKind::Heading { level: 6 }) => "H6",
            _ => "Text",
        };
        let mut bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default());
        bar = bar.child(
            self.selection_dropdown_control(
                Button::new("selection-style")
                    .ghost()
                    .small()
                    .label(style_label)
                    .dropdown_caret(true)
                    .accessibility_label("Paragraph style")
                    .tooltip("Paragraph style")
                    .disabled(styles_disabled),
                section.clone(),
                SelectionPanel::Styles,
                cx,
            ),
        );
        for (name, label, mark) in [
            ("selection-bold", "B", MarkKind::Bold),
            ("selection-italic", "I", MarkKind::Italic),
            ("selection-underline", "U", MarkKind::Underline),
        ] {
            let title = match mark {
                MarkKind::Bold => "Bold",
                MarkKind::Italic => "Italic",
                _ => "Underline",
            };
            let target = section.clone();
            bar = bar.child(
                Toggle::new(name)
                    .ghost()
                    .small()
                    .label(label)
                    .tooltip(title)
                    .checked(model.mark_state(mark) == MarkState::On)
                    .disabled(marks_disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_selection_action(
                            &target,
                            CommandAction::Editor(EditorCommand::ToggleMark(mark)),
                            window,
                            cx,
                        );
                    })),
            );
        }
        let quote_section = section.clone();
        bar = bar.child(
            Button::new("selection-quote")
                .ghost()
                .small()
                .icon(self.selection_quote_icon.clone())
                .accessibility_label("Quote")
                .tooltip("Quote")
                .disabled(quote_disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_selection_action(
                        &quote_section,
                        CommandAction::Editor(EditorCommand::ToggleBlockquote),
                        window,
                        cx,
                    );
                })),
        );
        let link_section = section.clone();
        bar = bar.child(
            Button::new("selection-link")
                .ghost()
                .small()
                .icon(self.selection_link_icon.clone())
                .accessibility_label("Make or edit link")
                .tooltip("Link")
                .disabled(marks_disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_selection_link(&link_section, window, cx)
                })),
        );
        let collapsible_section = section.clone();
        bar = bar.child(
            Button::new("selection-collapsible")
                .ghost()
                .small()
                .icon(self.selection_collapsible_icon.clone())
                .accessibility_label("Turn into collapsible section")
                .tooltip("Collapsible")
                .disabled(collapsible_disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_selection_action(
                        &collapsible_section,
                        CommandAction::Collapsible,
                        window,
                        cx,
                    );
                })),
        );
        bar = bar.child(
            self.selection_dropdown_control(
                Button::new("selection-list")
                    .ghost()
                    .small()
                    .icon(self.selection_list_icon.clone())
                    .dropdown_caret(true)
                    .accessibility_label("Turn into list")
                    .tooltip("List")
                    .disabled(list_disabled),
                section.clone(),
                SelectionPanel::Lists,
                cx,
            ),
        );
        let mut toolbar = div()
            .id("selection-toolbar")
            .track_focus(&self.selection_toolbar_focus)
            .max_w((window.viewport_size().width - px(16.)).max(px(0.)))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .occlude()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                Toolbar::new("selection-toolbar-controls")
                    .small()
                    .content(bar),
            );
        if let Some(link) = self
            .selection_link
            .as_ref()
            .filter(|link| &link.section == id)
        {
            let field = div()
                .id("selection-link-url")
                .min_w(px(180.))
                .flex_1()
                .child(Input::new(&link.input).id("selection-link-input").small());
            #[cfg(test)]
            let field = {
                use gpui_kit::test::TestSupportExt;
                field.test_support()
            };
            let actions = div()
                .flex()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .child(
                    Button::new("selection-link-apply")
                        .small()
                        .label("Apply")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.apply_selection_link(false, window, cx)
                        })),
                )
                .child(
                    Button::new("selection-link-remove")
                        .ghost()
                        .small()
                        .label("Remove link")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.apply_selection_link(true, window, cx)
                        })),
                )
                .child(
                    Button::new("selection-link-cancel")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_selection_link(window, cx)
                        })),
                );
            toolbar = toolbar.child(
                div()
                    .flex()
                    .flex_wrap()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .child(field)
                    .child(actions),
            );
            if let Some(error) = link.error {
                toolbar = toolbar.child(div().text_xs().text_color(cx.theme().danger).child(error));
            }
        }
        #[cfg(test)]
        let toolbar = {
            use gpui_kit::test::TestSupportExt;
            toolbar.test_support()
        };
        let toolbar = toolbar.into_any_element();
        // Read selection geometry after native content prepaint, not from the
        // previous render frame, so the overlay tracks scrolling and reflow.
        Some(
            deferred(
                canvas(
                    move |_, window, cx| {
                        let bounds = editor.read(cx).selection_bounds()?;
                        let mut popup = anchored()
                            .anchor(Anchor::BottomCenter)
                            .attach(bounds, Anchor::TopCenter)
                            .offset(point(px(0.), px(-8.)))
                            .child(toolbar)
                            .into_any_element();
                        let viewport = window.viewport_size();
                        popup.layout_as_root(
                            size(
                                AvailableSpace::Definite(viewport.width),
                                AvailableSpace::Definite(viewport.height),
                            ),
                            window,
                            cx,
                        );
                        popup.prepaint_at(point(px(0.), px(0.)), window, cx);
                        Some(popup)
                    },
                    |_, popup, window, cx| {
                        if let Some(mut popup) = popup {
                            popup.paint(window, cx);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0(),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
