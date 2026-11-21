//! Native structured rich-text blocks backed by a Yjs-compatible CRDT document.
extern crate gpui_kit as gpui;

pub mod block;
mod block_markdown;
mod button;
mod diagram_viewer;
pub mod document;
pub mod editor;
pub mod extension;
pub mod media;
mod media_view;
mod popup_menu;
mod rich_map;
mod style;
mod view;

pub use block::{BlockSpec, SectionKind};
pub use document::{Document, RelativePosition, Section, SectionId};
pub use editor::{BlockEditor, DocumentEvent};
pub use extension::{
    CommandAction, CommandRegistry, ExtensionRegistry, PreviewExtension, SlashCommand,
};
pub use markdown_editor::EditorTextStyle;
pub use markdown_editor::rich::{
    RICH_DOCUMENT_VERSION, RichAtom, RichDocument, RichKind, RichLength, RichLink, RichMarks,
    RichNode, RichRun, register_custom_codec,
};
pub use media::{FileMediaStore, MediaAttachment, MediaKind, MediaStore};
pub use style::EditorStyle;
pub use view::BlockEditorView;

/// Call after `gpui_kit::init`, before constructing editors.
pub fn init(cx: &mut gpui_kit::App) {
    markdown_editor::init(cx);
    popup_menu::init(cx);
}

#[cfg(test)]
mod media_tests;
