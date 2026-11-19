use gpui_kit::component::{ActiveTheme, StyledExt, button::Button};
use gpui_kit::*;
use gpui_kit_block_editor::{
    BlockEditor, CommandAction, Document, DocumentEvent, ExtensionRegistry, SlashCommand,
};

struct Demo {
    left: Entity<BlockEditor>,
    right: Entity<BlockEditor>,
    connected: bool,
    updates: usize,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

fn extensions() -> ExtensionRegistry {
    let mut extensions = ExtensionRegistry::default();
    extensions
        .commands
        .register(SlashCommand {
            id: "callout".into(),
            title: "Callout".into(),
            description: "An application-defined extension".into(),
            keywords: vec!["note".into(), "custom".into()],
            action: CommandAction::InsertMarkdown(
                "> **Note**\n> A custom command, registered by the host application.".into(),
            ),
        })
        .expect("unique demo command");
    extensions
}

impl Demo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut document = Document::new();
        for markdown in [
            "# A place for the next idea\n\nA native, collaborative Markdown canvas. Every line is an independently movable block.",
            "## Make it yours\n\nType **/** to explore commands. Use **⌘B** for bold and **⌘I** for italic. Hover for the **⠿** drag handle. **Tab** nests a block; **Shift+Tab** lifts it.\n\n- Write in rich text\n- Shape a table or a list\n  - Drag items inside their list group\n- Turn an idea into a diagram",
            "```mermaid\nflowchart LR\n    Idea[An idea] --> Draft[A shared draft]\n    Draft --> Ship[Something great]\n```",
        ] {
            document
                .insert_section(None, markdown)
                .expect("valid initial blocks");
        }
        document.clear_history();
        let mut replica = Document::new();
        replica
            .apply_update(&document.encode_state())
            .expect("valid initial update");
        let left = cx.new(|cx| BlockEditor::new(document, extensions(), window, cx));
        let right = cx.new(|cx| BlockEditor::new(replica, extensions(), window, cx));
        let mut subscriptions = Vec::new();
        for (source, target) in [(left.clone(), right.clone()), (right.clone(), left.clone())] {
            subscriptions.push(cx.subscribe_in(
                &source,
                window,
                move |this, _, event: &DocumentEvent, window, cx| {
                    let DocumentEvent::Changed { update } = event;
                    this.updates += 1;
                    if this.connected {
                        if let Err(error) =
                            target.update(cx, |editor, cx| editor.apply_update(update, window, cx))
                        {
                            this.error = Some(error.to_string());
                        }
                    }
                    cx.notify();
                },
            ));
        }
        Self {
            left,
            right,
            connected: true,
            updates: 0,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn toggle_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connected {
            let result = (|| -> anyhow::Result<()> {
                // Compute both diffs before applying either, exactly as two offline peers do.
                let a = self
                    .left
                    .read(cx)
                    .document()
                    .encode_update(&self.right.read(cx).document().state_vector())?;
                let b = self
                    .right
                    .read(cx)
                    .document()
                    .encode_update(&self.left.read(cx).document().state_vector())?;
                self.right
                    .update(cx, |editor, cx| editor.apply_update(&a, window, cx))?;
                self.left
                    .update(cx, |editor, cx| editor.apply_update(&b, window, cx))?;
                Ok(())
            })();
            if let Err(error) = result {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        }
        self.connected = !self.connected;
        cx.notify();
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left = self.left.clone();
        let right = self.right.clone();
        div().size_full().flex().bg(cx.theme().background).text_color(cx.theme().foreground)
            .child(div().w(px(205.)).h_full().flex().flex_col().p_5().gap_5().bg(rgb(0x202c29)).text_color(rgb(0xe8eee9))
                .child(div().text_xs().font_semibold().child("FIELDNOTES / 001"))
                .child(div().mt_6().text_2xl().font_family("Georgia").child("Good ideas\nstart here."))
                .child(div().h(px(1.)).bg(rgb(0x46554d)))
                .child(div().text_sm().child("A working document"))
                .child(div().text_xs().text_color(rgb(0xa6b7ae)).child("Native GPUI Kit\nMarkdown, without the markup.\n\nTwo independent replicas.\nOne shared canvas."))
                .child(div().flex_1())
                .child(div().text_xs().text_color(rgb(0xa6b7ae)).child("TRY IT\n\nPause the connection. Edit both sides. Reconnect to merge your changes.")))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .child(div().px_6().py_4().border_b_1().border_color(cx.theme().border).flex().items_center().gap_3()
                    .child(div().flex_1().child(div().font_family("Georgia").text_2xl().child("The shared canvas"))
                        .child(div().mt_1().text_xs().text_color(cx.theme().muted_foreground).child("BLOCK EDITOR  /  MARKDOWN + MERMAID + YJS")))
                    .child(Button::new("connection").label(if self.connected { "Pause connection" } else { "Reconnect & merge" })
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_connection(window, cx))))
                    .child(Button::new("copy-markdown").label("Copy Markdown").on_click(cx.listener(|this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(this.left.read(cx).markdown()));
                    }))))
                .child(div().flex_1().min_h_0().flex()
                    .child(div().flex_1().min_w_0().flex().flex_col().border_r_1().border_color(cx.theme().border)
                        .child(div().px_5().py_3().border_b_1().border_color(cx.theme().border).text_xs().child("01  /  YOUR WORKSPACE"))
                        .child(left))
                    .child(div().flex_1().min_w_0().flex().flex_col()
                        .child(div().px_5().py_3().border_b_1().border_color(cx.theme().border).text_xs().child("02  /  COLLABORATOR"))
                        .child(right)))
                .child(div().px_5().py_2().border_t_1().border_color(cx.theme().border).text_xs().text_color(cx.theme().muted_foreground)
                    .child(self.error.clone().unwrap_or_else(|| format!("{}  ·  {} local transactions  ·  Yjs-compatible v1 updates", if self.connected { "● Connected in-process" } else { "○ Offline — edits stay local" }, self.updates)))))
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            gpui_kit_block_editor::init(cx);
            cx.activate(true);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1440.), px(940.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Fieldnotes — GPUI Kit Block Editor".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Demo::new(window, cx)),
            )
            .expect("open demo window");
        });
}
