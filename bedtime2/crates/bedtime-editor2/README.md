# Bedtime text editor

Embeddable plain text editing with a persistent UTF-8 buffer, viewport-only
rendering, independent views, shared history and bed1's animated caret. Default
builds require neither Workbench nor language tooling. Independent optional
`workbench` and `treesitter` features add canonical document integration and
shared syntax/fold contributions; no LSP or debugger is installed.

```sh
cargo run --offline --locked -p bedtime-editor2 --example standalone
```

The example shows two views of one in-memory document without Workbench, with
per-view line numbers and an **Editing** menu for wrapping, indentation and
Find/Replace. bEd2 uses the editor for files opened through Files; local file
policy belongs to the embedding application.

## Ownership and execution

| State | Owner |
| --- | --- |
| Text, revision, undo/redo and change journal | `TextSession` implementation |
| Persistent indexed UTF-8 content | `TextBuffer` |
| Selection, composition, navigation, collapsed regions and preferred column | `EditorView` |
| Optional language and shared analysis | Host session / Workbench document |
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

Ordinary UI editing and layout never materialize complete source bytes. Copy
materializes the selection; optional syntax analysis materializes bounded source
snapshots in its worker, and hosts own file I/O.
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

The optional syntax worker accepts sources up to 8 MiB; larger documents retain
plain editing without semantic observations. This is an analysis limit, not an
editor or file-size limit. bEd2 loads file-backed Code documents on managed
workers (UTF-8 up to 256 MiB), supplies checked saves and source reconciliation,
and creates no sample document in an empty editor. Those policies remain outside
the editor crate.

## Composition and external control

IME preedit is local draft state. Commit creates one canonical transaction;
Escape, cancellation or focus loss abandons preedit. Workbench drafts block
save, undo/redo, rebinding and controlled retirement. Another editor cannot
commit over a draft. If an external source changes the canonical revision during
composition, the draft is retained until explicitly discarded.

`Navigation` identifies a revision and reveal range, with optional selection,
minimal/centered reveal and explicit focus. `request_navigation` works on an
embedded view; `navigate_editor` targets a Workbench panel and selects/focuses
that panel and activates its existing layout only when requested.
`Navigation::scroll_to_line`, `EditorView::scroll_to_line` and
`workbench::scroll_to_line` resolve zero-based logical lines through this same
revision-bound path, with or without Tree-sitter. Reveal without selection
preserves the caret and expands enclosing folds.
Stale or invalid requests fail before changing view state. Applications choose
which document/view to open before requesting navigation.

Each wheel input locks to its dominant axis, and the next input can immediately
choose another axis. egui's smoothed tail retains the latest input's axis. Scrollbars appear
only for overflow and remain independently draggable. Wrapping disables horizontal
scrolling and resets any previous horizontal offset, including during wrap preparation.
Tree-sitter builds reserve a narrow fold column before
analysis arrives, so asynchronous fold discovery never shifts text. Disclosure
triangles share UI2's file-tree artwork. Fold bodies crop at normal font size
with bed1's reversible 160 ms smoothstep timing; fractional row geometry is shared
by painting, hit testing and caret placement. Reduced motion settles immediately.
The caret and active line number use bed1's RGB sine cycle.

Primary pointer presses place the caret immediately; the second press selects a
word and the third selects the logical line excluding its line ending, with the
caret after the final character. These selections do not wait for release.

The caret's logical position updates immediately. Its visual leading/trailing
edges use bed1's 35/110 ms cubic easing and a bounded fading trail. Content
coordinates prevent scrolling from becoming caret motion. Layout-only changes
settle the caret, and `MotionPolicy.enabled = false` disables movement animation.

## Optional syntax and folding contributions

Enable `treesitter` to depend on [shared Tree-sitter analysis](../bedtime-treesitter2/README.md).
Standalone hosts own one `AnalysisSession` per document, call
`syntax::update_analysis` with their language choice, then attach the same accepted
`AnalysisSession` to sibling views with `EditorView::set_analysis`. This retains
colors during pending refreshes; `set_syntax` also accepts an explicit observation
and clears contributions when given `None`. These are direct contribution APIs,
with no provider registry or hidden parsing during drawing.

```sh
cargo run --offline --locked -p bedtime-editor2 --features treesitter --example syntax
```

With `workbench,treesitter`, install `EditorAnalysisPlugin`, bind text editors
normally, and attach `CodeLanguage` to the document entity (`DocumentId.0`).
`DocumentAnalysis` owns one managed worker/session per document with open editor
viewers, including hidden tabs. Its ordinary Update system submits cheap roots,
polls results and retires attachments after the last viewer detaches; Workbench
owns worker shutdown across document/workspace closure. Drawing attaches the
document's analysis before temporarily removing the view component.
`analysis_for_panel` validates the panel binding, current revision and language
before returning it.
Changing language invalidates prior results even at the same text revision.

Visible fragments use binary span lookup and a `SyntaxPalette` at layout time;
color changes preserve font metrics and require no reparse. The Workbench adapter
maps the current host theme to that palette. Long-line shaping retains its 32 KiB
ceiling.

During edits and pending analysis, views retain the last accepted colors and map
only visible spans through the complete change journal, including IME drafts.
Fresh analysis replaces these provisional colors. Source/language changes,
analysis refusal, explicit detachment and journal gaps discard them. Retained
colors never supply fold controls or Structure navigation; those still require a
matching revision. No full-document highlight copying or remapping runs per edit.

`toggle_fold`, `fold_all` and `unfold_all` keep collapse state in the view, outside
undo history. Sparse cached hidden-row intervals compose with wrapping for
scrolling, painting, hit testing, selections and vertical/page navigation. No
full-document visible-row vector is rebuilt per frame. Gutter controls remain
available with line numbers hidden; collapsed headers display an ellipsis.
Selections, copy and edits still use canonical source bytes. Find and external
navigation reveal hidden targets. A fold containing the caret moves it to its
header when collapsed. Edits before folds map their ranges; edits touching them,
journal gaps and changed source/language identities expand uncertain regions.
Folding and navigation reject active composition.

`EditorView::set_diagnostics` accepts an immutable shared `Diagnostics` report
with UTF-8 byte ranges, severity, messages and a captured revision. Producers
prepare its interval index and severity counts off-thread. Visible fragments
draw clipped underlines with message tooltips only at that revision and outside
IME composition; edits hide old contributions until refreshed. Unversioned
reports are visually muted best-effort observations. The API has no LSP types.
The optional [LSP plugin](../bedtime-lsp2/README.md) supplies these contributions
through canonical Workbench documents. `language_tools` enables revision-bound
pointer text/symbol hits and modifier-click definition intentions in `EditorOutput`;
the host owns language requests and diagnostic/documentation popup arbitration.

Host-supplied `set_completions` lists reserve navigation/acceptance keys before
ordinary editing and paint a virtualized caret-anchored list. `queue_completion`
accepts a captured revision and `CompletionEdit`; canonical editing/read-only/IME
gates are checked when applying it. Replacement and nonoverlapping additional
edits form one persistent-buffer change and undo step, preserving intervening
rope roots. Numbered snippet defaults, choices, escapes, mirrors and variable
defaults support Tab/Shift-Tab navigation. Unsupported transforms fail preparation.
Stale proposals change no text. None of these contribution APIs depends on LSP.

Other gutter markers, multicursor and debugger integration remain later milestones. No universal
provider framework is installed.

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

Wrap metadata is prepared in cancellable view-owned workers. During resize the
last revision-valid wrapped geometry remains visible and interactive until the new
width is ready; no preparation label is painted. Initial preparation and bulk
edits without a valid index use unwrapped coordinates. A ready index updates only
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
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter million_line_ -- --ignored --nocapture
cargo check --offline --locked -p bedtime-editor2 --no-default-features --example standalone
cargo test --offline --locked -p bedtime-workbench2 --no-default-features
```

The explicit large fixture covers 112 MB and one million lines, bounded drawing,
navigation near EOF, editing and shared-root undo. The former bed2 native editor
scenario was retired with the application demo; the following results record
historical evidence from before that cleanup.

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


Verified 2026-10-19: the plain editor suite and standalone build passed; the
feature-enabled suite includes independent fold geometry/hit testing, wrapping,
theme changes, composition gates, grouped edits/history, journal expiry, and
editable analysis-limited long lines. The two explicit 112 MB/million-line tests
passed with `workbench,treesitter`: unwrapped idle median 0.83 ms, maximum 1.36 ms,
edit 1.37 ms; wrapped incremental edit 1.01 ms and 17 rows / 889 shaped bytes.
These measure headless UI work; large-file semantic analysis is intentionally
limited. The current Code/Structure native smoke passed eight captures under
`target/bed2-code-smoke`. Background analysis timing and bounds are recorded in
the Tree-sitter crate README.

Completion menus use bed1-style compact rows: a host-supplied category, label and
muted inline detail, with accent-tinted selection/hover backgrounds and up to eight
visible rows. Item tooltips are omitted; the host owns resolved documentation.
