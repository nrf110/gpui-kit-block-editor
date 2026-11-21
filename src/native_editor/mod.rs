//! Private native editing engine, integrated from gpui-kit-markdown-editor.
//!
//! Modified for gpui-kit-block-editor: the former standalone crate is now an
//! internal module; its themed component wrapper and optional toolbar are removed.
//! The model, transactions, Markdown/rich import and export, input state, and
//! native rendering remain the engine behind the crate's public editor.
//!
//! The upstream design began in
//! [longbridge/gpui-kit#3049](https://github.com/longbridge/gpui-kit/pull/3049)
//! and [#3566](https://github.com/longbridge/gpui-kit/pull/3566).
//! Original Apache-2.0 provenance is preserved in the root license and notices.

mod code_editor;
mod command;
mod cull;
mod direct;
mod document;
mod editor;
mod element;
mod frame;
mod host;
mod id;
mod import;
mod inline;
mod input_rule;
mod kind;
mod markdown_source;
mod marks;
mod order;
mod position;
mod projection;
pub mod rich;
mod rich_bridge;
mod selection;
pub use rich::*;
mod serialize;
mod slice;
mod state;
mod step;
mod style;
mod support;
pub(crate) mod table;
mod table_ui;
mod transaction;
mod undo;

#[cfg(test)]
mod tests;

pub use command::{CommandError, EditorCommand};
pub use document::{BlockRef, DocumentRef};
pub use editor::BlockEditorModel;
pub use element::BlockEditor;
pub use gpui_kit::base::text::editor_host::ColumnAlign;
pub use input_rule::InputRule;
pub use state::{BlockEditorEvent, BlockEditorState, ClipboardPolicy, PasteRequest, SubmitPolicy};
pub use style::{BlockEditorStyle, EditorTextStyle};
pub use table::{TableEdit, TableLine};

/// Registers the editor's key bindings. Call once at startup, after `gpui_kit::init`.
pub fn init(cx: &mut gpui::App) {
    state::init(cx);
}
/// gpui-kit's own layers first (theme, global state), then the editor.
#[cfg(test)]
pub(crate) fn init_for_tests(cx: &mut gpui::App) {
    gpui_kit::init(cx);
    init(cx);
}

pub use id::BlockId;
pub use kind::BlockKind;
pub use marks::{MarkChange, MarkKind, MarkSet, MarkState};
pub use position::{Affinity, BlockPosition, PositionError};
pub use selection::{BlockSelection, SelectionError};
pub use transaction::{EditOrigin, Transaction};

pub use host::{CapturedRange, CapturedRangeError, SourceSnapshot};

/// Whether image atoms may load resources while the editor presents them.
/// `Placeholder` keeps the atomic document, source and selection but never
/// resolves URLs, decodes data, reads paths or creates image elements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImagePolicy {
    #[default]
    Load,
    Placeholder,
}
