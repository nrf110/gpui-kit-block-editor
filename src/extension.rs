use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use markdown_editor::{BlockKind, EditorCommand, MarkKind, TableEdit};

/// An executable item in the editor's slash menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SlashCommand {
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub action: CommandAction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandAction {
    Editor(EditorCommand),
    InsertMarkdown(String),
}

#[derive(Clone, Debug)]
struct IndexedCommand {
    command: SlashCommand,
    search_text: String,
}

/// Commands retain registration order, including when a search has multiple matches.
#[derive(Clone, Debug)]
pub struct CommandRegistry {
    commands: Vec<IndexedCommand>,
}

impl CommandRegistry {
    /// Register a command without replacing an existing command with the same ID.
    pub fn register(&mut self, command: SlashCommand) -> Result<()> {
        ensure!(
            !self
                .commands
                .iter()
                .any(|entry| entry.command.id == command.id),
            "a slash command with ID {:?} is already registered",
            command.id
        );
        let search_text = std::iter::once(command.id.as_str())
            .chain(std::iter::once(command.title.as_str()))
            .chain(std::iter::once(command.description.as_str()))
            .chain(command.keywords.iter().map(String::as_str))
            .flat_map(|text| text.chars().chain(std::iter::once(' ')))
            .flat_map(char::to_lowercase)
            .collect();
        self.commands.push(IndexedCommand {
            command,
            search_text,
        });
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
}

impl Default for CommandRegistry {
    fn default() -> Self {
        let mut registry = Self {
            commands: Vec::new(),
        };
        let mut add = |id: &str, title: &str, description: &str, keywords: &[&str], action| {
            registry
                .register(SlashCommand {
                    id: id.into(),
                    title: title.into(),
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
    }
}

/// A native renderer for the contents of a fenced Markdown code block.
/// Renderers receive the source only, without the opening and closing fences.
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
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        let mut registry = Self {
            commands: CommandRegistry::default(),
            previews: Vec::new(),
        };
        registry
            .register_preview(MermaidExtension)
            .expect("the built-in Mermaid language is unique");
        registry.commands.register(SlashCommand {
            id: "mermaid".into(),
            title: "Mermaid diagram".into(),
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
    /// Fence language matching and duplicate detection are ASCII case-insensitive.
    pub fn register_preview(&mut self, extension: impl PreviewExtension + 'static) -> Result<()> {
        let language = extension.language();
        ensure!(
            !language.is_empty() && !language.chars().any(char::is_whitespace),
            "preview language must be a nonempty fence language token"
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

    /// Render the first fenced code block whose language has a registered renderer.
    /// Backtick and tilde fences are supported, including fences longer than three
    /// characters and an unclosed final fence (which extends to EOF, as in Markdown).
    /// Fences must be standalone, with at most three leading spaces; nested list or
    /// blockquote containers are intentionally outside this section-level API.
    pub fn render_preview(&self, markdown: &str) -> Option<Result<String>> {
        let (extension, source) = self.preview_source(markdown)?;
        Some(extension.render_svg(source))
    }

    pub(crate) fn preview_source<'a>(
        &self,
        markdown: &'a str,
    ) -> Option<(&Arc<dyn PreviewExtension>, &'a str)> {
        struct OpenFence {
            marker: u8,
            length: usize,
            source_start: usize,
            preview: Option<usize>,
        }
        let mut open: Option<OpenFence> = None;
        let mut offset = 0;
        for line in markdown.split_inclusive('\n') {
            let parts = fence_parts(line.trim_end_matches(['\r', '\n']));
            if let Some(fence) = &open {
                if let Some((marker, length, tail)) = parts {
                    if marker == fence.marker && length >= fence.length && tail.trim().is_empty() {
                        if let Some(index) = fence.preview {
                            return Some((
                                &self.previews[index].extension,
                                &markdown[fence.source_start..offset],
                            ));
                        }
                        open = None;
                    }
                }
            } else if let Some((marker, length, info)) = parts {
                // A backtick fence's info string may not itself contain backticks.
                if marker != b'`' || !info.contains('`') {
                    let language = info.split_whitespace().next().unwrap_or("");
                    let preview = self
                        .previews
                        .iter()
                        .position(|entry| entry.language.eq_ignore_ascii_case(language));
                    open = Some(OpenFence {
                        marker,
                        length,
                        source_start: offset + line.len(),
                        preview,
                    });
                }
            }
            offset += line.len();
        }
        let fence = open?;
        let index = fence.preview?;
        Some((
            &self.previews[index].extension,
            &markdown[fence.source_start..],
        ))
    }
}

fn fence_parts(line: &str) -> Option<(u8, usize, &str)> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let line = &line[indentation..];
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = line.bytes().take_while(|byte| *byte == marker).count();
    (length >= 3).then(|| (marker, length, &line[length..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_command() -> SlashCommand {
        SlashCommand {
            id: "custom-note".into(),
            title: "Custom note".into(),
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
                "unexpected fenced source: {source:?}"
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

    #[test]
    fn custom_preview_receives_only_first_matching_fenced_body() {
        let registry = source_registry("first\n");
        let markdown = "Prose\n```rust\nignored\n```\n```custom extra-info\nfirst\n```\n```custom\nsecond\n```\n";
        assert!(
            registry
                .render_preview(markdown)
                .unwrap()
                .unwrap()
                .contains("<circle")
        );
    }

    #[test]
    fn supports_tilde_long_indented_and_crlf_fences() {
        let registry = source_registry("first\r\n");
        assert!(
            registry
                .render_preview("   ~~~~CUSTOM\r\nfirst\r\n  ~~~~~ \r\n")
                .unwrap()
                .is_ok()
        );
        let registry = source_registry("```\nfirst\n");
        assert!(
            registry
                .render_preview("````custom\n```\nfirst\n````\n")
                .unwrap()
                .is_ok()
        );
        let registry = source_registry("first");
        assert!(registry.render_preview("```custom\nfirst").unwrap().is_ok());
    }

    #[test]
    fn ignores_nonfences_and_fences_inside_other_code_blocks() {
        let registry = source_registry("unused");
        for markdown in [
            "custom\nfirst",
            "inline ```custom``` text",
            "```unknown\nfirst\n```",
            "```\nfirst\n```",
            "    ```custom\n    first\n    ```",
            "```text\n~~~custom\nfirst\n~~~\n```",
            "```custom`invalid\nfirst",
        ] {
            assert!(registry.render_preview(markdown).is_none(), "{markdown:?}");
        }
    }

    #[test]
    fn closing_fence_must_match_marker_length_and_have_no_info() {
        let registry = source_registry("~~~\n``` trailing\nfirst\n");
        assert!(
            registry
                .render_preview("```custom\n~~~\n``` trailing\nfirst\n```\n")
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
                .render_preview("```custom\nfirst\n```\n")
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
        let svg = registry.render_preview(&markdown).unwrap().unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("Start"));
        assert!(svg.contains("Finish"));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn invalid_mermaid_returns_an_error_and_does_not_fall_through() {
        let registry = ExtensionRegistry::default();
        let markdown = "```mermaid\nflowchart LR\nsubgraph S\nA --> B\n```\n```mermaid\nflowchart LR\nA --> B\n```\n";
        assert!(registry.render_preview(markdown).unwrap().is_err());
        assert!(MermaidExtension.render_svg("").is_err());
        assert!(MermaidExtension.render_svg("not a diagram").is_err());
    }
}
