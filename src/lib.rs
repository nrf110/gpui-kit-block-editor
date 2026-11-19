//! A GPUI Kit section editor backed by a Yjs-compatible CRDT document.
extern crate gpui_kit as gpui;

pub mod block;
mod block_markdown;
pub mod document;
pub mod editor;
pub mod extension;
mod source_map;

pub use block::{BlockSpec, SectionKind};
pub use document::{Document, Section, SectionId};
pub use editor::{BlockEditor, DocumentEvent};
pub use extension::{
    CommandAction, CommandRegistry, ExtensionRegistry, PreviewExtension, SlashCommand,
};

/// Call after `gpui_kit::init`, before constructing editors.
pub fn init(cx: &mut gpui_kit::App) {
    markdown_editor::init(cx);
}
