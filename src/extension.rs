use std::{collections::HashMap, fmt, sync::Arc};

use crate::media::{FileMediaStore, MediaKind, MediaStore};
use crate::native_editor::rich::{RichDocument, RichKind, RichNode};
use crate::native_editor::{BlockKind, EditorCommand, MarkKind, TableEdit};
use anyhow::{Context, Result, ensure};
use gpui_kit::assets::IconName;
use gpui_kit::{AssetSource, component::Icon};

// Keep the component self-contained: these glyphs are not in GPUI Kit's default bundle.
gpui_kit::assets::icon_assets!(
    CommandIcons,
    [
        Type,
        Heading1,
        Heading2,
        Heading3,
        Bold,
        Italic,
        Strikethrough,
        Code,
        List,
        ListOrdered,
        ListTodo,
        TextQuote,
        SquareCode,
        Table,
        BetweenHorizontalStart,
        BetweenHorizontalEnd,
        Rows3,
        BetweenVerticalStart,
        BetweenVerticalEnd,
        Columns3,
        ListCollapse,
        Image,
        Video,
        Music,
        File,
        Workflow,
        MessageSquareText,
        Minus,
    ]
);

/// An executable item in the editor's slash menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SlashCommand {
    pub id: String,
    pub title: String,
    /// Menu heading. Matching labels share a group, including host-defined commands.
    pub group: String,
    /// Bundled icon displayed beside the command's title and description.
    pub icon: IconName,
    pub description: String,
    pub keywords: Vec<String>,
    pub action: CommandAction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandAction {
    Editor(EditorCommand),
    InsertMarkdown(String),
    Collapsible,
    CollapsibleList,
    InsertMedia(MediaKind),
}

#[derive(Clone, Debug)]
struct IndexedCommand {
    command: SlashCommand,
    search_text: String,
}

/// Groups retain first-registration order; commands retain registration order within a group.
#[derive(Clone)]
pub struct CommandRegistry {
    commands: Vec<IndexedCommand>,
    icons: HashMap<IconName, Icon>,
}

impl fmt::Debug for CommandRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandRegistry")
            .field("commands", &self.commands)
            .finish_non_exhaustive()
    }
}

impl CommandRegistry {
    /// Register a command without replacing an existing command with the same ID.
    pub fn register(&mut self, command: SlashCommand) -> Result<()> {
        ensure!(
            !command.group.trim().is_empty(),
            "a command group must not be empty"
        );
        ensure!(
            !self
                .commands
                .iter()
                .any(|entry| entry.command.id == command.id),
            "a slash command with ID {:?} is already registered",
            command.id
        );
        if let std::collections::hash_map::Entry::Vacant(entry) = self.icons.entry(command.icon) {
            let icon = match CommandIcons.load(command.icon.path().as_ref())? {
                Some(bytes) => Icon::default().data(&bytes),
                None => Icon::new(command.icon),
            };
            entry.insert(icon);
        }
        let search_text = std::iter::once(command.id.as_str())
            .chain(std::iter::once(command.title.as_str()))
            .chain(std::iter::once(command.description.as_str()))
            .chain(std::iter::once(command.group.as_str()))
            .chain(command.keywords.iter().map(String::as_str))
            .flat_map(|text| text.chars().chain(std::iter::once(' ')))
            .flat_map(char::to_lowercase)
            .collect();
        let index = self
            .commands
            .iter()
            .rposition(|entry| entry.command.group == command.group)
            .map_or(self.commands.len(), |index| index + 1);
        self.commands.insert(
            index,
            IndexedCommand {
                command,
                search_text,
            },
        );
        Ok(())
    }

    /// Match all whitespace-separated terms, case-insensitively, across command metadata.
    /// An empty query (or just `/`) returns every command.
    pub fn search(&self, query: &str) -> Vec<SlashCommand> {
        let query = query.trim().trim_start_matches('/').to_lowercase();
        self.commands
            .iter()
            .filter(|entry| {
                query
                    .split_whitespace()
                    .all(|term| entry.search_text.contains(term))
            })
            .map(|entry| entry.command.clone())
            .collect()
    }

    pub(crate) fn icon(&self, name: IconName) -> Icon {
        self.icons[&name].clone()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        let mut registry = Self {
            commands: Vec::new(),
            icons: HashMap::new(),
        };
        let mut add = |id: &str, title: &str, description: &str, keywords: &[&str], action| {
            let group = match &action {
                EditorCommand::ToggleList { .. } | EditorCommand::ToggleTask => "Lists",
                EditorCommand::InsertTable { .. } | EditorCommand::EditTable(_) => "Table",
                _ => "Text",
            };
            let icon = match &action {
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }) => IconName::Heading1,
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }) => IconName::Heading2,
                EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }) => IconName::Heading3,
                EditorCommand::ToggleMark(MarkKind::Bold) => IconName::Bold,
                EditorCommand::ToggleMark(MarkKind::Italic) => IconName::Italic,
                EditorCommand::ToggleMark(MarkKind::Strikethrough) => IconName::Strikethrough,
                EditorCommand::ToggleMark(MarkKind::Code) => IconName::Code,
                EditorCommand::ToggleList { ordered: false } => IconName::List,
                EditorCommand::ToggleList { ordered: true } => IconName::ListOrdered,
                EditorCommand::ToggleTask => IconName::ListTodo,
                EditorCommand::ToggleBlockquote => IconName::TextQuote,
                EditorCommand::SetCodeBlock { .. } => IconName::SquareCode,
                EditorCommand::InsertTable { .. } => IconName::Table,
                EditorCommand::EditTable(TableEdit::AddRowAbove) => {
                    IconName::BetweenHorizontalStart
                }
                EditorCommand::EditTable(TableEdit::AddRowBelow) => IconName::BetweenHorizontalEnd,
                EditorCommand::EditTable(TableEdit::RemoveRow) => IconName::Rows3,
                EditorCommand::EditTable(TableEdit::AddColumnLeft) => {
                    IconName::BetweenVerticalStart
                }
                EditorCommand::EditTable(TableEdit::AddColumnRight) => IconName::BetweenVerticalEnd,
                EditorCommand::EditTable(TableEdit::RemoveColumn) => IconName::Columns3,
                _ => IconName::Type,
            };
            registry
                .register(SlashCommand {
                    id: id.into(),
                    title: title.into(),
                    group: group.into(),
                    icon,
                    description: description.into(),
                    keywords: keywords.iter().map(|word| (*word).into()).collect(),
                    action: CommandAction::Editor(action),
                })
                .expect("built-in slash command IDs are unique");
        };
        add(
            "paragraph",
            "Paragraph",
            "Normal-size body text",
            &["text", "font", "size", "normal"],
            EditorCommand::SetBlockKind(BlockKind::Paragraph),
        );
        add(
            "heading-1",
            "Heading 1",
            "Largest heading size",
            &["h1", "font", "size", "large", "title"],
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }),
        );
        add(
            "heading-2",
            "Heading 2",
            "Medium heading size",
            &["h2", "font", "size", "medium", "subtitle"],
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
        );
        add(
            "heading-3",
            "Heading 3",
            "Small heading size",
            &["h3", "font", "size", "small"],
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }),
        );
        add(
            "bold",
            "Bold",
            "Toggle bold text",
            &["strong", "format"],
            EditorCommand::ToggleMark(MarkKind::Bold),
        );
        add(
            "italic",
            "Italic",
            "Toggle italic text",
            &["emphasis", "format"],
            EditorCommand::ToggleMark(MarkKind::Italic),
        );
        add(
            "strikethrough",
            "Strikethrough",
            "Toggle struck-through text",
            &["strike", "delete", "format"],
            EditorCommand::ToggleMark(MarkKind::Strikethrough),
        );
        add(
            "inline-code",
            "Inline code",
            "Toggle inline code formatting",
            &["monospace", "format"],
            EditorCommand::ToggleMark(MarkKind::Code),
        );
        add(
            "bulleted-list",
            "Bulleted list",
            "Toggle an unordered list",
            &["bullet", "unordered"],
            EditorCommand::ToggleList { ordered: false },
        );
        add(
            "ordered-list",
            "Ordered list",
            "Toggle a numbered list",
            &["number", "numbered"],
            EditorCommand::ToggleList { ordered: true },
        );
        add(
            "task-list",
            "Task list",
            "Toggle a checklist item",
            &["checkbox", "todo", "checklist"],
            EditorCommand::ToggleTask,
        );
        add(
            "quote",
            "Quote",
            "Toggle a blockquote",
            &["blockquote", "quotation"],
            EditorCommand::ToggleBlockquote,
        );
        add(
            "code-block",
            "Code block",
            "Turn the current block into source code",
            &["fence", "monospace"],
            EditorCommand::SetCodeBlock { lang: None },
        );
        add(
            "table",
            "Insert table",
            "Insert a three-row, three-column table",
            &["grid", "cells"],
            EditorCommand::InsertTable { rows: 3, cols: 3 },
        );
        add(
            "table-add-row-above",
            "Add row above",
            "Insert a table row above the current row",
            &["table", "row", "insert"],
            EditorCommand::EditTable(TableEdit::AddRowAbove),
        );
        add(
            "table-add-row-below",
            "Add row below",
            "Insert a table row below the current row",
            &["table", "row", "insert"],
            EditorCommand::EditTable(TableEdit::AddRowBelow),
        );
        add(
            "table-remove-row",
            "Remove row",
            "Delete the current table row",
            &["table", "row", "delete"],
            EditorCommand::EditTable(TableEdit::RemoveRow),
        );
        add(
            "table-add-column-left",
            "Add column left",
            "Insert a table column to the left",
            &["table", "column", "insert"],
            EditorCommand::EditTable(TableEdit::AddColumnLeft),
        );
        add(
            "table-add-column-right",
            "Add column right",
            "Insert a table column to the right",
            &["table", "column", "insert"],
            EditorCommand::EditTable(TableEdit::AddColumnRight),
        );
        add(
            "table-remove-column",
            "Remove column",
            "Delete the current table column",
            &["table", "column", "delete"],
            EditorCommand::EditTable(TableEdit::RemoveColumn),
        );
        registry
            .register(SlashCommand {
                id: "collapsible".into(),
                title: "Collapsible section".into(),
                group: "Layout".into(),
                icon: IconName::ListCollapse,
                description: "Group content under an expandable title".into(),
                keywords: vec!["toggle".into(), "details".into(), "section".into()],
                action: CommandAction::Collapsible,
            })
            .expect("the built-in collapsible command ID is unique");
        registry
            .register(SlashCommand {
                id: "collapsible-list".into(),
                title: "Collapsible list".into(),
                group: "Lists".into(),
                icon: IconName::ListCollapse,
                description: "A list of expandable titles and nested content".into(),
                keywords: vec!["toggle-list".into(), "disclosure".into(), "fold".into()],
                action: CommandAction::CollapsibleList,
            })
            .expect("the built-in collapsible list command ID is unique");
        registry
            .register(SlashCommand {
                id: "divider".into(),
                title: "Divider".into(),
                group: "Layout".into(),
                icon: IconName::Minus,
                description: "Separate content with a horizontal line".into(),
                keywords: vec!["rule".into(), "horizontal".into(), "separator".into()],
                action: CommandAction::Editor(EditorCommand::InsertBlock(
                    BlockKind::HorizontalRule,
                )),
            })
            .expect("the built-in divider command ID is unique");
        for (id, title, description, keywords, kind) in [
            (
                "image",
                "Image",
                "Choose an image to preview",
                &["photo", "picture", "media"][..],
                MediaKind::Image,
            ),
            (
                "video",
                "Video",
                "Attach a video",
                &["movie", "clip", "media"][..],
                MediaKind::Video,
            ),
            (
                "audio",
                "Audio",
                "Attach audio",
                &["sound", "music", "media"][..],
                MediaKind::Audio,
            ),
            (
                "file",
                "File",
                "Attach a PDF, Word document, or other file",
                &["attachment", "pdf", "document", "media"][..],
                MediaKind::File,
            ),
        ] {
            registry
                .register(SlashCommand {
                    id: id.into(),
                    title: title.into(),
                    group: "Media".into(),
                    icon: match kind {
                        MediaKind::Image => IconName::Image,
                        MediaKind::Video => IconName::Video,
                        MediaKind::Audio => IconName::Music,
                        MediaKind::File => IconName::File,
                    },
                    description: description.into(),
                    keywords: keywords.iter().map(|word| (*word).into()).collect(),
                    action: CommandAction::InsertMedia(kind),
                })
                .expect("built-in media command IDs are unique");
        }
        registry
    }
}

/// A native renderer for the semantic contents of a code block.
/// Renderers receive plain code text, independent of Markdown fences.
pub trait PreviewExtension: Send + Sync {
    fn language(&self) -> &str;
    fn render_svg(&self, source: &str) -> Result<String>;
}

/// Mermaid is registered exactly like any application-provided preview extension.
#[derive(Clone, Copy, Debug, Default)]
pub struct MermaidExtension;

impl PreviewExtension for MermaidExtension {
    fn language(&self) -> &str {
        "mermaid"
    }

    fn render_svg(&self, source: &str) -> Result<String> {
        mermaid_rs_renderer::render_strict(source, mermaid_rs_renderer::RenderOptions::default())
            .context("could not render Mermaid diagram")
    }
}

struct RegisteredPreview {
    language: String,
    extension: Arc<dyn PreviewExtension>,
}

pub struct ExtensionRegistry {
    pub commands: CommandRegistry,
    previews: Vec<RegisteredPreview>,
    pub(crate) media_store: Arc<dyn MediaStore>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        let mut registry = Self {
            commands: CommandRegistry::default(),
            previews: Vec::new(),
            media_store: Arc::new(FileMediaStore::default()),
        };
        registry
            .register_preview(MermaidExtension)
            .expect("the built-in Mermaid language is unique");
        registry.commands.register(SlashCommand {
            id: "mermaid".into(),
            title: "Mermaid diagram".into(),
            group: "Diagrams".into(),
            icon: IconName::Workflow,
            description: "Insert a natively rendered flowchart".into(),
            keywords: vec!["diagram".into(), "flowchart".into(), "graph".into()],
            action: CommandAction::InsertMarkdown(
                "```mermaid\nflowchart LR\n    A[Start] --> B{Ready?}\n    B -->|Yes| C[Finish]\n    B -->|No| A\n```".into()
            ),
        }).expect("the built-in Mermaid command ID is unique");
        registry
    }
}

impl ExtensionRegistry {
    /// Configure managed attachments before constructing an editor.
    /// Hosts distribute asset files separately from CRDT metadata updates.
    pub fn with_media_store(mut self, store: Arc<dyn MediaStore>) -> Self {
        self.media_store = store;
        self
    }

    /// Code language matching and duplicate detection are ASCII case-insensitive.
    pub fn register_preview(&mut self, extension: impl PreviewExtension + 'static) -> Result<()> {
        let language = extension.language();
        ensure!(
            !language.is_empty() && !language.chars().any(char::is_whitespace),
            "preview language must be a nonempty code language token"
        );
        ensure!(
            !self
                .previews
                .iter()
                .any(|entry| entry.language.eq_ignore_ascii_case(language)),
            "a preview for language {language:?} is already registered"
        );
        let language = language.to_owned();
        self.previews.push(RegisteredPreview {
            language,
            extension: Arc::new(extension),
        });
        Ok(())
    }

    /// Render the first semantic code block with a registered language.
    pub fn render_preview(&self, content: &RichDocument) -> Option<Result<String>> {
        let (extension, source) = self.preview_source(content)?;
        Some(extension.render_svg(&source))
    }

    pub(crate) fn preview_source(
        &self,
        content: &RichDocument,
    ) -> Option<(&Arc<dyn PreviewExtension>, String)> {
        fn find<'a, 'b>(
            nodes: &'a [RichNode],
            previews: &'b [RegisteredPreview],
        ) -> Option<(&'b Arc<dyn PreviewExtension>, &'a RichNode)> {
            for node in nodes {
                if node.kind == RichKind::CodeBlock {
                    if let Some(language) = node.attrs.get("lang").and_then(|value| value.as_str())
                    {
                        if let Some(preview) = previews
                            .iter()
                            .find(|entry| entry.language.eq_ignore_ascii_case(language))
                        {
                            return Some((&preview.extension, node));
                        }
                    }
                } else if let Some(found) = find(&node.children, previews) {
                    return Some(found);
                }
            }
            None
        }
        let (extension, node) = find(&content.blocks, &self.previews)?;
        Some((extension, node.text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_command() -> SlashCommand {
        SlashCommand {
            id: "custom-note".into(),
            title: "Custom note".into(),
            group: "Callouts".into(),
            icon: IconName::MessageSquareText,
            description: "Insert an application-specific note".into(),
            keywords: vec!["admonition".into()],
            action: CommandAction::InsertMarkdown("> Note\n".into()),
        }
    }

    #[test]
    fn commands_are_extensible_searchable_and_cloneable() {
        let mut registry = CommandRegistry::default();
        let command = custom_command();
        registry.register(command.clone()).unwrap();
        assert_eq!(registry.search("/ADMONITION custom"), vec![command.clone()]);
        assert_eq!(
            registry.search("application-specific"),
            vec![command.clone()]
        );
        assert!(registry.search("no-such-command").is_empty());
        let mut results = registry.search("admonition");
        results[0].title.clear();
        assert_eq!(registry.search("admonition"), vec![command]);
        assert_eq!(registry.search("/").len(), registry.search("").len());
    }

    #[test]
    fn duplicate_command_does_not_replace_original() {
        let mut registry = CommandRegistry::default();
        let command = custom_command();
        registry.register(command.clone()).unwrap();
        let count = registry.search("").len();
        let mut duplicate = command.clone();
        duplicate.title = "Replacement".into();
        assert!(registry.register(duplicate).is_err());
        assert_eq!(registry.search("").len(), count);
        assert_eq!(registry.search("admonition"), vec![command]);
    }

    #[test]
    fn late_extensions_join_existing_groups_and_group_names_are_searchable() {
        let mut registry = CommandRegistry::default();
        let mut callout = custom_command();
        callout.group = "Text".into();
        registry.register(callout.clone()).unwrap();
        let commands = registry.search("");
        let index = commands
            .iter()
            .position(|command| command.id == callout.id)
            .unwrap();
        assert_eq!(commands[index - 1].group, "Text");
        assert_eq!(commands[index + 1].group, "Lists");
        let text = registry.search("/TEXT");
        assert!(text.contains(&callout));
        assert!(text.iter().all(|command| command.group == "Text"));

        let mut custom = SlashCommand {
            id: "host-note".into(),
            ..custom_command()
        };
        registry.register(custom.clone()).unwrap();
        assert_eq!(registry.search("Callouts"), vec![custom.clone()]);
        custom.id = "another-host-note".into();
        registry.register(custom.clone()).unwrap();
        assert_eq!(registry.search("").last(), Some(&custom));
    }

    #[test]
    fn builtins_cover_formatting_lists_and_table_edits() {
        let actions: Vec<_> = CommandRegistry::default()
            .search("")
            .into_iter()
            .map(|command| command.action)
            .collect();
        for command in [
            EditorCommand::SetBlockKind(BlockKind::Paragraph),
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 1 }),
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 2 }),
            EditorCommand::SetBlockKind(BlockKind::Heading { level: 3 }),
            EditorCommand::ToggleMark(MarkKind::Bold),
            EditorCommand::ToggleMark(MarkKind::Italic),
            EditorCommand::ToggleMark(MarkKind::Strikethrough),
            EditorCommand::ToggleMark(MarkKind::Code),
            EditorCommand::ToggleList { ordered: false },
            EditorCommand::ToggleList { ordered: true },
            EditorCommand::ToggleTask,
            EditorCommand::ToggleBlockquote,
            EditorCommand::SetCodeBlock { lang: None },
            EditorCommand::InsertTable { rows: 3, cols: 3 },
            EditorCommand::EditTable(TableEdit::AddRowAbove),
            EditorCommand::EditTable(TableEdit::AddRowBelow),
            EditorCommand::EditTable(TableEdit::RemoveRow),
            EditorCommand::EditTable(TableEdit::AddColumnLeft),
            EditorCommand::EditTable(TableEdit::AddColumnRight),
            EditorCommand::EditTable(TableEdit::RemoveColumn),
        ] {
            assert!(actions.contains(&CommandAction::Editor(command)));
        }
        assert!(actions.contains(&CommandAction::Collapsible));
        let commands = CommandRegistry::default().search("details toggle");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "collapsible");
    }

    struct ExactSource {
        language: &'static str,
        expected: &'static str,
    }

    impl PreviewExtension for ExactSource {
        fn language(&self) -> &str {
            self.language
        }

        fn render_svg(&self, source: &str) -> Result<String> {
            ensure!(
                source == self.expected,
                "unexpected semantic code text: {source:?}"
            );
            Ok("<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"1\"/></svg>".into())
        }
    }

    fn source_registry(expected: &'static str) -> ExtensionRegistry {
        let mut registry = ExtensionRegistry::default();
        registry
            .register_preview(ExactSource {
                language: "custom",
                expected,
            })
            .unwrap();
        registry
    }

    fn code(language: Option<&str>, text: &str) -> RichNode {
        let mut node = RichNode::paragraph(text);
        node.kind = RichKind::CodeBlock;
        if let Some(language) = language {
            node.attrs.insert("lang".into(), language.into());
        }
        node
    }

    fn content(blocks: Vec<RichNode>) -> RichDocument {
        RichDocument {
            blocks,
            ..RichDocument::default()
        }
    }

    #[test]
    fn custom_preview_receives_only_first_matching_semantic_body() {
        let registry = source_registry("first\n");
        let document = content(vec![
            RichNode::paragraph("Prose"),
            code(Some("rust"), "ignored\n"),
            code(Some("custom"), "first\n"),
            code(Some("custom"), "second\n"),
        ]);
        assert!(
            registry
                .render_preview(&document)
                .unwrap()
                .unwrap()
                .contains("<circle")
        );
    }

    #[test]
    fn semantic_code_preserves_crlf_fences_and_terminal_text() {
        for text in ["first\r\n", "```\nfirst\n", "first"] {
            let registry = source_registry(text);
            let mut node = code(Some("CUSTOM"), text);
            node.attrs.insert("meta".into(), "extra-info".into());
            assert!(
                registry
                    .render_preview(&content(vec![node]))
                    .unwrap()
                    .is_ok()
            );
        }
    }

    #[test]
    fn ignores_fence_like_prose_and_unknown_or_missing_languages() {
        let registry = source_registry("unused");
        for node in [
            RichNode::paragraph("```custom\nfirst\n```"),
            code(Some("unknown"), "first"),
            code(None, "first"),
            code(Some("text"), "~~~custom\nfirst\n~~~"),
            code(Some("custom extra-info"), "first"),
        ] {
            assert!(registry.render_preview(&content(vec![node])).is_none());
        }
    }

    #[test]
    fn semantic_preview_discovers_nested_code_without_parsing_its_body() {
        let registry = source_registry("~~~\n``` trailing\nfirst\n");
        let mut parent = RichNode::paragraph("");
        parent.kind = RichKind::Quote;
        parent
            .children
            .push(code(Some("custom"), "~~~\n``` trailing\nfirst\n"));
        assert!(
            registry
                .render_preview(&content(vec![parent]))
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn preview_duplicate_registration_is_case_insensitive_and_nonmutating() {
        let mut registry = source_registry("first\n");
        assert!(
            registry
                .register_preview(ExactSource {
                    language: "CUSTOM",
                    expected: "replacement"
                })
                .is_err()
        );
        assert!(
            registry
                .render_preview(&content(vec![code(Some("custom"), "first\n")]))
                .unwrap()
                .is_ok()
        );
        assert!(registry.register_preview(MermaidExtension).is_err());
    }

    #[test]
    fn mermaid_command_renders_a_real_svg() {
        let registry = ExtensionRegistry::default();
        let command = registry
            .commands
            .search("mermaid")
            .into_iter()
            .next()
            .unwrap();
        let CommandAction::InsertMarkdown(markdown) = command.action else {
            panic!("Mermaid inserts Markdown")
        };
        let content = RichDocument::from_markdown(&markdown).unwrap();
        let svg = registry.render_preview(&content).unwrap().unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("Start"));
        assert!(svg.contains("Finish"));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn invalid_mermaid_returns_an_error_and_does_not_fall_through() {
        let registry = ExtensionRegistry::default();
        let document = content(vec![
            code(Some("mermaid"), "flowchart LR\nsubgraph S\nA --> B\n"),
            code(Some("mermaid"), "flowchart LR\nA --> B\n"),
        ]);
        assert!(registry.render_preview(&document).unwrap().is_err());
        assert!(MermaidExtension.render_svg("").is_err());
        assert!(MermaidExtension.render_svg("not a diagram").is_err());
    }
}
