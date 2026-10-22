# bEd 2

A workbench application with a temporary default workspace rooted at the user's
home directory. An explicit directory argument or **File → Open Folder…** opens
or reuses a folder workspace. Application metadata lives in the user configuration
directory, never inside the opened project.

The titlebar controls are **Terminal, Files, Structure, Settings**. Click to open
or focus a tool in the current layout; drag to create a new panel in an area or
along its edge. **View** also offers concrete panel creation. Workbench has no
panel-type registry or picker.
The controls match bed1's native toolbar geometry: 16-point artwork in 26×22-point
buttons, 2-point gaps and a 10-point trailing inset. UI2 owns the macOS final alpha
conversion so antialiased edges remain smooth over transparent window backgrounds.
Toolbar icons use the theme's text color through hover and press, with animated
interaction backgrounds.

Settings has General, Appearance and Editor sections with a Global/Workspace
switch. Startup opens a single terminal layout by default; General can opt into
reopening the last folder workspace. Document autosave defaults to a one-second idle delay. Five numbered layout
slots retain live views and workers; their starting arrangements can be saved as
global defaults. See the [composition record](../MIGRATION/application-composition.md)
for boundaries and the remaining Save As/recovery and workspace-management work.

```sh
cargo run --offline -p bed2
```

`src/main.rs` installs Bevy, UI2 and Workbench. Startup attaches `WorkbenchSurface`
and `Titlebar { height, draw }` to the window and spawns an explicit
`WorkbenchCamera`. `settings.rs` owns explicit global values, individual workspace
overrides and their application to UI/editor/tree state. `storage.rs` owns global
RON preferences I/O; `session.rs` owns one combined file per folder workspace.
`layouts.rs` creates concrete layouts after settings/session readiness and keeps
live instances while switching slots. Required components supply panel metadata
and drawing immediately; there is no panel registry or delayed panel initialization.
`menus.rs` owns creation choices, and `main.rs` renders the titlebar.
`cube.rs` owns the shared cube/ground/light scene and viewport implementation;
`tasks.rs` adds the Tasks & Processes panel. `bedtime-terminal2::TerminalPlugin` adds local shell
panels; `terminal.rs` contributes optional automation inspection.
`files.rs` installs the file tree and file-backed Code panels; `file_editor.rs`
owns their canonical document bindings and save backend. `code.rs` supplies
canvas context-menu actions, analysis execution and the application-owned
[Structure viewer](crates/bed-structure2/README.md).
`close.rs` owns application quit decisions and workspace worker shutdown.
All use the main World and renderer. Open another panel through **View**; empty
areas no longer discover registered types. A concrete panel spawn selects its tab
without changing area focus. `select_panel` and `focus_area` are independent;
`focus_panel` explicitly does both. Workbench hooks maintain ownership and fallback
selection/focus immediately as composition applies.

The memory-only Documents and Hello World demos remain removed. The file tree is
the application entry point for opening documents; the standalone library examples
remain available independently.

## Local packages and installation

On macOS, build a release app and install it for testing:

```sh
scripts/package-bed2-macos.sh --install
open /Applications/bEd2.app
```

The script creates an ad-hoc signed `target/dist/bed2/bEd2.app` and
`bEd2-<version>-<architecture>.zip`. Installation stages and verifies the new app
before replacing `/Applications/bEd2.app`, restoring the previous app if the
replacement fails. Its bundle ID is `org.bed-editor.bed2`; bed1's `bEd.app` and
settings remain separate. Omit `--install` to build packages only.

Use `--skip-build` to package an existing `target/release/bed2`.
`BED2_BINARY`, `BED2_DIST_DIR` and `BED2_INSTALL_DIR` override the executable,
package output directory and macOS installation directory. For a user-local
installation, use `BED2_INSTALL_DIR="$HOME/Applications"`.

On Linux:

```sh
scripts/package-bed2-linux.sh
```

This produces a `bEd2-<version>-<architecture>-linux.tar.gz` and, when `dpkg-deb`
is available, `bed2_<version>_<architecture>.deb`. The archive contains a `usr/`
tree with `bin/bed2`, a distinct desktop entry and icons, and notices. Extract
with `--strip-components=1 -C /usr/local` to install its contents under
`/usr/local`, or install the Debian package with the system package manager.
Linux packaging requires ImageMagick and the normal desktop build dependencies.

Both scripts build only `bed2` and package its embedded-asset license notices;
no SSH helpers or Docker builds run. macOS packages are for local testing and
are not notarized. Run packaging and license regression checks with:

```sh
python3 -m unittest discover -s scripts/tests
```

Verified on 2026-10-21: release build, real macOS app/ZIP assembly, signature
verification and `--install` into an isolated temporary Applications directory
passed. All 34 script tests passed, including Linux archive assembly and failed
installation rollback fixtures. Native Linux packaging was not exercised.

## Files and documents

On macOS the application installs real native **bEd**, **File**, **Edit** and
auxiliary **View** menus:

- **bEd → Quit bEd**;
- **File → Open Folder…, Save**;
- **Edit → Undo, Redo**, a separator, then **Cut, Copy, Paste**;
- **View** creates Files, Structure, Terminal, viewport, tasks and Settings
  panels; it never creates an empty Code panel.

The same abstract definitions supply an in-window fallback where native menus are
unavailable. Standard editing accelerators use the focused canonical editor or
text widget, not a second undo history. Save/Open Folder/Quit bindings are app-owned.
The constrained menus expose no New, Save All or Reload actions. Reload remains in
the editor's context menu; Save As remains unfinished.

Click **Files** in the titlebar to open or focus the tree in the current layout.
With no argument its root and default terminal cwd are the user's home directory,
not the executable's launch directory. **View** creates auxiliary panels, not Code:
open Code by opening a file from Files. An explicit directory chooses a folder root:

```sh
cargo run --offline --locked -p bed2 -- /path/to/project
```

The code editor has no top control bar. Right-click its canvas for Save, Reload,
Split view, Keep open and editing/language/folding options. Titlebar tool icons
animate their background and ink on hover and show descriptive tooltips.

Single-click opens a replaceable preview beside Files, updates Structure to that
file, and retains tree focus.
Double-click, Enter, or **Keep open** pins the tab; double-click/Enter focus the
editor. Modified/drafting/saving/conflicted documents are never replaced by another
preview. Reopening a file within a workspace shares its canonical document,
including retained edits/history after its last viewer closes. Different layouts
have separate preview slots. **Split view** opens another view of the same document
in an adjacent area.

UTF-8 files up to 256 MiB load on managed background workers. Binary, oversized and
nonregular files show metadata. No synchronous full-file reads occur while drawing.
**Save** or Cmd/Ctrl+S uses the canonical save service, source checkpoints and atomic
replacement. A changed disk source refuses overwrite; **Reload** compares incoming
bytes while preserving local edits, then **Keep my edits** / **Use disk contents**
resolve an observed conflict. Confirmed tree moves preserve document/history and
rebind saving to the new path. Deleted dirty sources retain edits and lose their
save binding; Save As and missing-source recovery remain future application work.

The restored tree includes lazy watching, file operations, per-panel settings and
root-scoped visibility. The application stores root-scoped visibility in the owning folder workspace's
config file, together with its settings and layout state. One asynchronous writer
owns the entire workspace record; admitted changes survive workspace retirement.
The default home workspace retains visibility changes in memory, including when
home contains an existing `.bed` directory. The reusable plugin chooses neither
cwd nor storage itself. Core Workbench snapshots/restoration remain removed; application session recipes
are described below.

## Code and Structure

Open a file through **Files**, then choose **Structure** from the application's
**View** menu. File extensions choose the initial language; the language selector
can override it, including Plain text. **Split view** shares the canonical document
with independent selection, scrolling, wrapping and collapsed regions. **Folding** exposes Fold all
and Unfold all; gutter arrows also work with **Line numbers** hidden. Syntax colors
follow the live theme. Plain text remains editable without structural analysis.

Structure uses the current eligible editor or remembers the last one while
Structure/Terminal/other panels have focus. It retains the last same-source
outline while analysis refreshes, so editing does not flash the empty-state message.
Automatic tab focus after closing a
source may establish another eligible source. Otherwise a closed/rebound source
shows **No structure available**, without scanning previous editors. Clicking a
declaration reveals its actual source tab/layout, unfolds its name, selects it
and focuses that view. Stale or composition-blocked activation has no effect.
The [Structure contract](crates/bed-structure2/README.md) and
[editor APIs](../bedtime2/crates/bedtime-editor2/README.md) describe ownership.

The editor's `treesitter` feature proves external contributions through shared
observations. The source viewer is application-owned and code-only; no generic
structure API is introduced. Analysis shares one bounded managed worker per
viewed document and retires after its last editor closes. Closing viewers retains
the canonical document under Workbench's existing contract. Files reopens retained
file documents; folder session recipes reopen file descriptors, not unsaved contents.

## Viewports

Each viewport panel is also its `Camera3d`, with an independent `UiViewport` image
and `OrbitCamera` state. Left-drag orbits, Shift-left-drag or middle-drag pans,
scroll zooms, and **Reset view** restores the initial camera. egui owns the drag
until release, including outside the image; controls respect window focus and
popups. Panning scales with distance, field of view and viewport height. Pitch
and distance are clamped. Camera transforms update in `Update`; visibility is
synchronized in `UiSystems::SyncViewports` before UI2 resizes images and Bevy
updates projections. Hidden views stop rendering. Closing a panel retires its
camera and image ownership; the application-owned scene remains available.
No core layout/session serializer persists viewports. Any camera-state restore
recipe belongs to the application; the current session format does not store orbit state.

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
Bevy's virtual time. No session serializer stores live workers, logs or results;
applications reconstruct fresh idle panels if implementing restore.

## Terminal

Choose **Terminal** from the application's **View** menu. Each panel owns one local
shell, started automatically after its first size observation. Normal shell input,
ANSI/truecolor output, wide/combining cells, scrollback, selection/copy, paste,
alternate screen, mouse reporting and resize use the headless terminal core with
a new egui canvas. The terminal follows the active theme's font size, default and
ANSI colors, with bed1's Everforest palettes for dark/light presets. Changing the Settings theme recolors existing indexed output without restarting
the shell. The library also supports application-supplied ANSI palettes. Explicit truecolor and shell
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
- There is no core workspace/layout snapshot. Application folder recipes can
  reconstruct a terminal panel, which starts lazily, never from a persisted PID or
  transcript. Startup waits for root/preferences readiness, and uses home for the
  default workspace or the explicit folder root (unless launch policy is overridden).
  No shell helper executable or extra entrypoint is installed.

Initial support is macOS/Linux only, without SSH, `+` commands, inline images or
shared debugger sessions. Fonts determine glyph coverage; cross-cell contextual
shaping, bidi reordering and color emoji are not implemented. The text-only parser
also omits DCS protocols and OSC 1337. See the
[terminal contract](../bedtime2/crates/bedtime-terminal2/README.md).

## Settings, layouts and persistence

All application metadata uses versioned RON with a `.bed` extension. The config
root is `$XDG_CONFIG_HOME/bed2`, falling back to `$HOME/.config/bed2`:

| File | Contents |
| --- | --- |
| `preferences.bed` | Global settings, startup policy/last folder and five saved layout defaults |
| `workspaces/<id>.bed` | One folder workspace's settings overrides, file visibility, layouts and active slot |

The workspace ID is the BLAKE3 digest of its canonical folder path's native encoded
bytes. Each record includes the root and checks it before restore/replacement.
Explicit folder opening canonicalizes off-thread and reuses an already-open
workspace. No JSON import runs: older JSON and project `.bed` files stay untouched.
`WorkspaceRoot` supplies browsing/cwd; only `FolderWorkspace` opts into persistence.
The default workspace keeps its settings, visibility and layouts in memory. Global
settings and explicitly saved global layout defaults persist normally.

Settings has no header or storage-location prose. Its sidebar contains:

- **General:** Open last workspace on open (Global only, default off), Autosave
  (Never, Change Focus, Timer), plus timeout for Timer.
- **Appearance:** Dark/Light theme, window opacity, macOS-only blur, panel opacity,
  border radius/gap, file-tree indentation and shader effects.
- **Editor:** Line numbers, smart wrapping and smart indentation, all enabled by default.

Startup policy is global because it is resolved before a workspace is opened.
With **Open last workspace on open** enabled, launching without a folder argument
reopens the last active folder workspace through normal session restoration,
including its layouts and active slot. An explicit folder argument takes precedence.
If the remembered folder is unavailable, startup reports the error and opens home.
The temporary home workspace does not replace the remembered folder.

Other values support workspace inheritance. Editing a Workspace value creates an
individual override; its reset icon resumes following Global. Unedited values
continue inheriting global changes. Switching workspaces applies effective
appearance to the window. Keybinding settings are deferred; Save, Open Folder,
Quit and layouts retain fixed Primary+S/O/Q and Primary+1–5 shortcuts. Primary is
Command on macOS and Control elsewhere. Widget-owned editing shortcuts retain
first refusal. The standalone Theme editor is retired.

**Shader effects** enables whole-window CRT processing, disabled by default.
Independent 0–1 sliders control scanlines, vignette, bloom, static noise,
curvature, pixel grid, phosphor persistence and brightness pulse. Values remain
editable and persist while effects are off. Each value supports the same
Global/Workspace inheritance and reset behavior as other settings; older records
default to effects off. There are no presets, jitter or chromatic aberration.

`effects.rs` and its embedded `effects.wesl` own the application rendering.
Bevy runs persistence, when enabled, then CRT processing after egui and before
UI2's platform alpha conversion and final output. Effects operate on
display-premultiplied color and return linear-premultiplied color, preserving
transparent coverage and embedded viewport composition. Off bypasses all effect passes.
History textures belong to the camera's render entity, are allocated only for
persistence, and reset on resize, effect-value changes and workspace/layout
switches. Decay uses real elapsed frame time rather than frame count.

Bloom downsamples to half width/height, then uses a separable five-tap Gaussian
blur with three bilinear samples per axis. Two camera-owned `Rgba16Float`
textures hold display-premultiplied color, so blur samples require no color
conversion. The final CRT pass adds one filtered bloom sample at the curved
source coordinate. Bloom runs after persistence and before curvature; zero
intensity or disabled effects release its textures and skip all three bloom
passes. Resizing rounds reduced dimensions upward, including odd window sizes.
The blur approximates the original radial kernel with a similar four-pixel
radius; it preserves flat-color gain and source alpha, but is not pixel-identical.

Before UI2 draws, bed2 maps egui pointer movement, button and touch coordinates
through the same display-to-source curvature formula used by the shader. Native
cursor coordinates stay unchanged. Stationary pointers reproject when curvature
changes or is disabled. Input and rendering capture the same effective settings
before drawing, so Appearance edits take effect together on the next frame.
At the curved border, input follows the shader's clamped source coordinates.

The native regression uses an isolated config directory and checks curved clicks
and slider dragging, GPU color/alpha, resize/layout changes, viewport composition
and restoration after restart:

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_effects.py
```

The opt-in native GPU regression compares flat colors, text-like edges and
transparent regions against the original shader, and measures the complete
bloom + CRT GPU time at 4K using render-pass timestamps:

```sh
cargo test --offline --locked -p bed2 native_bloom_quality_and_gpu_cost -- --ignored --nocapture
```

It requires a timestamp-capable GPU; timing excludes image upload/readback.
On 2026-10-22, the Apple M4 Pro/Metal synthetic 4K comparison measured median
bloom + CRT GPU time of 2.900 ms for the original and 0.447 ms for the reduced
pipeline (6.49× faster). This measures the effect passes, not whole-app frame
time or thermal behavior. Frame pacing remains unchanged.

Document autosave is independent of metadata persistence. Timer defaults to one
second since the last content edit (supported timeout: 0.01–86400 seconds).
Change Focus saves when leaving an editor's text canvas, including panel/layout/
workspace changes and OS focus loss. Canonical document IDs deduplicate viewers;
inactive and viewless documents remain eligible. Composition, pending saves,
conflicts and missing bindings defer/prevent writes. Saves use the existing
source-checked service. Failures remain visible and do not retry the same revision
indefinitely; manual Save remains available. Quit decisions suspend new autosaves.

**View → Layout** selects layouts 1–5 and offers **Save Current Layout** and
**Reset Current Layout**. Switching retains panel/document/terminal instances.
A slot first uses its current global default, then retains its workspace edits.
Saving a default captures geometry, auxiliary tool placement, selection and focus,
without file paths, Code viewers or process state. It does not rearrange existing
workspace layouts. Reset reuses matching tools in stable tab order, creates
missing tools, and appends remaining panels to the largest area (first area wins
a size tie). Existing documents, drafts, histories and workers retain their owners.

The built-in layout 1 is one full-area Terminal. Only that layout is created at
startup; layouts 2–5 start empty when first selected. Saved global layout defaults
continue to apply to new layouts. Startup waits for
global preferences and workspace restoration before composing default tools, so
an initial shell cannot block restoration. Installing untouched defaults does not
create a workspace file.

Global and workspace stores validate bounded reads, merge untouched setting
values from late reads, buffer changes for 250 ms, and atomically replace files
on background workers. Workspace capture runs after layout edits and before
workspace retirement, and again after ordinary updates. Writes outlive retired
entities. Malformed/unsupported files and late composition conflicts are preserved
with visible errors; repair/access retry revalidates disk before saving live work.
Unresolved policy keeps editor/file operations read-only and blocks new terminals.
The quit barrier drains final snapshots before cancelling managed work.

Workspace records restore geometry, panel kinds, file/tree descriptors, tab
order/selection and area focus. They do not recover unsaved content/history/drafts,
editor cursor/folds/scroll, tree expansion/selection, terminal processes/transcripts
or viewport orbit state. Save As, New/Untitled creation, unsaved-content recovery
and broader workspace-management UI remain later work.

## Close decisions and application shutdown

Destructive panel/area UI intentions arrive as `PanelCloseRequested`. `close.rs`
currently approves through `commit_close` only while quit is idle and rejects
viewer-local text composition. `close_targets` preflights the exact views removed
by joins/docks using the same geometry planner as commit. Closing a view retains its canonical document and
edits; this is distinct from discarding a workspace. Workbench revalidates the
captured layout stamp at commit. Raw entity despawn bypasses decisions and prompts,
though ownership/worker cleanup still runs.

Closing a clean application flushes buffered metadata/settings and shuts down
without a save-workspace question. Only dirty/conflicted/composing documents need a
Save All / Discard / Cancel decision, across all workspaces including retained
viewless documents. Cancel retains live state and workers. Persistence failures
have a separate Retry / Cancel path, never a workspace Save/Discard decision.
Native actions obey the same close-modal gate; pending folder results resume on
cancellation, not after the final shutdown barrier. Already admitted editing
follow-ups are observed before deciding documents are clean. Quit preflights
closure, drains persistence, cancels managed work, waits for cleanup, then exits.

The quit policy retains Workbench's document gates for future integrations:
Save All awaits admitted writes, failures or changed revisions return to the
decision, and pending writes/drafts prevent destructive shutdown. File-backed Code documents participate in Save All, discard and cancel decisions;
missing-source documents require explicit discard or recovery.

## Verification

```sh
cargo test --offline --locked -p bed2 --features automation
cargo check --offline --locked -p bed2
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
python3 bedtime2/tools/smoke_cube.py
python3 bedtime2/tools/smoke_tasks.py
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_code.py
python3 bedtime2/tools/smoke_editor.py
python3 bedtime2/tools/smoke_filetree.py
python3 bedtime2/tools/smoke_workspace_policy.py
```

The [automation protocol](../bedtime2/crates/bedtime-automation2/README.md) supports
stdin/stdout control, application input injection, runtime configuration, inspection
and rendered screenshots. Scenarios and assertions live outside `main.rs`.
The general scenario verifies sectioned settings, scope/reset behavior, layout
shortcuts, titlebar click/drag and rendered captures. The workspace-policy scenario
uses isolated config/home fixtures to verify real Timer/Change Focus autosaves,
Never/manual Save, temporary startup, centralized RON files and restart restoration. Generic snapshot/restore endpoints have been removed;
scenarios must use application-owned creation rather than `panel.open`. The other scenarios
inspect and exercise independent orbit viewports (`app.inspect_cube`), task and
process ownership/I/O (`app.inspect_tasks`), and real PTY sessions, input,
rendering, appearance and cleanup (`app.inspect_terminal`). Settings panels also
serve as cover tabs when checking hidden panels. Files/editor scenarios use temporary
projects to verify opening, previews, edits/saves, watchers, operations and cleanup.
The former Documents browser and its automation commands remain retired.
`app.inspect_menus` reports the shared tree and actual native installation;
`app.activate_menu` accepts a nested label path such as `["View", "Layout", "Layout 2"]` and revalidates the same action as a native
click. `app.open_folder` exercises the accepted directory-selection boundary, not
OS dialog clicking. `app.inspect_filetree` includes workspace roots, persistence
opt-in and settings/session readiness. `app.inspect_settings`/`app.set_settings`,
`app.inspect_layouts`/`app.select_layout` and `app.inspect_titlebar` expose the
application policy and actual titlebar geometry for automation. These are application operations, not a
reintroduced library panel registry.

Terminal support is initially macOS/Linux; native verification in this repository
has used macOS/Metal. The Windows task echo script and non-macOS native input
have not been verified here. Quit choices and worker-draining behavior have
application unit coverage; automation transport exit is separate from window quit.

Current workspace/menu policy verification is recorded in
[application composition](../MIGRATION/application-composition.md#follow-up-verification--2026-10-19):
the application and affected library suites passed, and the desktop policy scenario
verified native menus, actual home cwd, memory-only defaults, pre-quit autosave,
file/layout restore and clean quit. Files functional behavior passed; its full GPU
capture checks remain intermittent and unverified on the latest run. OS picker
clicking was not automated. Give each ordinary smoke a fresh `XDG_CONFIG_HOME` to
avoid modifying personal settings.

The following are historical results, predating the entity-first refactor; none
alone establishes verification of the current worktree.

Cleanup verified on 2026-10-18 on macOS/Metal: all five application quit tests,
normal bed2 compilation and the automation build passed. The four native scenarios
passed against the pinned `target/bed2-cleanup-smoke` executable: general workbench
(9 captures), viewport (8), tasks/processes (6) and terminal (15). Their summaries
and captures are under `target/bed2-cleanup-*-smoke` and
`target/bed2-cleanup-smoke-results`. Updated Python syntax and documentation links
also passed checks.


Tree-sitter/Structure verified on 2026-10-19: backend language/incremental tests,
editor feature suites, seven Structure lifecycle/navigation tests and the
application automation build passed. `smoke_code.py` passed eight native captures
for syntax, folding, Structure activation, shared views, placeholder, hidden-tab
reveal, automatic replacement focus and fold controls without line numbers.
Captures and summary are under
`target/bed2-code-smoke`. Its `app.inspect_code` command exposes current analysis,
view selections/folds and workspace source bindings without serializing buffers.

Files restoration verified on 2026-10-19 on macOS/Metal: 37 application tests,
100 filetree library tests, normal application checking and the automation build
passed. Native scenarios passed for file-backed syntax/Structure (8 captures,
`target/files-code-smoke`), editing/saving/shared views including a 112 MB document
(3 captures, `target/restored-editor-smoke`), and filetree operations, watching,
preview routing and persisted preferences (11 captures,
`target/restored-filetree-smoke`). Python syntax, Rust formatting and local
documentation links also passed.
