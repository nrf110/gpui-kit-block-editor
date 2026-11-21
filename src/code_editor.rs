use super::*;
use gpui_kit::component::{
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
};

pub(super) struct CodeLanguageMenu {
    section: SectionId,
    menu: Entity<PopupMenu>,
    _subscription: Subscription,
}

const LANGUAGES: &[(&str, Option<&str>)] = &[
    ("Plain text", None),
    ("Rust", Some("rust")),
    ("Python", Some("python")),
    ("JavaScript", Some("javascript")),
    ("TypeScript", Some("typescript")),
    ("JSON", Some("json")),
    ("HTML", Some("html")),
    ("CSS", Some("css")),
    ("Shell", Some("bash")),
    ("SQL", Some("sql")),
    ("Go", Some("go")),
    ("Java", Some("java")),
    ("C", Some("c")),
    ("C++", Some("cpp")),
    ("C#", Some("csharp")),
    ("Ruby", Some("ruby")),
    ("Swift", Some("swift")),
    ("Kotlin", Some("kotlin")),
    ("YAML", Some("yaml")),
    ("TOML", Some("toml")),
    ("Markdown", Some("markdown")),
    ("XML", Some("xml")),
    ("Mermaid", Some("mermaid")),
];

pub(super) fn code_language(content: &RichDocument) -> Option<Option<&str>> {
    let [node] = content.blocks.as_slice() else {
        return None;
    };
    (node.kind == RichKind::CodeBlock)
        .then(|| node.attrs.get("lang").and_then(serde_json::Value::as_str))
}

impl BlockEditor {
    pub(super) fn code_history_action(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self.views.values().any(|view| {
            view.editor
                .read(cx)
                .code_editor_state()
                .is_some_and(|input| input.focus_handle(cx).is_focused(window))
        });
        if !focused || self.is_composing(cx) {
            cx.propagate();
            return;
        }
        if let Err(error) = self.history(redo, window, cx) {
            self.error = Some(error.to_string());
            cx.notify();
        }
        cx.stop_propagation();
        window.prevent_default();
    }

    pub(super) fn reconcile_code_language_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = &self.code_language_menu else {
            return;
        };
        if self.views.get(&menu.section).is_some_and(|view| {
            code_language(&view.content).is_some() && (!view.previewable || view.show_source)
        }) {
            return;
        }
        let id = menu.section.clone();
        self.set_code_language_menu(&id, false, window, cx);
    }

    fn set_code_language_menu(
        &mut self,
        id: &str,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !open {
            if self
                .code_language_menu
                .as_ref()
                .is_some_and(|menu| menu.section == id)
            {
                let menu = self.code_language_menu.take().unwrap();
                if menu.menu.focus_handle(cx).contains_focused(window, cx) {
                    if let Some(view) = self.views.get(id) {
                        view.editor.update(cx, |state, cx| state.focus(window, cx));
                    } else {
                        self.focus_scope.focus(window, cx);
                    }
                }
                cx.notify();
            }
            return;
        }
        if let Err(error) = self.flush_local_edits(window, cx) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let Some(current) = self
            .views
            .get(id)
            .and_then(|view| code_language(&view.content))
        else {
            return;
        };
        let current = current.map(str::to_owned);
        self.completion = None;
        self.selection_dropdown = None;
        self.selection_link = None;
        let owner = cx.weak_entity();
        let section: Arc<str> = Arc::from(id);
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.min_w(px(180.)).max_h(px(260.)).scrollable(true);
            for &(label, language) in LANGUAGES {
                let owner = owner.clone();
                let section = section.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(current.as_deref() == language)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.code_language_menu = None;
                                // A peer may have removed or converted the block while the menu was open.
                                if !this
                                    .views
                                    .get(section.as_ref())
                                    .is_some_and(|view| code_language(&view.content).is_some())
                                {
                                    cx.notify();
                                    return;
                                }
                                let editor = this.views[section.as_ref()].editor.clone();
                                let selection = editor.read(cx).selection().clone();
                                this.views.get_mut(section.as_ref()).unwrap().show_source = true;
                                this.execute_command(
                                    &section,
                                    CommandAction::Editor(EditorCommand::SetCodeBlock {
                                        lang: language.map(str::to_owned),
                                    }),
                                    None,
                                    window,
                                    cx,
                                );
                                // A language change isn't a selection change.
                                if let Err(error) = editor
                                    .update(cx, |state, cx| state.set_selection(selection, cx))
                                {
                                    this.error = Some(format!("{error:?}"));
                                    cx.notify();
                                }
                            });
                        }),
                );
            }
            menu
        });
        let subscription =
            cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
                let id = this
                    .code_language_menu
                    .as_ref()
                    .filter(|active| active.menu == *menu)
                    .map(|active| active.section.clone());
                if let Some(id) = id {
                    this.set_code_language_menu(&id, false, window, cx);
                }
            });
        self.code_language_menu = Some(CodeLanguageMenu {
            section: id.to_owned(),
            menu: menu.clone(),
            _subscription: subscription,
        });
        menu.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn code_language_control(
        &self,
        id: &str,
        index: usize,
        language: SharedString,
        cx: &Context<Self>,
    ) -> Popover {
        let section: Arc<str> = Arc::from(id);
        let target = section.clone();
        let owner = cx.weak_entity();
        let open = self
            .code_language_menu
            .as_ref()
            .is_some_and(|menu| menu.section == id);
        Popover::new(("code-language-popover", index))
            .appearance(false)
            .overlay_closable(false)
            .anchor(Anchor::TopLeft)
            .trigger(
                Button::new(("code-language", index))
                    .ghost()
                    .small()
                    .icon(IconName::SquareTerminal)
                    .label(language)
                    .dropdown_caret(true)
                    .accessibility_label("Code language")
                    .tooltip("Code language"),
            )
            .open(open)
            .on_open_change(cx.listener(move |this, open, window, cx| {
                this.set_code_language_menu(&target, *open, window, cx);
            }))
            .content(move |_, window, cx| {
                let menu = owner.upgrade().and_then(|owner| {
                    owner
                        .read(cx)
                        .code_language_menu
                        .as_ref()
                        .filter(|menu| menu.section == section.as_ref())
                        .map(|menu| menu.menu.clone())
                });
                if let Some(menu) = &menu {
                    let focus = menu.focus_handle(cx);
                    if !focus.contains_focused(window, cx) {
                        focus.focus(window, cx);
                    }
                }
                let surface = div()
                    .id("code-language-menu")
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
}
