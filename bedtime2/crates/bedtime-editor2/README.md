# Bedtime text editor

Embeddable plain text editing with a persistent UTF-8 buffer, viewport-only
rendering, independent views, shared history and bed1's animated caret. No LSP,
syntax, structure or debugging dependency is installed. The default build has no
Workbench dependency; the optional `workbench` feature supplies an adapter to
Workbench's existing document services.

```sh
cargo run --offline --locked -p bedtime-editor2 --example standalone
```

The example shows two views of one in-memory document without Workbench. bEd2
uses the same editor for files opened from Files, with Save, Split view and a
per-view Line numbers toggle. Edited previews become pinned tabs.

## Ownership and execution

| State | Owner |
| --- | --- |
| Text, revision, undo/redo and change journal | `TextSession` implementation |
| Persistent indexed UTF-8 content | `TextBuffer` |
| Selection, composition, navigation and preferred column | `EditorView` |
| Scroll presentation and input focus | Stable per-view egui IDs |
| Caret visual history | `EditorView` |
| Files, source identity, saves and conflict policy | Application |

`EditorView::show` is an ordinary sequential drawing call. It reads a cheap
snapshot, maps positions through accepted changes, processes focused input in
event order, commits explicit edits and paints visible rows. There is no widget
undo history, second canonical edit buffer, background editing system or hidden
text mutation in a getter. Every accepted edit goes through `TextSession::edit`.

`TextDocument` is the standalone in-memory implementation. Multiple views borrow
the same owner. With `workbench`, `TextBuffer` implements `DocumentModel` using
`BufferEdit`, and `bind_text_editor` attaches a `WorkbenchEditor` to an existing
document panel. `draw_editor` connects drawing to that document's canonical
revision/history; it installs no panel, window, renderer or file policy itself.
Closing a Workbench viewer retains its document under the existing contract.

## Positions and changes

- Stored positions are UTF-8 byte offsets. `TextSelection` validates both ends
  against a snapshot. Anchor and head preserve selection direction.
- Screen rows, display cells and glyph positions are presentation coordinates.
  The same line layout supplies text, hit testing, selections and carets.
- Grapheme-aware left/right and deletion keep combining characters, emoji and
  CRLF together, including across rope leaves. Vertical movement retains a
  preferred display column. Initial tab display width is four cells.
- A `TextEdit` replaces a byte range at an expected revision. Rejected edits
  change neither canonical text nor history. `BufferEdit` retains shared roots
  for exact, cheap undo/redo; inverse restoration is conditional on root identity.
- Journal changes use successive pre-change states. Workbench's independent,
  bounded `document_changes_since` journal includes transactions, undo, redo and
  replacement invalidations. Its retention does not depend on undo retention.
- A journal gap or whole-model replacement invalidates positional knowledge.
  Views clamp existing positions to valid boundaries; they do not diff an entire
  large document on the UI thread. Plugins must refresh derived semantic data.
- Consecutive typing groups end on navigation, view ownership changes, focus
  loss or 650 ms of idle time, as well as document-service history/save barriers.

Only explicit copy or file I/O materializes complete selected/source bytes.
Document snapshots and history share immutable roots. Views retain an observed
revision rather than another text buffer, so dormant views do not pin old text.

## Large files

The target is approximately 100 MiB and one million lines. `TextBuffer` is an AVL
rope with bounded UTF-8 leaves, cached line starts, line/cell summaries and
composable content fingerprints. Editing copies boundary leaves and tree paths;
line navigation uses the index. Hash inequality is a fast equality rejection;
equal fingerprints still require exact content equality unless roots are shared.

Drawing shapes only visible line fragments, both vertically and horizontally.
Even a single multi-megabyte line does not create a full-line galley. Each fragment
also has a 32 KiB shaping ceiling for pathological zero-width text. The canonical
document is never truncated. `EditorOutput.rendered_lines` and `laid_out_bytes`
expose the actual work for verification.

bEd2 loads/builds text on managed workers and currently accepts UTF-8 text files
up to 256 MiB; larger, binary and nonregular files show metadata. Equal reloads
reuse known roots after worker-side comparison. Saves stream rope chunks through
the application's atomic replacement path. Inspection returns at most 16 KiB of
text plus complete byte/line counts, rather than serializing the whole file.

## Composition and external control

IME preedit is local draft state. Commit creates one canonical transaction;
Escape, cancellation or focus loss abandons preedit. Workbench drafts block
save, undo/redo, rebinding and controlled retirement. Another editor cannot
commit over a draft. If an external source changes the canonical revision during
composition, the draft is retained until explicitly discarded.

`Navigation` identifies a revision and reveal range, with optional selection,
minimal/centered reveal and explicit focus. `request_navigation` works on an
embedded view; `navigate_editor` targets a Workbench panel and selects/focuses
that panel only when requested. Reveal without selection preserves the caret.
Stale or invalid requests fail before changing view state. Applications choose
which document/view to open before requesting navigation.

The caret's logical position updates immediately. Its visual leading/trailing
edges use bed1's 35/110 ms cubic easing and a bounded fading trail. Content
coordinates prevent scrolling from becoming caret motion. Layout-only changes
settle the caret, and `MotionPolicy.enabled = false` disables movement animation.

## Optional code capabilities

The editor remains usable without language tools. Future providers will supply
revision-bound data and explicit actions: syntax spans, diagnostics, fold ranges,
gutter markers, hover content and edit/navigation requests. Providers will own
their data and cleanup; the editor will own placement, clipping and interaction.
Line-number visibility must remain independent of other gutter contributions.
Debug execution locations and breakpoints must remain independent of selections.
No provider registration framework is implemented in this slice.

Indentation-aware soft wrapping, smart indentation/backspace, find/replace,
folding, multicursor interaction and language-tool contributions remain later
milestones. Ordinary typing, selection/drag, word/line/document/page navigation,
clipboard, read-only presentation and history are implemented here.

## Verification

```sh
cargo test --offline --locked -p bedtime-editor2 --features workbench
cargo test --offline --locked -p bedtime-editor2 --features workbench million_line_editing -- --ignored --nocapture
cargo check --offline --locked -p bedtime-editor2 --no-default-features --example standalone
cargo test --offline --locked -p bedtime-workbench2 --no-default-features
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_editor.py
```

The explicit large fixture covers 112 MB and one million lines, bounded drawing,
navigation near EOF, editing and shared-root undo. The native smoke uses temporary
small/large files and a memory clipboard, exercises real inputs and saves, and
retains screenshots and a summary under `target/editor-smoke`.

Verified on 2026-10-18 on an Apple M4 Pro in an unoptimized development build:
the 112,000,000-byte fixture loaded/indexed in 5.55 s; headless egui editor frames
had a 0.47 ms idle median, 0.83 ms maximum, 0.87 ms edit and 0.49 ms undo. These
measure editor CPU work, not total native application/GPU frame time. The native
smoke rendered 23 rows and shaped 506 bytes near EOF while exercising editing,
shared history and saving the complete file. Measurements are local evidence,
not performance guarantees for other machines or file contents.
