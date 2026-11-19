# GPUI Kit Block Editor

A native, embeddable nested-block Markdown editor for [GPUI Kit](https://gpui-kit.com/). Rich-text editing uses [gpui-kit-markdown-editor](https://github.com/wabzqem/gpui-kit-markdown-editor); collaboration uses [Yrs](https://github.com/y-crdt/y-crdt), the Rust implementation of the Yjs CRDT.

## Run

```sh
cargo run --example demo
cargo test --lib
```

The project pins Rust 1.100.0 in `rust-toolchain.toml`. With rustup installed, Cargo selects it automatically. Native builds need GPUI Kit's [platform prerequisites](https://gpui-kit.com/docs/installation); macOS needs Xcode and its command-line tools. `Cargo.lock` records compatible upstream backend versions.

The demo connects **two independent CRDT replicas in-process**. Edit either pane, pause the connection, edit both independently, then reconnect to merge. This is a transport demonstration, not a hosted multiplayer service. Copy Markdown exports the left document.

## Editing

- Each ordinary paragraph line is its own borderless block with a permanent ID. Enter/Shift+Enter creates a sibling and focuses it; splitting preserves inline formatting. Code fences, tables, and explicit Markdown hard breaks retain their internal lines.
- **Tab / Shift+Tab** nest and unnest blocks when the slash menu is closed. Nesting moves the entire subtree. Enter on an empty list item exits its list; new task items start unchecked.
- Hover a block to reveal its vertically centered **⠿** drag handle and trash icon in the side gutters. Drop onto another row to move before it, including across list groups; a list's bottom strip appends to that list. The narrow strips before/after lists move items to the parent level, and the document's bottom area moves them to the top level. Items leaving lists become ordinary blocks; ordinary blocks entering lists become items. Nested children travel with their parent, and numbered lists renumber automatically. Cross-editor drops and moves into a selected block's own subtree are rejected.
- Click a grip to select a block. **Shift-click** selects a visible range; **Cmd/Ctrl-click** toggles individual blocks; **Escape** clears block selection. Drag any selected grip to move the selection in document order. Successful drops clear the selection and its range anchor. A multi-block selection covering every item of a list moves the original list container, preserving its type and starting number. **Alt/Option-click** a grip explicitly selects its entire containing list, including a one-item list. Selected blocks include their nested children; selecting a parent and child never moves the child twice.
- **Add block** creates and focuses a new top-level block; the trash icon deletes a block and its children.
- Type **/** at the start of a word to search commands. **↑ / ↓**, **Enter / Tab**, and **Escape** navigate, accept, and dismiss the menu. Clicking a command also accepts it. IME composition is not intercepted.
- Commands include paragraphs, heading sizes 1–3, bold, italic, strikethrough, inline/source code, quotes, bulleted/numbered/task lists, table insertion, and adding/removing table rows and columns. Table edits operate on the cell containing the caret.
- Heading levels provide Markdown-compatible font sizes; arbitrary point sizes/font families are not part of standard Markdown.
- Native editing supplies selection, clipboard, IME, Markdown input rules, and formatting shortcuts. Cmd/Ctrl+B and Cmd/Ctrl+I apply inline styling; Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z undo/redo this replica's edits across sections without undoing peers' changes.
- **/mermaid** inserts a diagram section in rendered mode. Hover to reveal the source/preview toggle immediately left of the trash icon: the terminal icon opens editable source, and the eye icon returns to the diagram. Only one view is shown at a time. Source edits refresh the native SVG in the background; rendering errors remain switchable back to source. The view choice is local to each editor, survives remote edits, and does not change the CRDT document. Registered preview extensions use the same toggle.

## Embed

Call `gpui_kit::init(cx)` followed by `gpui_kit_block_editor::init(cx)` once at application startup. Keep the editor entity, rather than recreating it during rendering:

```rust
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, Window};
use gpui_kit_block_editor::{BlockEditor, Document, ExtensionRegistry};

struct Notes {
    editor: Entity<BlockEditor>,
}

impl Notes {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut document = Document::new();
        document.insert_section(None, "# Notes\n\nStart writing.").unwrap();
        document.clear_history(); // Initial content is not a user undo step.
        let editor = cx.new(|cx| {
            BlockEditor::new(document, ExtensionRegistry::default(), window, cx)
        });
        Self { editor }
    }
}

impl Render for Notes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.editor.clone()
    }
}
```

Mount this inside a sized GPUI layout using `gpui_kit::open_window`. The component owns its scroll container and uses the host's theme. See `examples/demo.rs` for a complete application and subscriptions.

`Document::new()` starts empty. `insert_section(before, markdown)` imports ordinary paragraph lines as separate blocks and lists/quotes as nested groups; it returns the first editable block's ID. `sections()` returns the visible tree in preorder, including noneditable groups, with `parent`, `kind`, and `depth`; `children(parent)` returns direct siblings. `BlockSpec` and `SectionKind` describe subtrees for `insert_block()`. `move_section()` strictly reorders siblings; `reparent_section()`, `indent_section()`, and `outdent_section()` change nesting without replacing text identities. `move_sections(ids, parent, before)` moves a batch in document order, filters duplicates/covered descendants, converts editable list roles, and validates the complete batch before writing. Containers cannot be placed directly inside a list. Batch movement and empty-container cleanup form one undo step.

Markdown export reconstructs lists, task markers, numbering, and quotes. Markdown alone cannot retain IDs or arbitrary plain-block parenting. Persist `Document::encode_state()` and restore it with `apply_update()` into an empty replica to retain exact collaboration identity and hierarchy.

## Extensions

Configure the registry before constructing an editor. Commands and previews are independent; a feature can register either or both.

```rust
use gpui_kit_block_editor::{CommandAction, ExtensionRegistry, SlashCommand};

let mut extensions = ExtensionRegistry::default();
extensions.commands.register(SlashCommand {
    id: "callout".into(),
    title: "Callout".into(),
    description: "Insert an editorial note".into(),
    keywords: vec!["note".into()],
    action: CommandAction::InsertMarkdown(
        "> **Note**\n> Add context for your collaborators.".into(),
    ),
})?;
```

`CommandAction::Editor` runs a native `markdown_editor::EditorCommand` at the caret; list/quote and nesting commands update the shared block tree. `InsertMarkdown` imports new sibling blocks after the current one, preserving structured Markdown rather than inserting literal text. Within a list, ordinary inserted text becomes sibling list items. Duplicate command IDs return errors; search covers IDs, titles, descriptions, and keywords.

Implement `PreviewExtension` (`Send + Sync`) with `language()` and `render_svg(source)`, then call `register_preview()`. It receives fence contents and returns SVG or an error. Rendering runs on the background executor; superseded results are discarded. Mermaid uses this same public registration path, not a special-case UI renderer. Duplicate fence languages are rejected case-insensitively.

Previews support standalone backtick/tilde fences, including unclosed fences while typing. The first registered-language fence in an editable block is rendered, including a standalone fence imported beneath a list item or quote. Mermaid rendering uses [mermaid-rs-renderer](https://github.com/1jehuang/mermaid-rs-renderer), without Node, a browser, or Mermaid CLI; syntax and visual fidelity follow that renderer, not complete Mermaid.js parity.

## Collaboration contract

### Shared schema

```text
Y.Doc (UTF-16 text offsets)
└── sections: Y.Map
    └── <UUID>: Y.Map
        ├── source: Y.Text      # editable Markdown body; containers have no text
        ├── kind: string       # text, quote, list, item, or checked task kind
        └── position: string   # root rank or parent-UUID|rank, one atomic register
```

Local source changes apply Unicode-safe disjoint character splices to `Y.Text`, preserving unchanged CRDT items even between formatting delimiters. Moves update the atomic parent/rank register and, when crossing a list boundary, the editable kind; IDs, text identities, and children remain intact. Concurrent moves resolve through Y.Map conflict resolution. Editable list roles are projected from the surviving parent, so concurrent moves or checkbox edits cannot leave a paragraph directly in a list or an item outside one. Dense hexadecimal ranks contain UUID discriminators so concurrent insertions remain orderable. Missing parents hide orphan descendants; cycles project deterministically with the lexicographically smallest cycle ID at the root. Projection never rewrites shared state. Deleting a subtree does not resurrect concurrently edited descendants.

Kinds are encoded as `text`, `quote`, `item`, `task:checked`, `task:unchecked`, `list:bullet:<start>`, or `list:ordered:<start>`. Lists contain item nodes; an item owns its body text and any nested lists/blocks. Existing flat snapshots without `kind` remain text blocks. Use the hierarchy-aware schema in every collaborating client.

Separate peer initialization from creating content: initialize the shared document once, then sync replicas. Independently seeding identical Markdown creates independent sections with different IDs, as it should.

### Raw Yjs v1 updates

- `state_vector()` → encoded Yjs state vector.
- `encode_update(remote_state_vector)` → update containing missing changes and deletions.
- `encode_state()` → full encoded update for persistence/bootstrap.
- `apply_update(bytes)` → apply an update, including duplicate or out-of-order delivery.

The GUI emits `DocumentEvent::Changed { update }` for local transactions. Send these **raw** bytes to connected replicas and call the receiving editor's `apply_update(update, window, cx)`. Remote application does not emit a local change event, avoiding echo loops. The demo implements this routing with retained GPUI subscriptions.

### Sync handshake

`start_sync()` and `receive_sync()` use the **y-websocket-style outer sync envelope**: varuint `0`, sync subtype (`0` step 1, `1` step 2, `2` update), then a length-prefixed payload. Both peers send `start_sync()` when connecting; pass each incoming frame to `receive_sync()` and send every returned response back to its sender. Concatenated sync frames are accepted. Empty, truncated, unsupported, or trailing-invalid payloads return errors.

Do not pass raw update bytes to `receive_sync()` or framed messages to `apply_update()`. A transport using bare `y-protocols/sync` messages must add/remove the outer `0` envelope. In JavaScript, the raw APIs correspond to `Y.encodeStateVector`, `Y.encodeStateAsUpdate`, and `Y.applyUpdate`.

### Relative selections and collaborative undo

GPUI is not the limiting layer: the upstream Markdown editor's `set_source` replaces its native document and clears its internal selection/history. The adapter now restores selections through Yrs relative positions and owns a separate, local-origin Yrs undo manager; it does not rely on the native undo stack.

- Both selection endpoints are anchored to CRDT items, not stale leaf IDs or numeric offsets. A source/model-generation cache maps native rich-text positions to Markdown AST spans and back, including marks, lists, tables, code, Unicode, and reversed selections. Deleted content collapses to a surviving editable boundary; deleting the focused section moves focus to a neighboring section (or the empty document).
- `Document::relative_position(section_id, source_byte_offset, yrs::Assoc)` creates an anchor. `resolve_position(&anchor)` returns its current section ID and UTF-8 source offset. The underlying text uses Yjs-compatible UTF-16 indices. Missing/deleted sections and replaced Y.Text identities resolve to `None`, never recreate content.
- `BlockEditor::undo(window, cx)` / `redo(window, cx)` and the keyboard shortcuts emit ordinary `DocumentEvent::Changed` updates. Peers receive and apply those changes normally. Headless `Document` also exposes `undo`, `redo`, `can_undo`, and `can_redo`. History is per replica, not a shared global undo stack.
- Typing in one block coalesces within 500ms. Navigation, block changes, commands, insertions, moves, and deletions establish capture boundaries; slash operations, block splits, nesting changes, and IME composition form logical undo groups. A peer edit does not clear local history or redo. Moving an item retains relative text anchors; splitting retains the original ID/Y.Text on the first editable piece and focuses the newly created piece. If a peer has entirely overwritten an operation, Yrs may skip it rather than undo the peer's replacement.
- Hosts can use `stop_capturing()` to establish a boundary, balanced `begin_history_group()` / `end_history_group()` calls for a compound command, and `clear_history()` after seeding initial content. Document snapshots persist content/CRDT identity, not local undo history.
- Remote updates received during active IME composition are queued until composition ends, without replacing marked text. Sync-step replies remain available while updates wait. Accepted queued updates are reflected in the native editor after commit, not immediately within `apply_update`.

### Boundaries

- Network transport, persistence policy, authentication/authorization, room membership, and awareness/presence are host responsibilities. Never accept untrusted updates without transport-level admission and size limits.
- Collaboration shares the **block tree and Markdown source characters**, not an inline rich-text AST. Concurrent edits to Markdown delimiters or table structure can converge to syntactically undesirable Markdown; convergence does not imply semantic intent preservation.
- Blocks are content-sized and retained; this wrapper does not virtualize a large number of blocks.

## Verification

Headless native GPUI tests exercise rich-text and empty-block splits, nested list/task editing, cross-group and parent-level drag/drop, modifier-based multi-selection, whole-list movement, peer-safe batch undo, cycle rejection, numbering, code/table newlines, slash commands, Mermaid insertion, hover controls, source/diagram switching, relative selections, deletion recovery, and IME-safe remote delivery. Model tests cover batch validation/ordering, concurrent list-role changes, nested Markdown import/export, stable subtree/text identity, concurrent reparenting and cycles, Unicode anchors, local-origin history, structural undo, delete/edit conflicts, update delivery, and sync framing. Source-map tests cover rich Markdown positions and selection variants; extension tests cover custom commands/renderers, fences, duplicate registration, and Mermaid success/error output.
