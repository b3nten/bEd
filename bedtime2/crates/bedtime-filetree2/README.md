# bedtime-filetree2

Concrete local filesystem panels for Workbench2. The application supplies a root
**on each panel**, chooses what an activation opens, and owns preference storage.
There is no implicit workspace root, document-backed filesystem, recursive finder
index, Git installation, or generic decoration/provider framework.

## Application contract

Install `FileTreePlugin` alongside the application's usual Bevy/UI2/Workbench
setup. Installing the plugin creates no panels and registers no panel types.
Compose a concrete `FileTreePanel` with its root explicitly:

```rust,ignore
let panel = world.spawn((
    FileTreePanel::default(),
    InArea(area),
    FileTreeRoot(project_directory),
    FileTreeSettings { read_only: false, indent_size: 16.0 },
)).id();
// Required components provide metadata/drawing immediately and preserve overrides.
// Spawning selects this tab but does not change the layout's focused area.
```

- Missing `FileTreeRoot` means **unconfigured**, not cwd. A supplied relative path
  is resolved on work. An inaccessible/non-directory root is an explicit error.
- Changing/removing the root, or moving a panel to another workspace, starts a new
  browsing generation. Expansion, selection, drafts and stale intentions reset.
- `FileTreeSettings` is per-panel runtime configuration (not hidden-path storage).
  `read_only` blocks filesystem mutations at admission, including incoming drops,
  while allowing browsing, opening, copying and visibility changes. Dragging from
  a read-only panel to a writable one copies rather than moves. This does not
  constrain document loading/editability; those are application policy. Indentation
  defaults to 16 UI points, clamps to `0..=64`, and nonfinite values use 16.
- Panels in one workspace share filesystem observations and work, but have
  independent roots, expansion, selection, scrolling and inline edits. Different
  workspaces have separate runtimes. Hiding a panel or disabling background UI
  updates does not stop watching or result reconciliation.
- Files emit buffered Bevy `FileTreeOpenRequested` messages with the originating
  `panel`, `workspace`, actual native `PathBuf`, and `FileTreeActivation`.
  Consume these after `FileTreeSystems::Update`; do not infer the destination from
  whichever workspace happens to be active at delivery.
- The first single click is delivered immediately. A double click's second click
  produces `DoubleClick`, **not another `SingleClick`**. Applications can reuse a
  preview on single clicks and promote it on double/keyboard/context activation.
  Directory navigation stays local and emits no open-file request.

`FileTreePanel::snapshot()` exposes immutable Arc observations; `selection()`,
`error()`, `is_busy()`, `refresh()` and viewport-only `row_rects()` support hosts
and diagnostics. The row index is built on the worker. Paths—not lossy labels—are
identity. Own confirmed moves remap navigation and manual hiding; external renames
are reconciled as filesystem observations, not guessed identity changes.

## Interaction

- Every folder, including the root, expands/collapses. The root starts expanded;
  collapsing preserves cached listings, watches and nested expansion choices.
  There is no permanent duplicate name/refresh toolbar. Arrows rotate and visible
  rows animate with UI2's motion policy; reduced motion settles directly.
- Up/Down, Left/Right, Home/End and PageUp/PageDown navigate; Shift selects ranges,
  Primary-click toggles selection, and printable text performs prefix selection.
  Enter opens the current leaf or toggles its folder.
- Context-menu Rename edits inline; Enter submits and Escape cancels. The stem is
  selected initially. Failure retains the draft for retry; unchanged native names
  do not round-trip through a lossy display string.
- Direct context menus provide New File/Folder, Rename, Copy, Cut, Paste,
  Duplicate, Move to Trash, hiding/recovery and Refresh. Filetree Workbench command
  registrations and operation shortcuts are intentionally removed pending the
  shortcut-system redesign. Ordinary text-editor keys still work inside naming.
- Menu actions capture row/selection and generation. Later selection/focus changes
  cannot redirect an admitted action. Refresh remains available on a failed root's
  background even when there is no readable root row yet.
- Internal dragging moves within the same root; Option/Primary requests a copy.
  Another root or a read-only source is a copy. Incoming OS drops always copy.
  A child-file hover selects its enclosing folder as the destination, never the
  file itself. Root background targets the root.
- Eligible destinations use the same overlay painter as panel dragging, covering
  the folder's entire visible subtree. Extents are indexed off-thread; rendering
  does not scan all descendants. Closed folders expand after continuous hover,
  and only the hovered viewport autoscrolls.
- Self/descendant drops, same-parent moves, stale/cross-workspace payloads and
  read-only destinations have no overlay and are silent no-ops. The eligibility
  decision is shared by feedback and release. Real I/O failures still report.
- Only the pointer-owning panel consumes native drops, exactly once, without
  changing the clipboard. On macOS install `WorkbenchNativeWindowPlugin` (normally
  included by `WorkbenchPlugin`) for live OS-drag coordinates: Winit 0.30's file
  drag events carry paths but no positions. The native adapter samples the content
  view pointer during hover and the final drop frame.
- Only explicit collapse retires rows toward an ancestor. Rename/move/removal
  does not fly the old row or scroll anchor toward the root.

Failures create human-readable Workbench error dialogs. Original diagnostics are
available under collapsed Technical details; portable operation error kinds drive
messages rather than parsing OS strings. Persistent failures are acknowledged
once until recovery or a fresh attempt. Failed-folder indicators remain status,
not inline diagnostic text. Invalid gestures and deliberate cancellation do not
create error dialogs, and partial completed changes are never called a rollback.

## Work, observations and responsiveness

One workspace-managed filesystem worker owns directory reads, sorting, flattened
row projection, native watch management and serialized mutations. Initial opening
reads the root only; expanded directories are loaded lazily. Symlinks are rows,
never recursively traversed. Sorting is directories first, case-insensitive display
order, then native-name order for deterministic ties.

Watching is invalidation plus reconciliation, not blind replay of event contents.
Watches precede baseline reads, include root parents, and are released on cache
retirement. macOS uses a small directory-only kqueue implementation; other desktop
platforms use notify's native watcher. Native failures are observable and enable
bounded periodic polling. Ordinary healthy idle does not crawl the project.
Background reloads retain the last known listing, so another file's watcher event
does not erase selection or an unfinished rename. Loading observations are not
proof of deletion.

Latest-value requests/snapshots coalesce. Limits are 64 views, 512 cached/expanded
directories, and 16 queued/running/undrained mutations per workspace; exceeding a
limit reports an error rather than silently omitting expanded branches. Native
notification intake is bounded, and overflow requests reconciliation. Directory
count is bounded, not the total number of entries in a very wide directory.

Main-world update/drawing does no filesystem enumeration, sorting or flattening.
Normal drawing scales with the viewport; explicit typeahead and
selection reconciliation can scale with the user's selection or row count.
Obsolete library-owned snapshots are handed back to the worker for destruction
through a bounded retirement lane. A saturated lane or already-stopped worker
uses synchronous teardown; external Arc holders determine their own final-release
thread. A long mutation occupies this same worker: the UI remains interactive,
but new directory reads/watch reconciliation wait until it finishes or is cancelled.

## Mutation safety and lifecycle

- Create, rename, move and copy never replace an existing destination. Duplicate
  and copying into the same parent choose deterministic keep-both names. Other
  collisions are errors. There is no automatic merge/replace of directories.
- Native paths remain native. Names reject empty/escaping/multi-component values.
  Canonical parents enforce the selected root boundary; the root itself cannot
  be renamed, moved, duplicated, hidden or trashed. Final symlinks are preserved.
- Copies stream in 64-KiB chunks into a reserved sibling staging directory and
  atomically commit without replacement. Source identity/version and staging
  ownership are rechecked before commit. Cancellation cleans uncommitted staging
  when it is still ours, never a replacement at the same name.
- Cut intent is consumed **only for confirmed moved sources**. Failed/unfinished
  sources remain retryable. A newer Cut or replaced clipboard is not consumed or
  overwritten by an older operation's completion. Native move authorization uses
  a platform clipboard change counter plus the shared service revision and file
  payload; memory mode has complete local write identity.
- An authenticated local Copy/Cut uses the original selected paths, not the native
  clipboard's round-tripped paths: macOS arboard canonicalizes symlinks. Clipboard
  snapshots authenticate ownership only; copying/moving their canonical targets
  would be incorrect. Loss of ownership and cross-root cut paste still safely
  fall back to Copy.
- Native cut ownership is supported on macOS/Windows. On other native clipboard
  backends without a reliable change identity, Cut reports unsupported rather
  than risking a stale destructive move; Copy and internal drag remain available.
- macOS/Linux provide atomic no-replace commits. Other platforms return explicit
  Unsupported for these commits. Cross-device moves also return Unsupported with
  the source preserved. Copies reject special files and nesting beyond 256 levels;
  directory modes, ACLs and timestamps are not preserved.
- Path-based validation is not a sandbox against hostile concurrent ancestor
  replacement, and metadata revalidation is not a locked filesystem snapshot.
  An externally relocated staging directory can require manual cleanup.

`FileTreeOperationFinished` carries actual completed changes, per-path errors and
cancellation—even after the origin closes or changes root. Cancellation is not
rollback. Outcomes have reserved queue slots and an application-World archive
independent of workspace entity lifetime. A failed worker without a final result
reports that effects are unknown rather than claiming nothing happened.

Hosts needing to persist move-related state before destructive workspace closure
can use `filetree_operation_count(world, workspace)`: this includes admitted work
whose panel has closed. Defer close until outcomes are reconciled, then admit the
host's preference save before cancelling its storage worker. Direct Workbench
close still cancels/drains work and reports outcomes; it cannot preserve custom
components of an already-retired workspace for the application.

## Visibility and persistence

`FileTreePreferences` is workspace-owned data, keyed by each canonical absolute
root. `FileTreeVisibility` combines the hidden-file policy with explicit
root-relative hidden paths; hiding a directory hides descendants too. Reveal
Hidden Entries temporarily bypasses both filters and dims hidden rows. Unhide and
Clear Explicitly Hidden Entries provide recovery. Temporary reveal is not saved.

`FileTreePreferencesChanged` tells the application to persist its custom component;
there are no core Workbench layout snapshots or snapshot change notifications.
`encode`/`decode`
provide a strict versioned JSON format, losslessly encoding Unix bytes or Windows
wide path units. Unsupported versions/platforms, duplicates, malformed paths and
escaping rules reject the whole document. `FileTreePreferences` also implements
Serde serialization/deserialization through those same validated native-path
records, so a host can embed the data directly in another format such as RON
without embedding a JSON string. No filesystem I/O occurs in this codec.
The host chooses storage location, read/write policy and errors; the library does
not create project configuration directories.

Git is deliberately deferred. A future concrete repository observation in the
World should drive ignore filtering/status independently of open documents, with
explicit coverage/unknown state. It should not replace manual hidden preferences
or introduce a speculative decoration/provider registry.

## Verification

Focused commands:

```sh
cargo test --offline --locked -p bedtime-filetree2 -p bedtime-clipboard2
```

bEd2 installs Files and routes activations to file-backed editors with optional
Tree-sitter analysis and Structure. Its application policy chooses home for the temporary workspace or an explicitly
opened folder, background loading, source-checked saving and centralized RON
visibility storage alongside the workspace's settings/layouts. The library's explicit per-panel-root/activation contract is unchanged.
The application scenarios use temporary projects:

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_filetree.py
python3 bedtime2/tools/smoke_editor.py
python3 bedtime2/tools/smoke_code.py
```

Injected drops do not verify Finder/Explorer OS dragging. Non-macOS native
behavior remains unverified.

Selected row labels use bed1's RGB sine cycle. Disclosure arrows share UI2's
filled triangle with editor folds and Structure; expansion uses 160 ms smoothstep
timing, including row movement. Reduced motion uses a steady selection color.
