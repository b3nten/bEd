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
per-view Line numbers toggle. The **Editing** menu exposes wrapping, indentation
and Find/Replace. Edited previews become pinned tabs.

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
Document snapshots and history share immutable roots. Unwrapped views retain an observed
revision rather than another text buffer. A prepared wrap index retains its
snapshot so journal changes can update the affected line metadata.

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

Multicursor interaction and language-tool contributions remain later milestones.
Folding is deliberately deferred until Tree-sitter integration; no manual or
heuristic folding is installed.

## Everyday editing

The **Editing** menu is optional host UI (`EditorView::editing_menu`); keyboard
editing and Find/Replace also work when embedded without that menu. Settings and
search belong to the view, independently of sibling views and document history.

- **Wrap lines** uses viewport-width soft rows with continuation indentation
  matching the line's leading whitespace, capped at half the row width. Breaks
  use display cells and snap to complete graphemes, without syntax assumptions.
  Numbers appear only on the first visual row. Pointer hits, selections, arrow/
  page movement, Home/End, reveal and caret painting use the same row mapping;
  caret affinity distinguishes the two sides of a soft row boundary.
- **Smart indentation** preserves leading spaces/tabs and LF/CRLF/CR on Enter,
  removes whitespace around the split, and adds one indentation unit after an
  opening brace/bracket/parenthesis. Between a matching pair, Enter inserts a
  separate closing line. Typing a closing delimiter in leading whitespace
  outdents one level. These are local character rules, not language parsing.
- **Tab** inserts tabs or spaces to the next indentation stop. Tab/Shift+Tab on
  a selection indent/outdent whole lines, preserve selection direction and omit
  an exclusive final line. Backspace steps through leading whitespace by stops,
  respecting mixed spaces/tabs; Home toggles between indentation and row start.
  Choose tabs or 2/4/8 spaces; existing whitespace is preserved. Tabs still
  display at four-cell stops. Smart indentation can be disabled.
- **Cmd/Ctrl+F** opens Find. **Cmd+Option+F** or **Ctrl+H** opens Replace;
  the menu also exposes both. Enter/Shift+Enter, F3/Shift+F3 and Cmd/Ctrl+G with
  optional Shift navigate forward/backward with wraparound. Escape closes Find
  and returns focus to the editor. Selection seeds the query when at most 4 KiB.
- Search is literal, with Unicode lowercase matching, Match case and Whole word
  options. Matches can span rope leaves and include line endings; no regex or
  Unicode normalization is applied. Queries accept at most 4,096 characters.
  Replace next changes an observed selected match, then finds the next one.
  Replace all is one atomic, undoable replacement; replacement text is literal.
  Read-only views can search but cannot replace.

Wrap metadata is prepared in cancellable view-owned workers. While a new width
or bulk edit is being indexed, the view consistently uses ordinary unwrapped
coordinates and displays a preparation indicator. A ready index updates only
changed logical lines using shared 256-line blocks and block prefix totals.
Viewport layout remains bounded, including for a single very long wrapped line.
Widths/fonts can differ between sibling views.

Search scans immutable snapshots in cancellable workers. Count/navigation results
are revision-bound; edits invalidate them. Only 10,000 match ranges are cached,
plus bounded source-region summaries for exact visible highlighting. Navigation
beyond the cache requests another anchored observation, so all matches remain
reachable without storing a range for every character. Replace all streams every
match into a new rope in a worker rather than using that limited cache, with a
256 MiB result limit. Indentation spans above 256 KiB are prepared off-thread too.
Workers never mutate documents. Explicit acceptance checks revision, read-only
and draft gates; stale or cancelled preparation changes no text or history.
Closing Find cancels its work; dropping a view cancels all its workers.

## Verification

```sh
cargo test --offline --locked -p bedtime-editor2 --features workbench
cargo test --offline --locked -p bedtime-editor2 --features workbench million_line_ -- --ignored --nocapture
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
all 34 editor tests, including both explicit million-line fixtures, passed; the
40 bEd2 application tests and the standalone no-Workbench build also passed.
The 112,000,000-byte unwrapped fixture loaded/indexed in 5.90 s, with a 0.73 ms
idle median, 1.14 ms maximum and 1.14 ms edit. The wrapped fixture prepared its
index in 1.32 s, edited in 1.13 ms and searched asynchronously in 1.27 s, rendering
17 rows / 889 bytes near EOF. These measure headless editor CPU work, not total
native application/GPU frame time. The expanded native smoke verifies wrapping,
Find reopening, replace-one/all, shared undo/redo, smart whitespace and real saves.
Its final pinned-executable run passed with 21 rendered rows / 460 shaped bytes
near EOF; three captures and `summary.json` are in `target/editor-everyday-smoke`.
Measurements are local evidence, not guarantees for other machines or contents.
