# GPUI Kit Block Editor

A native, embeddable structured rich-text editor for [GPUI Kit](https://gpui-kit.com/), backed by [Yrs](https://github.com/y-crdt/y-crdt). Markdown is an import/export format, not the editing or storage model.

This is our versioned document schema, not Linear's private schema or a `y-prosemirror` binding. Yjs wire compatibility does not imply document-schema compatibility.

## Run

```sh
cargo run --example demo
cargo test --lib
```

Rust 1.100.0 is pinned in `rust-toolchain.toml`. Native builds need GPUI Kit's [platform prerequisites](https://gpui-kit.com/docs/installation); macOS needs Xcode and its command-line tools. `Cargo.lock` pins the application dependencies. Use `--offline` when dependencies are cached and registry access is unavailable.

The demo connects two independent CRDT replicas in-process. Pause the connection, edit both panes, then reconnect to merge. The initial collapsible section can be expanded independently in each pane. Copy Markdown exports the left document. Transport hosting and durable persistence remain host responsibilities.

## Native document model

- `Section` has a permanent ID, `parent`, structural `kind`, derived `depth`, and semantic `content: RichDocument`.
- `RichDocument` contains versioned `RichNode`s with stable IDs, typed `RichKind`s, metadata attributes, marked text runs, inline atoms, and children. Headings store a level; code stores a language and plain text; tables store rows/cells. Formatting is data, not Markdown delimiters.
- `SectionKind` describes the outer block tree: text, list group, ordinary/task/collapsible list item, quote, or editable collapsible title. A table remains one draggable outer block, with semantic rows/cells inside its content.
- Native editing and reconciliation use structured snapshots directly. Stable node IDs map selections to attributed CRDT text; no Markdown AST or source-offset caret mapping is involved.
- Markdown input rules and explicit Markdown/template imports remain available at the boundary. Passing `"**literal**"` to `RichDocument::paragraph` stores literal asterisks; it does not apply bold.

The native editing engine is incorporated into this crate under the private `src/native_editor` module. It derives from [gpui-kit-markdown-editor 0.3.0](https://github.com/wabzqem/gpui-kit-markdown-editor), with structured import/export, stable external node identities, native code editing, and per-instance presentation extensions. There is no separate backend crate or local path dependency.

## Editing

- Enter/Shift+Enter creates and focuses a sibling block. Code and table cells retain native internal-line editing. Markdown import splits ordinary paragraph lines into blocks, while preserving explicit hard breaks and keeping code/tables atomic.
- **Up / Down** moves between visible blocks at the first/last visual line, retaining the preferred horizontal position across short blocks. Wrapped text, code lines, and table rows navigate internally first; collapsed descendants are skipped. Attachment cards receive visible block selection rather than hidden input focus, and entering a diagram reveals its editable source. Cross-block movement does not change shared content or undo history.
- **Backspace / Delete** at the start of an empty paragraph, heading, or list item removes it and focuses a neighboring block. Backspace prefers the previous block's end; Delete prefers the next block's start. Nested content is promoted rather than deleted, and empty list/quote containers are pruned. The final empty paragraph remains editable; a lone empty list item becomes a plain paragraph. Removal is one collaborative undo step.
- **Tab / Shift+Tab** nests and unnests blocks when the slash menu is closed. Nested children move with their parent. Enter on an empty list item exits its list; new task items start unchecked.
- Hover a row to reveal its vertically centered **+** command button, **⠿** grip, and trash icon. Drop on another row to move before it, including across lists. A list's end strip appends to that list; strips before/after lists pull items to the parent level. The document's bottom drop area moves items out to the top level. Lists renumber automatically.
- Click a grip to select a block. **Shift-click** selects a visible range; **Cmd/Ctrl-click** toggles individual blocks. Clicking block content, blank space, or outside the editor clears the grip selection without consuming the click; **Escape** also clears it. Drag a selected grip to move the selection in document order. A multi-block selection covering every item moves the original list container; **Alt/Option-click** explicitly selects a whole list, including a one-item list.
- Ending a drag clears selection and its range anchor, including snap-back, rejected-drop, and cancellation paths. No-op/cancelled drags do not change content or create an undo step. Selected ancestors cover their descendants, so a subtree never moves twice. Cross-editor drops, cycles, and noneditable containers inside lists are rejected. Collapsible titles adopt the destination's list role without losing their bodies or folding state.
- The trash icon deletes its block and children. **Add block** creates a top-level paragraph.
- Click **+** to the left of a block's grip, or type **/**, to open the command menu. Opening with **+** preserves the text and selection without inserting a slash; formatting commands apply to that selection. The **+** menu floats beside its button without moving the block. Wheel scrolling stays inside either menu, including at its scroll limits. Scroll to browse every command, or type **/** and keep typing to filter. **Up/Down** keeps the selected command in view; **Enter/Tab** accepts it and **Escape** dismisses the menu. Commands include headings, marks, lists/tasks, quotes, tables and cell operations, code, Mermaid, and collapsible sections. IME composition is not intercepted.
- **Cmd/Ctrl+B / I** applies styling. **Cmd/Ctrl+Z / Shift+Z** undoes/redoes this replica's edits without undoing peers' changes. Heading levels provide font sizes; arbitrary font families/point sizes are not exposed by the current native renderer.

The command menu groups related actions under **Text**, **Lists**, **Table**, **Layout**, **Media**, and **Diagrams**. Headings are not selectable; keyboard navigation moves between commands and keeps them visible. Search matches both commands and their group names, hiding empty groups. Host extensions set `SlashCommand.group` to join an existing heading or introduce a new one. Groups follow first-registration order, with commands in registration order within each group.

Group headings are larger and semibold; indented command rows pair a muted icon with a title and description. `SlashCommand.icon` selects a `gpui_kit::assets::IconName`. Built-in command glyphs are embedded by the editor and cached at registration, so hosts do not need to configure extra assets for them. Hosts choosing other catalog icons must include those icons in their application's asset source.

### Selection toolbar

Highlight text to show a floating toolbar above the selected glyphs. It follows wrapping and scrolling without moving the document, and flips below the selection when there is no room above. The toolbar offers paragraph/heading 1–6 presets, bold, italic, underline, quote, link editing, collapsible sections, and bulleted, numbered, task, or collapsible lists.

Style and list choices open separate, keyboard-navigable dropdown menus anchored to their buttons, without adding a toolbar row. Wheel scrolling stays inside each dropdown, even at its scroll limits. Quote, link, collapsible-section, and list controls use embedded icons with hover tooltips and accessible labels.

Inline styles and links affect only the highlighted text. Heading and structural actions affect its containing block, retaining its text and descendants; unavailable conversions are disabled in code/table or container contexts. Formatting preserves the selected range for successive actions. Collapsible conversion expands the section and focuses its new child.

**Link** opens a padded editing row and focuses a URL field prefilled from the selection's existing link. **Apply** or **Enter** saves; **Cancel** or **Escape** discards the draft. **Remove link**, or applying an empty URL, unlinks the text. HTTP, HTTPS, mailto, relative, and fragment URLs are supported; unsafe schemes are rejected without changing the document. The target uses CRDT-relative positions, so peer edits during URL entry do not redirect the action to unrelated text. Toolbar actions participate in local collaborative undo.

### Tables

The hover-only corner grid handle selects the whole table; its highlight covers complete cells, including their padding and trailing column space. Clicking elsewhere deselects it without changing content or scrolling back to the table.

Hover a row or column to reveal an ellipsis icon to its left or above it. Click to insert rows above/below, insert columns left/right, or delete that row/column. Column menus also offer alignment. Header-row deletion remains disabled. Escape or an outside click closes the menu, and wheel input over it does not scroll the document. Menu edits use the same shared storage and collaborative undo as keyboard/slash commands.

### Dividers

Choose **Layout → Divider** from the **+** or slash menu, or type **/divider** and press **Enter**. An empty paragraph/heading without children becomes a native horizontal rule; otherwise the divider is inserted below the current block without replacing its text. On list or collapsible titles, it is inserted into the body before existing children, leaving the title intact.

A new paragraph below the divider receives focus for continued typing. Dividers use the normal block drag and trash controls, persist in native JSON and CRDT updates, and export as Markdown `---`. Insertion and its typing paragraph are one collaborative undo step.

Divider object selections clear when clicking another block or blank space, or when focus moves elsewhere. Dismissal does not edit shared content or scroll back to the divider; ordinary text selections remain available to formatting controls.

### Code snippets

**/code** uses GPUI Kit's editable **Editor** component: line numbers, syntax highlighting, indentation, horizontal scrolling, and a bounded-height code viewport. Choose a language from the snippet's dropdown; **Plain text** clears the language without removing the code block. Unknown imported language identifiers are retained. The dropdown scrolls independently of the document.

**Enter** inserts a code line; **Tab / Shift+Tab** indents/outdents; **Cmd/Ctrl+Enter** exits to a paragraph below. Backspace in an empty snippet returns it to a paragraph. Up/Down stays within code rows until reaching the block boundary. The existing selection toolbar remains available.

Code still stores the same semantic code node, stable identity, plain text, language, and metadata. Markdown export remains fenced code; native JSON and CRDT storage are unchanged. The code input follows the existing CRDT-relative selections and collaborative undo, including peer edits and IME composition. Mermaid's source view uses the same code editor; its rendered preview is unchanged.

### Collapsible sections

Type **/collapsible** and press **Enter**, or scroll the **/** menu and click **Collapsible section**. This converts the current block into an editable title and creates an editable child. An empty title becomes `Details`; existing content and descendants are retained. The creator's view expands immediately for editing.

The title's chevron shows or hides its entire subtree. The shared `default_collapsed` value starts true; each editor's expansion override is local and does not emit a document update. Hidden body editors and content are retained, so peer edits are available on reopening. Focusing a hidden child expands its ancestors. Dragging or deleting the title moves or removes the entire section; undo restores it.

These are explicit title/body containers, not ranges inferred from the next heading. Nest existing blocks beneath the title to include them.

### Collapsible lists

Choose **Lists → Collapsible list** in the **+** or slash menu, type **/collapsible-list**, or use the selection toolbar's **List** dropdown. The current block becomes a foldable list title, preserving its rich text and existing children. Empty titles remain blank; items without children receive an initial editable body. The creator's view opens immediately and keeps the title focused.

**Enter** in a title splits it into sibling collapsible items; **Enter** on an empty title exits the list. **Tab / Shift+Tab**, block selection, whole-list dragging, reordering, and collaborative undo work as with other lists. Pulling an item out of its group preserves it as a standalone collapsible section; dragging it back into a list restores its collapsible-item role. Reapplying **Collapsible list** also lifts it into a standalone section. Explicitly choosing a bulleted, numbered, or task list removes folding but preserves the body.

Each chevron hides only that item's descendants, independently on each replica. `SectionKind::CollapsibleListItem` stores the shared initial folding state inside an ordinary list group; mixed item types are allowed. Native JSON and CRDT updates retain the item type. Markdown import/export represents these items as list entries containing `<details>` disclosures, preserving title/body structure rather than inserting an empty list-item wrapper.

### Mermaid

**/mermaid** inserts a code block with semantic language `mermaid` and opens its native SVG preview. Hover for the source/preview toggle immediately left of the trash icon. Only one view is shown at a time; source edits refresh the preview asynchronously. Errors remain switchable back to source.

Hover for **Expand diagram** to open a near-full-window viewer. Use **− / +** or the mouse wheel to zoom (25–800% relative to Fit), drag to pan, and **Fit** to recenter. Wheel zoom stays anchored at the pointer and does not scroll the document. Close with **Escape**, the close icon, or the backdrop. Zoom and pan are local: they do not change shared content or undo history. Open viewers refresh with diagram edits; deleted or non-diagram blocks show an unavailable message rather than stale content.

The viewer retains parsed SVG and rasterizes off the UI thread at the displayed resolution, capped at 4096 pixels per texture side. Rapid camera changes coalesce behind one raster job.

Preview mode is local to each editor. Diagrams work inside lists, quotes, and collapsibles. Rendering uses [mermaid-rs-renderer](https://github.com/1jehuang/mermaid-rs-renderer), without Node or a browser; syntax/fidelity follow that renderer rather than full Mermaid.js parity.

### Media attachments

Use **/image**, **/video**, **/audio**, or **/file**, or choose **Image**, **Video**, **Audio**, or **File** from a block's **+** menu. The native file picker accepts multiple files. Cancelling leaves the document unchanged. Images preview inline with their aspect ratio preserved; video, audio, PDFs, Word documents, and other files appear as attachment cards. **Open** uses the system's associated application; **Save** exports a copy. Executable/active and unknown file types can be saved but cannot be opened directly from the editor.

Drag files from the filesystem onto a block to insert before it, onto a list boundary to use that group, or into unused editor space to append at the top level. Types are inferred from file signatures and extensions. Directories and symlinks are rejected. A batch preserves file order and is one collaborative undo step. Imported media blocks retain the existing drag, nesting, selection, and trash controls. File reads/copies and asset resolution run off the UI thread.

Files are copied into managed storage, so moving/deleting the original does not break the attachment. Only versioned metadata (asset UUID, kind, filename, MIME type, and byte length) is stored in a native custom block and replicated through CRDT updates/JSON. Absolute paths and file bytes are not replicated. Markdown export produces `asset://<UUID>` links, not a self-contained media bundle or an attachment-metadata roundtrip.

The default store uses the persistent user-data directory (`~/Library/Application Support/gpui-kit-block-editor/media` on macOS). Configure an application-owned directory before constructing the editor:

```rust
use std::sync::Arc;
use gpui_kit_block_editor::{ExtensionRegistry, FileMediaStore};

let extensions = ExtensionRegistry::default()
    .with_media_store(Arc::new(FileMediaStore::new("/path/to/application/assets")));
```

Implement `MediaStore` for other backends. Its blocking operations are dispatched to the background executor by the component. Multiplayer hosts must upload/distribute assets separately, using `FileMediaStore::install_file(&attachment, path)` to fulfil a peer reference without changing its UUID. Missing assets show a **Retry** control after the host supplies them. Removing/undoing an attachment changes references, not stored bytes; the host owns retention and garbage collection, including assets still referenced by saved documents or undo history.

## Embed

Call `gpui_kit::init(cx)` and then `gpui_kit_block_editor::init(cx)` once at startup. Retain the editor entity:

```rust
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, Styled, Window};
use gpui_kit_block_editor::{
    BlockEditor, BlockEditorView, BlockSpec, Document, ExtensionRegistry, RichDocument, SectionKind,
};

struct Notes {
    editor: Entity<BlockEditor>,
}

impl Notes {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut document = Document::new();
        document.insert_block(None, None, &BlockSpec {
            kind: SectionKind::Text,
            content: RichDocument::paragraph("Start writing."),
            children: vec![],
        }).unwrap();
        document.clear_history();
        let editor = cx.new(|cx| {
            BlockEditor::new(document, ExtensionRegistry::default(), window, cx)
        });
        Self { editor }
    }
}

impl Render for Notes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        BlockEditorView::new(&self.editor).size_full()
    }
}
```

Mount inside a sized GPUI layout using `gpui_kit::open_window`. The component owns its scroll container and uses the host theme; `examples/demo.rs` shows subscriptions and peer routing.

`sections()` returns the projected tree in preorder, including structural groups and folded descendants. `children(parent)` returns direct siblings. `set_content(id, &RichDocument)` applies semantic changes. `move_section()` strictly reorders siblings; `reparent_section()`, `indent_section()`, and `outdent_section()` change nesting. `move_sections(ids, parent, before)` validates a batch before writing, filters duplicates/covered descendants, converts ordinary text/list roles, and groups movement plus empty-container cleanup into one undo step.

### Styling

`BlockEditorView` implements GPUI's `Styled` trait for its outer layout: size, margin, padding, background, borders, and inherited typography. These refinements belong to that mount. Internal presentation belongs to the retained editor: configure `BlockEditor::with_style(EditorStyle)` at construction, or call `set_style(style, cx)` at runtime. `style()` reads the current configuration.

```rust
use gpui_kit::{StyleRefinement, Styled, px};
use gpui_kit_block_editor::{EditorStyle, EditorTextStyle};

let mut text = EditorTextStyle::default();
text.body = StyleRefinement::default()
    .font_family("Georgia")
    .text_size(px(17.));
text.headings[0] = StyleRefinement::default().text_size(px(32.));
text.code = StyleRefinement::default()
    .font_family("Menlo")
    .text_size(px(14.));

let appearance = EditorStyle {
    document: StyleRefinement::default().p(px(24.)),
    content: StyleRefinement::default().max_w(px(960.)).gap(px(8.)),
    controls: StyleRefinement::default().size(px(28.)),
    drag_handle: StyleRefinement::default().w(px(28.)).h(px(32.)),
    popup_menu: StyleRefinement::default().min_w(px(220.)).max_h(px(240.)),
    ..Default::default()
}.with_text(text);

editor.update(cx, |editor, cx| editor.set_style(appearance, cx));
```

Unspecified properties follow the current application theme and existing defaults. Overrides refine those defaults rather than replacing an entire theme. `EditorStyle::default()` clears all local overrides. Instances can use different appearances while editing replicas of the same document; `examples/demo.rs` gives the left pane local prose typography and leaves the right pane on theme defaults.

| Configuration | Surface |
| --- | --- |
| `document`, `content` | Scroll viewport, readable width, document padding, block spacing |
| `block`, `selected_block` | Block surfaces and outer block selection |
| `text` | Body, six heading levels, independent code font, quotes, tables/header/cells, caret and text-selection colors |
| `controls`, `controls_hover`, `drag_handle`, `drag_handle_hover` | Chrome sizing, typography, and hover appearance |
| `slash_menu*`, `popup_menu` | Command menu/group/row styles and style/list/language popup surfaces |
| `selection_toolbar`, `link_editor` | Floating formatting controls and URL editor |
| `media`, `diagram` | Attachment cards/images and inline diagram previews |
| `diagram_viewer`, `diagram_toolbar`, `diagram_canvas` | Expanded dialog dimensions/chrome, toolbar, and pan/zoom viewport |

Prose typography does not replace the code editor's monospace defaults; use `text.code` for code-specific overrides. Syntax-token colors and popup checked/disabled/hover state colors continue following the GPUI theme. Code IME underlines follow the code foreground.

Styling is local UI state: it does not write document JSON or CRDT updates, create undo entries, reset code input, or replace attachments. Open menus retain their focus/highlight/scroll; expanded diagrams retain their zoom/pan when restyled. Document formatting commands remain separate from application presentation.

## Import and export

### Boundaries

- `insert_section(before, markdown)` imports Markdown and returns its first editable block ID; `set_markdown(id, markdown)` is an explicit body replacement/import, not the native editing path.
- `markdown()` exports the current tree. Collapsibles use `<details>/<summary>` with metadata for our own roundtrip, including empty children. Other Markdown consumers may ignore that metadata or HTML. Markdown formatting/whitespace and reference spelling are canonicalized according to the renderer; exact original source bytes are not retained as the document.
- `export_json()` / `Document::import_json(json)` provide versioned semantic interchange with block/node IDs and hierarchy. JSON imports start with fresh Yjs identities and empty local history; they are not collaborative snapshots.
- Persist `encode_state()` and restore with `apply_update()` to retain CRDT identity. Restore a shared document once and bootstrap replicas from it; independently importing identical content creates independent identities.

### Format marker

`RICH_DOCUMENT_VERSION` defines the current structured format, starting at **1**. Native documents, JSON interchange, and CRDT metadata use that same constant. Unsupported versions are rejected before changing a live document.

The marker is retained for future breaking changes; there is no migration layer or compatibility promise for earlier unreleased development snapshots.

## Extensions

Configure `ExtensionRegistry` before constructing an editor. Commands retain stable IDs and registration order; duplicate IDs are errors.

```rust
use gpui_kit::assets::IconName;
use gpui_kit_block_editor::{CommandAction, ExtensionRegistry, SlashCommand};

let mut extensions = ExtensionRegistry::default();
extensions.commands.register(SlashCommand {
    id: "callout".into(),
    title: "Callout".into(),
    group: "Text".into(),
    icon: IconName::MessageSquareText,
    description: "Insert an editorial note".into(),
    keywords: vec!["note".into()],
    action: CommandAction::InsertMarkdown("> **Note**\n> Shared context.".into()),
})?;
```

`CommandAction::Editor` runs a native formatting command; list/quote/nesting operations update the shared outer tree. `InsertMarkdown` is an explicit template-import boundary. `CommandAction::Collapsible` uses the same command registry as built-ins.

`EditorCommand`, `BlockKind`, `MarkKind`, `MarkChange`, `MarkState`, and `TableEdit` are available from `gpui_kit_block_editor`; applications do not need a separate editing-backend dependency.

Implement `PreviewExtension` with `language()` and `render_svg(source)`, then `register_preview()`. Previews receive a semantic code block's plain contents, not fenced Markdown. Rendering runs on the background executor; superseded results are discarded. Duplicate languages are rejected case-insensitively.

Native custom atom payloads support unit, JSON, and strings. Additional serializable payload types can register a named serde codec through `register_custom_codec`; unknown payloads fail explicitly rather than falling back to incorrect content.

## Collaboration contract

### Shared schema

```text
Y.Doc (UTF-16 text indices)
├── meta: Y.Map { schema: "gpui-kit-block-editor", version: 1 }
└── sections: Y.Map<SectionId, Y.Map>
    ├── kind: string
    ├── position: root-rank or parent-ID|rank
    └── body: Y.Map
        └── nodes: Y.Map<NodeId, Y.Map>
            ├── kind: semantic node kind
            ├── position: root-rank or parent-ID|rank
            ├── attrs: Y.Map (independent metadata keys)
            └── text: attributed Y.Text
```

Text edits apply Unicode-safe disjoint splices, retaining unchanged CRDT items. Formatting changes use independent attributes, so concurrent bold and italic changes merge without replacing text. Inline atoms use the native object-replacement character plus atom metadata. Structural nodes also retain empty text types for stable selection anchors.

Parent/rank is one atomic register; concurrent moves resolve through Y.Map conflict resolution. Dense hexadecimal ranks include UUID discriminators. Missing parents hide orphan descendants; concurrent section cycles project deterministically with the lexicographically smallest cycle ID at the root. Projection never rewrites shared state. Editable list roles follow the surviving parent, including concurrent checkbox/move conflicts.

Client ID zero is reserved for the common schema seed. Every collaborating implementation must use the same schema/bootstrap, not merely understand Yjs updates.

### Concurrent semantic edits

Code blocks project as plain text even when a peer concurrently adds inline formatting or atoms. Reading or receiving that state does not rewrite the attributed Y.Text; undoing the code conversion can reveal the peer's formatting and atom payloads again. Mark values and atom payloads are validated before projection. Adjacent identical atoms remain separate rich runs even when Yjs coalesces their attributed spans.

Concurrent table row/column edits can leave ragged or empty grids. Every surviving cell and its ID is retained; native rendering/navigation handle missing slots, and Markdown export pads them without truncating wider rows. Alignment presentation follows the visible width. No synthetic collaborative cells or automatic repair transactions are created. A table without any text caret remains a selectable block: explicit row/column commands create real cells, and deleting the table restores an editable paragraph.

Malformed semantic values and unsupported versions still reject before changing the live document. JSON imports validate ancestor chains with checked lookups, so missing ancestors and cycles return errors regardless of section order.

### Transport

`state_vector()`, `encode_update(remote_state_vector)`, `encode_state()`, and `apply_update(bytes)` use raw **Yjs v1 binary encoding**. The binary encoding and document-format marker are independent versions, even though both currently equal 1. Duplicate/out-of-order updates are supported.

Local GUI changes emit `DocumentEvent::Changed { update }`. Send those raw bytes to peers and call the receiving editor's `apply_update(update, window, cx)`. Remote application never echoes a local change event.

`start_sync()` and `receive_sync()` use the y-websocket-style outer envelope: varuint `0`, sync subtype (`0` step 1, `1` step 2, `2` update), then a length-prefixed payload. Both peers send `start_sync()`; route returned responses back to the sender. Do not mix framed messages with raw update APIs. Truncated/unsupported frames are errors.

### Selections and undo

`Document::relative_position(section_id, node_id, utf8_byte_offset, yrs::Assoc)` creates an anchor into semantic text. `resolve_position()` returns `(section_id, node_id, utf8_byte_offset)`. The underlying indices use UTF-16; native selections use UTF-8/grapheme boundaries. Deleted/replaced types never resolve against an unrelated text object or resurrect content. Atomic before/after sides are preserved independently of their empty text anchors.

Remote reconciliation rebuilds affected native models directly from rich nodes, restores both selection endpoints through stable IDs/relative positions, and uses a local-origin Yrs undo manager instead of native history. Reversed selections and table/block ranges are retained where their structure survives; deletion recovers to a surviving boundary.

Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z, or `BlockEditor::undo` / `redo`, emit ordinary updates. Typing coalesces within 500ms; navigation, structural operations, and commands establish boundaries. Slash commands and IME composition form logical undo groups. Peer edits do not clear local history or undo peers' work. Snapshots persist content/CRDT identity, not local undo history.

Hosts can use `stop_capturing()`, balanced `begin_history_group()` / `end_history_group()`, and `clear_history()` after seeding. Remote writes received during active IME are queued until composition ends; sync-step replies remain available while writes wait.

### Host responsibilities and limits

- Network transport, room/version admission, authentication, size limits, durable storage, and awareness/presence belong to the host.
- Native structural convergence does not guarantee preservation of every concurrent editing intention, especially conflicting table/structure operations.
- Blocks remain content-sized and retained; this wrapper does not virtualize a large number of blocks.
- Expansion and preview modes are local view state, not shared-document persistence.

## Verification

Native headless GPUI tests exercise typed editing, literal Markdown text, rich/empty block splitting, table/code editing, list and multi-block dragging, hover controls, collapsible creation/focus/local folding/subtree deletion, hidden peer edits, nested Mermaid switching, relative selections, peer-safe undo, and IME delivery.

Model tests cover independent formatting/text merges, real shared-text identity, semantic tree ordering, unsupported-version rejection without partial changes, JSON boundaries, conflicts, and sync framing. Markdown boundary tests cover semantic import/export, definitions, whitespace normalization, lists, quotes, and collapsible roundtrips.

## License and attribution

This crate is licensed under Apache-2.0. The incorporated native editing engine retains its upstream attribution and license in `NOTICE` and `LICENSE-APACHE`; modified source files identify the integration changes. The unused upstream standalone wrapper and formatting toolbar are not included. The retained native engine tests run as part of `cargo test --lib`.
