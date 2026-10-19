# bEd 2

A small workbench application with one workspace and an initial 3D Viewport panel.
The titlebar's settings icon opens a Theme panel with live color, dimension,
panel rounding/gap, opacity and macOS background blur controls, plus normal/bright
terminal ANSI swatches. Changes are in memory.

```sh
cargo run --offline -p bed2
```

`src/main.rs` installs Bevy, UI2 and Workbench. Startup attaches `WorkbenchSurface`
and `Titlebar { height, draw }` to the window and spawns an explicit
`WorkbenchCamera`. `WorkbenchSurface` owns the panel appearance and interaction
settings edited by the Theme panel. An ordinary `Commands` system composes a
workspace, layout, area and registered viewport panel.
Hello World, Theme and the titlebar renderer live in `main.rs`; `cube.rs` owns the
shared cube/ground/light scene and the viewport implementation. `tasks.rs` adds
the Tasks & Processes panel, and `documents.rs` adds shared document viewers and
application-owned quit policy. `files.rs` integrates the reusable filetree panel,
text editor tabs and root-scoped preference storage; `file_editor.rs` owns local
text documents, source identity and streaming save policy.
`bedtime-terminal2::TerminalPlugin` adds local shell
panels; `terminal.rs` contributes optional automation inspection only.
All use the main World and renderer. To open another
panel, close the viewport tab and right-click the empty area to choose its type.
Workbench resolves membership and local defaults automatically. Closing the window
requests save/discard/cancel across all workspaces, then drains managed work before
exiting.

Each viewport panel is also its `Camera3d`, with an independent `UiViewport` image
and `OrbitCamera` state. Left-drag orbits, Shift-left-drag or middle-drag pans,
scroll zooms, and **Reset view** restores the initial camera. egui owns the drag
until release, including outside the image; controls respect window focus and
popups. Panning scales with distance, field of view and viewport height. Pitch
and distance are clamped. Camera transforms update in `Update`; visibility is
synchronized in `UiSystems::SyncViewports` before UI2 resizes images and Bevy
updates projections. Hidden views stop rendering. Closing a panel retires its
camera and image ownership; the application-owned scene remains available.
Workspace snapshots store the panel type and layout, so restoring creates fresh
orbit state. Camera-state persistence is not part of the layout format.

The Tasks & Processes panel demonstrates the existing `WorkspaceWork` API:

- **Start task** sums 1 through 100 on a worker thread, with a configurable
  duration, progress, a typed result (5050), and cooperative cancellation.
- **Start process** launches a fixed interactive echo script (`/bin/sh` on Unix,
  PowerShell on Windows). Send text with the button or Enter; `stderr` writes to
  stderr, `fail` exits with code 7, and `exit` exits successfully. **Close stdin**
  delivers EOF after queued writes; **Stop process** requests cancellation.
- Each panel retains its latest task result and the newest 64 KiB of each output
  stream. Process supervision status and the child's exit status are displayed
  separately: `Finished` can accompany a nonzero exit code.

Both workers are owned by the workspace where they started, with the panel as a
shorter-lived owner. Workbench handles cancellation and process cleanup when
either closes. An ordinary Update system collects results and drains output for
hidden panels and inactive workspaces too. Worker timing uses real time, not
Bevy's virtual time. Workspace snapshots restore the panel type with fresh idle
state; live workers, logs and results are not serialized.

## Terminal

Choose **Terminal** from an empty area's panel picker. Each panel owns one local
shell, started automatically after its first size observation. Normal shell input,
ANSI/truecolor output, wide/combining cells, scrollback, selection/copy, paste,
alternate screen, mouse reporting and resize use the headless terminal core with
a new egui canvas. The terminal follows the active theme's font size, default and
ANSI colors, with bed1's Everforest palettes for dark/light presets. **Theme →
Terminal ANSI colors** edits normal/bright colors independently and recolors
existing indexed output without restarting the shell. Explicit truecolor and shell
OSC overrides remain intact. PaperMono bold uses the actual bold face rather than
an overstrike; faint suppresses bold, and the grid/text align to physical pixels.
Tabs follow the local foreground process (falling back to the shell name), and
return to the shell when a job finishes. Names are observed off the UI thread,
including for quiet or hidden sessions; shell OSC titles remain separate metadata.
The running canvas has no extra title/Stop toolbar beneath its normal panel tab;
it uses a 14×12-point content inset with an inset scroll indicator. The default
background inherits panel opacity without adding another background layer.

- Copy/paste/select-all use Cmd+C/V/A on macOS and Ctrl+Shift+C/V/A elsewhere.
  Ctrl+C remains shell interruption. Shift bypasses application mouse reporting
  for local selection/scrolling; Shift+PageUp/PageDown/Home/End navigate scrollback.
- Shell exit retains the transcript and shows its status. **Restart** creates a
  fresh session after the old runtime has finished cleanup. Applications can still
  use `TerminalPanel::stop()`; the canvas has no permanent Stop control.
- Hidden/inactive sessions continue draining/parsing output, even with workspace
  background updates disabled. Panel/workspace closure and the normal quit flow
  cancel and drain the workbench-owned runtime off the UI thread.
- Workspace/layout snapshots restore fresh, lazily started panels, not live PIDs,
  launch options or transcripts. Shell/cwd/environment defaults are local; the
  application does not add shell helper executables or another entrypoint.

Initial support is macOS/Linux only, without SSH, `+` commands, inline images or
shared debugger sessions. Fonts determine glyph coverage; cross-cell contextual
shaping, bidi reordering and color emoji are not implemented. The text-only parser
also omits DCS protocols and OSC 1337. See the
[terminal contract](../bedtime2/crates/bedtime-terminal2/README.md).

## Files

The titlebar's **stacked-files icon** opens Files; it is also available in the empty-area
panel picker. Supply a directory as the first positional application argument:

```sh
cargo run --offline --locked -p bed2 -- /path/to/project
```

Without an argument, bEd2 explicitly chooses cwd. The reusable
[`bedtime-filetree2`](../bedtime2/crates/bedtime-filetree2/README.md) library never
chooses cwd or assumes one root per workspace: each panel has a `FileTreeRoot`.
Root resolution, lazy enumeration, sorting and watching run off the UI thread.

- Folders expand/collapse with animated disclosure arrows and viewport-only rows.
  The root collapses too, without forgetting nested expansion or cached listings.
  Arrow keys, Home/End, PageUp/PageDown, range/toggle selection and prefix
  navigation work without recursively indexing the project. The duplicate
  name/refresh header has been removed.
- Single clicks open/reuse an **editable preview** beside the tree; a double click,
  Enter or context Open pins/reuses that same file rather than making duplicate
  viewers. Editing pins the preview, so another file never replaces unsaved work.
  Routing retains the originating workspace/layout. UTF-8 files up to 256 MiB use
  the [reusable text editor](../bedtime2/crates/bedtime-editor2/README.md);
  binary/larger/nonregular files show metadata. **Split view** shares one document
  and history while keeping independent selections, scrolling and line numbers.
  Drawing is virtualized vertically and horizontally, including very long lines.
  Cmd/Ctrl+S saves through the shared document service. File reads, indexing,
  source comparisons and streaming writes run on managed workers.
- Direct context menus and inline naming provide file/folder creation,
  rename, cut/copy/paste, duplicate, Move to Trash and visibility controls. Incoming
  file/folder drops copy; internal same-root dragging moves, with Option/Primary
  for copying. A hovered child file chooses its enclosing folder, whose whole
  visible subtree gets the panel-style overlay. Invalid drops are silent and
  unhighlighted. Operations preserve symlinks and never overwrite a destination.
  Operation commands/shortcuts are deferred; tree navigation and Enter/Escape in
  naming remain. `FileTreeSettings` supplies per-panel `read_only` and
  `indent_size`; read-only governs tree operations, not viewer loading.
  Cancel is cooperative and reports already-completed items; conflicts remain
  errors rather than replacing or merging existing trees.
- Explicit Hide/Unhide rules and the hidden-file policy are shared by root within
  a workspace. Reveal Hidden Entries temporarily bypasses filtering for recovery.
  bEd2 loads/stores all root records in the startup project's
  `.bed/filetree2.json`, using lossless native-path JSON, lazy loading and atomic,
  debounced worker writes. Startup does not create storage or rewrite untouched
  preferences. Parse/save errors create readable dialogs; records above the
  4 MiB application storage limit are rejected before replacing readable storage.
- Own moves update source paths and save bindings after confirmed filesystem
  success while preserving text/history. Confirmed deletion unloads **clean**
  viewers; dirty, drafted or saving documents retain local work and lose their
  backing save binding. Save As is not implemented yet. Clean tabs close if
  siblings remain, otherwise leave an unloaded state. Worker metadata checks cover
  hidden/inactive viewers and paths outside all current tree roots, at most 64
  viewers per workspace batch with 500 ms cadence and round-robin coverage.
  Absence decisions defer during admitted mutations so an own move can rebind
  first. Permission/temporary errors are not deletion. Late work cannot resurrect
  an unloaded source. This is bed's filesystem-viewer policy, not a change to
  generic Workbench document retention or the memory-only Documents demo.
- Filetree, preview and preference failures use **human-readable error dialogs**,
  not inline OS diagnostics. Technical details are optional and initially
  collapsed; acknowledgement does not reopen an unchanged persistent failure.
  Invalid drag/drop and deliberate cancellation stay silent. Normal quit rejects
  still-admitted file operations, then admits the latest preferences and drains
  the storage worker's cancellation flush.

Preview is a tab-lifetime policy, not a second document format. A single click
reuses the layout's one replaceable preview, while double-click, Enter, context
Open or **Keep open** pins it. Opening an already displayed path reuses its tab.
Preview labels are italic at 90%
text opacity instead of underlined. Workbench's optional per-item `PanelTabStyle`
can override italic/opacity independently from semantic `PanelInstance.preview`;
these style/configuration components are runtime-only.

Reload observes disk contents through the existing source-generation and
conflict protocol. **Keep my edits** acknowledges the observed disk checkpoint;
**Use disk contents** explicitly replaces local content. Saves compare exact
source bytes, preserve permissions and symlinks, and atomically replace the target.
There is no filesystem content CAS: an unrelated process can still write between
the final comparison and rename. Known own moves/deletions invalidate pending
save identities. File sessions currently deduplicate by workspace and source path;
different aliases can name separate sessions.

The first editor slice includes normal selection/clipboard/navigation/history,
IME drafts, line numbers and bed1's caret animation. Indentation-aware wrapping,
smart indentation/backspace, find/replace, folding and language tooling remain
later milestones. Editor settings are in memory. The standalone example requires
neither Workbench nor language plugins:

```sh
cargo run --offline --locked -p bedtime-editor2 --example standalone
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_editor.py
```

Git status/ignore integration is deferred. Cross-device moves and atomic
no-replace commits outside macOS/Linux report Unsupported without deleting the
source. Native clipboard Cut requires reliable ownership evidence (macOS/Windows;
other native backends offer Copy/internal dragging). See the library contract for
resource limits, copy metadata limits and concurrent-filesystem caveats.

## Documents

The titlebar's folder icon opens **Documents** (the memory-only demo); it is also available in the panel
picker. **Create / reopen text** and **Create / reopen model** open a multiline
text session or a typed title/integer model. **Duplicate viewer** creates another
panel with independent selection/scroll state and the same canonical contents,
revision and undo/redo history. Drag tabs into separate areas, or reopen the same
document from another layout in the workspace. Closing every viewer retains the
session; another workspace gets its own instance.

- Accepted widget edits commit immediately. Typing groups end after an idle gap,
  focus loss, another viewer's edit, save, history operation or source observation.
  Text widgets route Ctrl/Cmd+Z, Shift+Z and Y through document history rather than
  their own private undo buffers. Save is also available through Primary+S.
- **Save** writes a captured snapshot to a **versioned in-memory source only**.
  There are no filesystem writes and all demo data disappears at application exit.
  A short worker delay makes editing during a pending save observable; a later
  edit remains dirty when that captured save completes.
- **External update + observe** modifies the memory source and offers the new
  value to the document. Clean updates are undoable without rolling back source
  knowledge. Divergent changes retain base/local/incoming for **Accept incoming**
  or **Keep local**. Neither conflict choice writes to the source.
- **Change source only** followed by Save demonstrates conditional-write failure;
  **Observe source** supplies the required refresh. **Reject saves** simulates a
  retryable backend error while retaining content, history and viewers.
- Quit offers **Save All and quit**, **Discard and quit**, and **Cancel**. Pending
  writes/drafts block destructive shutdown; errors or documents changed during
  Save All return to the decision. Cancel abandons only quit, not admitted writes.

The demo does not persist restoration descriptors. Restoring a layout creates
unbound viewers that can select a compatible live document or reopen the sample.
The Documents picker also reopens retained real-file editors, including unsaved
documents whose sources were deleted after editing. Its creation buttons and
memory viewers keep their in-memory save behavior.
The [document contract](../MIGRATION/workbench-documents.md) describes the core API.

Optional external automation uses the same application setup:

```sh
cargo build --offline -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
python3 bedtime2/tools/smoke_cube.py
python3 bedtime2/tools/smoke_tasks.py
python3 bedtime2/tools/smoke_documents.py
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_filetree.py --binary target/debug/bed2 --wide-entries 20000
```

The [automation protocol](../bedtime2/crates/bedtime-automation2/README.md) supports
stdin/stdout control, native input injection, runtime configuration, inspection
and rendered screenshots. Scenarios and assertions live outside `main.rs`.
The optional `app.inspect_cube` command exposes orbit state, transforms, camera
activity, projection aspect ratios and image identities for validation. The cube
scenario checks real input, independent views, texture captures, resizing,
workspace restoration and cleanup.
`app.inspect_tasks` exposes panel state, progress, results, process I/O/exit
status and workspace worker ownership. The task scenario drives the real
controls, checks completion/cancellation, stdout/stderr, EOF, nonzero exits,
fresh-state restoration, and panel/workspace cleanup.
`app.inspect_documents` reports canonical document/source contents, status,
revision/history availability, viewer bindings and quit phase. The document
scenario exercises real text input, shared undo, typed edits, memory saves,
external conflicts, source CAS failures and workspace ownership. Its process
cleanup uses the automation transport's explicit exit, not the application quit
dialog; quit decisions are covered by application unit tests.
`app.inspect_terminal` reports per-panel process status, viewport text/cells/modes,
geometry, resolved palette and work ownership. Its native scenario uses the real panel picker and
keyboard input, and checks PTY editing, ANSI/Unicode, alternate screen, native
resize, independent sessions, hidden/inactive I/O, restart/restoration and cleanup.
It also captures dark/light/custom terminal colors and font-size changes, checking
that live edits retain the process/transcript and preserve OSC/truecolor behavior.

`app.inspect_filetree` reports roots, observations, viewport geometry/motion,
activation counts, preview state, clipboard backend/revision and preference
storage. `app.set_filetree_root` changes an individual panel's supplied root;
`app.set_filetree_settings` atomically configures read-only/indentation;
`app.drop_files` injects bounded Bevy native-drop events through the normal bridge,
not a direct filesystem action. The native Files scenario uses isolated temporary
projects and a pinned executable, and overwrites the system clipboard with its
fixture paths. Injected events do not verify an OS Finder/Explorer drag.
`--functional-only` explicitly skips pixel validation; normal runs require
nontransparent, content-change-checked captures. `--wide-entries` additionally
checks bounded viewport painting and End navigation on a large flat directory.

Verified on 2026-10-18 on macOS/Metal: the five UI2 viewport tests, ten automation
tests, normal application compilation, and both native smoke scenarios passed.
The cube scenario produced eight captures in `target/cube-smoke`; the existing
workbench scenario produced nine in `target/automation2-smoke`.

The Tasks & Processes addition was verified on 2026-10-18 on macOS/Metal: its
native scenario passed with six captures in `target/tasks-smoke`, and all 17
`bedtime-workbench2` workspace tests passed. The ordinary application also
compiled without the automation feature. The Windows echo script has not been
run in this environment.

The Terminal addition was verified on 2026-10-18 on macOS/Metal: 36 Terminal2 tests
and the full UI2/Workbench2/bed2 suites passed, along with 55 headless and 84
legacy default-feature terminal tests. Normal bed2 compilation and the legacy
terminal plugin's remote feature compiled. Its native smoke passed with 10
nontransparent, content-change-checked captures in `target/terminal-smoke-validated`;
the existing workbench and task/process native scenarios passed as well. The
validated run used a pinned automation executable to avoid concurrent builds
replacing `target/debug/bed2`. Linux and non-macOS native input were not run.

The terminal appearance update was verified on 2026-10-18 on macOS/Metal: 39
Terminal2, 27 UI2, 166 Workbench2, 11 automation and 10 bed2 unit tests passed, plus
the workbench document doctest and normal bed2 compilation. Its expanded native
scenario passed with 14 captures in `target/terminal-rendering-smoke`, including
dark/light/custom ANSI and font-size changes. The existing workbench scenario
passed with nine captures in `target/terminal-rendering-workbench-smoke`. Both
used `target/bed2-terminal-rendering-smoke`, a pinned automation build. Other
native platforms were not run.

The initial Files addition was verified on 2026-10-19 on macOS/Metal: 85 Filetree2 tests,
7 Clipboard2 tests (with native support enabled), 21 bed2 tests, 27 UI2, 166
Workbench2, 44 Terminal2 and 11 automation tests passed, plus the Workbench document
doctest. Normal bed2 and headless Workbench compilation passed. The native Files
scenario passed with seven nontransparent, content-change-checked captures in
`target/filetree-validated-smoke`; the existing general workbench scenario passed
with nine captures. Incoming drops were exercised through injected Bevy native
window events, not an OS Finder drag. Native clipboard cases used the actual
macOS System backend.

A 20,000-entry temporary root reached ready in about 0.30 seconds in one observed
run and painted 21 viewport rows; End navigation reached the last entry without
materializing 20,000 widgets. This is an observation on this machine, not a
portable latency guarantee. All mutation fixtures were removed. Linux/Windows
native operation/input and successful OS Trash were not exercised.

Earlier runs intermittently produced all-transparent full-window captures, also
in the pre-Files general workbench scenario. Fresh current/previous-pinned startup
and Hello controls, followed by the final strict Files/general scenarios, passed.
No renderer fix is claimed: empty screenshots remain failures, and functional-only
runs must not be described as pixel verification.

Review follow-up verification: **411 unit tests plus the document doctest passed**
(35 bed2, 100 Filetree2, 187 Workbench2, 27 UI2, 44 Terminal2, 11 automation and
7 Clipboard2). Normal bed2 and no-default-features Workbench checks passed. The
updated Files scenario passed with **11** render-checked captures in
`target/filetree-review-dialog-smoke`, including a real rename-conflict dialog,
acknowledgement and retry with its draft intact, root collapse, read-only/indent
configuration, root/child-file drop routing, native System clipboard symlink Cut,
and external deletion in active/inactive layouts. The existing workbench scenario
passed with 9 captures using that pinned automation executable in
`target/filetree-review-workbench-smoke`. A 20,000-entry root reached ready in
about 0.26 seconds and painted 22 viewport rows in that run (not a portable
benchmark). An actual Finder-driven drag remains a manual verification item;
its new coordinate bridge has source diagnosis and native-coordinate unit tests,
while the automated drop scenario injects Bevy window messages.
