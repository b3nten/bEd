# bEd

bEd is a Rust desktop text editor with dockable tools, SSH projects and
embeddable document views. It uses a custom byte buffer with multi-cursor editing
and undo, Dear ImGui for its interface, and winit/wgpu for native windows and
rendering.

```sh
cargo run --locked -- path/to/project path/to/file.rs
```

Run without paths to resume the last workspace, or use `--new-window` for the
project picker. Recent projects remember their open
files, independent views, tool panels, terminal working directories and docking
layout. Documents, Files, terminals, Debug, Settings, Search, References, Diagnostics,
Structure and Language Servers are ordinary ImGui tabs. Drag tabs between groups, split
panels or detach them into native windows. macOS has native menus through Muda.
On macOS, **File → New Window** (Cmd+Shift+N) or **New Window** in the Dock
icon's right-click menu launches a separate bEd process at the project picker.
Instances share settings and recent projects; each has its own editing session.
Switching projects clears the previous workspace's docking and transient UI state.
Saved layouts discard obsolete tabs and are validated before loading; invalid
docking data falls back to a default arrangement while retaining restorable panels.
The main titlebar and native window title show `bEd • workspace_name`, using the
workspace's display name in operating-system window lists as well.
Titlebar panel buttons and panel menu clicks create new panels. New panels and
opened files join the largest dock group; keyboard shortcuts reveal existing
tools. Separate Search tabs keep independent queries and results.
The titlebar offers core panel buttons, Structure, and Open Image with uniform spacing.
Plugins can contribute more buttons and application menu commands. Splits duplicate the active document view,
including when a tool panel has focus.
Closing a detached window closes its contained tabs; a failed save or cancelled
Save As keeps the group open.
Right-click a tab to close it, all tabs in its group, other tabs, or tabs to its
left or right. Panels also persist when no project is open.
Right-click in an editor and toggle **Show Minimap** to change only that tab;
the override expires when the tab closes.
Restored terminals start fresh shells in their recorded launch directories;
live processes, scrollback and subsequent shell `cd` changes are not restored.
Terminal labels follow the foreground process on macOS and Linux, and terminal
titles supplied by remote shells or programs.

Panels fade in when opened or revealed, and menus and floating popovers grow
subtly into place.
Structure entries appear with a short stagger; expanding or collapsing branches
in Files and Structure fades their children and smoothly adjusts the row spacing.
Explicit navigation jumps, including Go to Line, definitions and Structure
selections, scroll smoothly. Wheel and trackpad scrolling and cursor following
use the same native behavior as Files and Settings. Disable **UI Animations** in
Settings to turn off panel, tree, popup and navigation motion.

Structure shows a nested outline of the most recently focused document, including
unsaved edits. Open it from the titlebar, the macOS View/Window menus, or the
editor's right-click menu. Click a name to focus its editor view and jump to the
source; click an arrow to expand its children. Structure supports all bundled
Tree-sitter languages, including JSON keys, TOML sections, HCL blocks, HTML
elements and CSS rules. It parses local buffers for local and SSH files and
works with syntax highlighting and language servers disabled. Panel instances
and docking are restored with the workspace.

Right-click in Files to toggle **Hide Gitignored Files** or **Hide Hidden Files**
(dot-prefixed names), or right-click a file or folder and choose **Hide from File
Tree**. Choices are saved per project in `~/bed/config/workspaces.json`, separately
for local and SSH projects; both filters start off. **Show Hidden Files** temporarily
reveals filtered entries dimmed, with **Unhide from File Tree** for manually hidden
paths. Hiding a folder covers its subtree. These controls affect only the tree.
The Git-ignore filter needs Git installed on the project’s machine. SSH directory
metadata and byte documents use protocol v3; bEd automatically installs the matching helper.

A document can appear in several views. Its text, undo, autosave, highlighting,
Git and LSP services are shared; cursors, selections, find and scrolling belong
to each view. Named files autosave after one second of inactivity by default;
Settings provides an **Autosave code files** toggle and **Autosave delay** control.
External disk
changes reload clean buffers; dirty buffers offer Reload, Keep Buffer or Save As.

File opening checks registered plugin extensions first; registration order resolves
overlapping claims. The bundled image plugin handles PNG/APNG, JPEG, SVG, GIF,
WebP, BMP, ICO, TIFF, TGA, PNM (PBM/PGM/PPM/PAM), QOI, DDS (DXT textures), HDR
and OpenEXR; the model plugin handles GLB, glTF and STL; the font plugin handles
TTF, OTF, TTC and OTC. The audio plugin handles WAV, MP3, FLAC, Ogg/Vorbis,
AAC/M4A, AIFF and CAF audio.
The CSV plugin opens CSV and TSV files as editable tables backed by the same
text document as Text Editor. Open With can show both views together; edits,
undo, save, autosave and SSH file handling are shared.
Other files open in the text editor or the built-in hex editor according to
their content. Files → right-click → **Open With** selects a viewer explicitly.
Image, model, font, audio and hex views share the same exact-byte document.
Switching between text and bytes saves
and closes its views before reopening; a cancelled or failed save keeps them open.

The hex editor shows offsets, hexadecimal bytes and ASCII. Click or Shift-click
bytes to select, type hexadecimal digits to overwrite, use Insert to toggle
insertion, and Backspace/Delete to remove bytes. Copy, cut and paste use hexadecimal
text. Undo/redo and save use the usual shortcuts. Byte documents preserve BOMs,
line endings and arbitrary bytes, and share save/autosave/conflict handling with
text documents; their undo history stays in memory. The image viewer supports
Fit, 100%, zoom and pan, plus an information popup and a default-fit setting.
Animated images show a still preview. SVGs render at their document dimensions
with transparency, system fonts and embedded images; external image paths are
ignored. Raster decoding and SVG output share a 64 MiB decoded-pixel limit.
The audio viewer shows a waveform for each channel, play/pause, restart, volume,
elapsed/total time and a seek slider. Click or drag the waveform to seek; Space
toggles playback while the viewer is focused. Opening audio does not start playback.
Decoding runs in the background with a 256 MiB decoded-sample limit. Position and
volume are restored with the workspace; closing the view stops playback.

The CSV viewer keeps column headers visible while scrolling and shows source
record numbers, including in sorted or filtered views. Select cells by clicking,
dragging or Shift-clicking; use row numbers and column headers to select whole
rows or columns. Double-click, Enter or F2 edits a cell; typing replaces its
contents. Enter commits, Tab commits and advances, and Escape cancels. Expand a
cell to edit multiline text. Values remain literal strings, preserving leading
zeros and formula-looking text.

Copy, cut and paste use quoted tab-separated ranges suitable for spreadsheets.
Context menus clear cells, insert/delete rows or columns, and rename headers.
Each operation is one shared undo step. Paste follows visible row order and
leaves filtered-out records untouched. Paste can expand the original table;
sorted or filtered views reject ranges extending past their visible rows. Clear
sorting and filters before inserting rows.

**Find** visits matching visible cells. Global and per-column filters combine
with AND and match text without case sensitivity; column filters support
**contains**, **equals** and **is empty**. Click a header to cycle ascending,
descending and original order, with **Auto**, **Text** or **Number** sort mode.
Sorting and filtering change only the view, keeping the saved row order intact.
**Format…** overrides detected comma/tab/semicolon/pipe delimiters and whether
the first record is a header. Interpretation controls do not change the file.
View settings restore with the workspace.

Parsing and filtering run in the background, with clipped table rendering for
ordinary exports around 100,000 rows. CSV editing supports UTF-8, quoted fields,
escaped quotes, embedded newlines and ragged records without rewriting untouched
fields. The viewer limits its index to 128 MiB and tables to 510 columns, in
addition to the editor's 128 MiB file limit. Malformed or unsupported files offer
**Open in Text Editor**. Formulas, row/column reordering, typed/date filters and
filtered-data export are not included.

Hover over a file-tree row to see its type, exact and human-readable size,
last-modified time (UTC), Git status and link target where applicable. Executable
and object files also show architecture and embedded debug information or external
debug-file references. Inspection runs in the background, caches results for five
seconds and works in local and SSH workspaces.
The Model Viewer uses Bevy to render static, self-contained glTF/GLB models and
ASCII or binary STL meshes. glTF materials support metallic/roughness shading,
normal maps, emissive maps, baked ambient occlusion, vertex colors and transparency.
The **Appearance** menu offers shaded, wireframe, wireframe overlay and normal-color
views, with optional normal vectors. Lighting presets include two embedded Poly
Haven HDRIs, Studio Small 08 and Kiara Dawn, which work offline for both lighting
and skyboxes. The horizon control lowers or raises the background independently
of the model. Skybox blur softens the background while preserving model lighting
and reflection detail. Shadows, exposure and screen-space ambient occlusion are adjustable;
screen-space AO requires a compatible GPU. Drag to orbit,
right/middle-drag to pan, scroll to zoom, and double-click or choose **Frame All**
to reset the framing. Camera and appearance state are restored with the workspace. Export GLB or
glTF with embedded buffers and PNG/JPEG textures. Draco-compressed meshes are
decoded in Rust, and skins are shown in their authored pose with up to four
joint influences per vertex. Companion files, morph targets and animation playback
are not supported yet. STL loading uses `bevy_stl` and a neutral material.

The font viewer opens TTF, OTF, TTC, OTC, WOFF and WOFF2 fonts, with editable
sample text, size and direction controls,
ligature and kerning toggles, a scrolling glyph browser with enlarged previews and
font-unit metrics, collection face selection, and font metadata and licensing
text. Jump to a glyph by character, `U+0041`, or numeric glyph ID; unencoded
alternates and ligatures are included. Copy encoded characters from the inspector.
Drag or scroll the sample preview to pan. View settings are restored with the
workspace, and font previews refresh after edits in a shared hex view, including
for SSH files. HarfRust shapes each sample line as one script/direction run;
FreeType rasterizes it independently of the editor font atlas. Wuff decodes webfonts
in memory on the preview worker; hex editing and saving preserve the original
compressed file. Samples are limited to 16 KiB and 256 lines, and font files to
64 MiB before and after decoding. Automatic mixed-script/bidi paragraph layout,
fallback fonts and variable-axis controls are not supported yet.

SSH projects use the same local editor: typing, undo, selections and highlighting
stay on your machine. Files, search, Git, language servers and shells run on the
remote host. In Projects, enter an SSH host or config alias and a project path
such as `~/Dev/foo` or `/srv/foo`. bEd resolves `~` using the remote account's
home directory and uses the folder name as the display name. Recent workspaces
can be renamed and retain separate layouts for each local/SSH target and root.
Existing saved local projects migrate automatically.

Desktop packages include prebuilt Linux x86-64 and ARM64 helpers. On first
connection, bEd detects the remote CPU, uploads the matching `bed-headless` to
the SSH user's `~/.cache/bed/helpers/`, and launches it there. Later connections
reuse the same helper; a different bundled binary installs alongside the old
one automatically. Uploads commit atomically after validating the executable.
No sudo, remote download, compiler or PATH changes are needed.

bEd manages the helper automatically. It invokes your system `ssh`, so existing
SSH config, keys and authentication agents apply. No
listening TCP port or additional credentials are configured by bEd. Language
servers and Git must be installed remotely; LSP configuration remains local, and
configured executable paths refer to the remote host.
Running an installed helper needs no Rust or C compiler, and connecting never
builds software or installs toolchains. Building it from source needs Rust and a C compiler
for the existing libgit2 dependency. Project builds and language servers retain
their own toolchain requirements.

Source checkouts use helpers under `target/remote-helpers/<Rust target>/`;
`BED_REMOTE_HELPERS_DIR` can select another bundle directory. Copy the CI
`bed-remote-helper-*` artifacts there, or stage a native Linux musl build with
`python3 scripts/lib/remote-helpers.py stage --target aarch64-unknown-linux-musl`
(use `x86_64-unknown-linux-musl` for x86-64). Cargo does not build these target
binaries automatically when running the desktop from source.

The supported desktop build platforms are macOS and Linux. Packaging has two
entry points:

```sh
bash scripts/package-macos.sh
bash scripts/package-linux.sh
```

Run the script for the current platform. Both build the release desktop binary,
prepare missing static Linux x86-64 and ARM64 SSH helpers, and write packages to
`target/dist`. macOS produces `bEd.app` and a ZIP; Linux produces a tar archive
and a Debian package when `dpkg-deb` is available. Add `--install` on macOS to
install `/Applications/bEd.app`.
Packaging requires Rust, Python 3 and the platform's native build tools
(Xcode Command Line Tools on macOS; ImageMagick and the development libraries
listed in `.github/workflows/ci.yml` on Linux).
Building missing SSH helpers requires a running Docker engine; prebuilt helper
bundles avoid that dependency. The installed app and remote servers do not need
Docker or a compiler.

To package an existing desktop binary and validated helper bundles:

```sh
bash scripts/package-macos.sh --skip-build
# Or, on Linux:
bash scripts/package-linux.sh --skip-build
```

Alternatively, extract both matching `bed-remote-helper-*` CI artifacts into
`target/remote-helpers` before packaging; that avoids a local Docker build.
Shared packaging implementation lives in `scripts/lib`, and the Linux desktop
CI driver lives in `scripts/ci`. Run their checks with
`python3 -m unittest discover -s scripts/tests`.

`assets/` holds the editable icon project, its exported PNG and sample models
used by tests. These source files are excluded from packages. `resources/`
holds runtime fonts, interface icons, configuration and highlighting queries.
Embedded model-viewer lighting files live in that crate's `resources/` directory.
Packaging copies runtime resources, generates the macOS ICNS or Linux launcher
PNGs from the icon export, and installs icons, desktop metadata, licenses and
SSH helpers in their platform locations. Generated files stay in the build output.
After editing `assets/bEd.icon` in Icon Composer, update
`assets/bEd-iOS-Default-1024@1x.png` with a new 1024-pixel export before packaging.

Remote saves are asynchronous and check the previously loaded disk baseline.
They preserve BOM/line-ending bytes and clear dirty state only after the relevant
version is acknowledged. Closing a tab during a save keeps it open; retry after
the save finishes. Losing SSH retains local buffers and undo; **Reconnect SSH**
starts a fresh helper and checks for external changes. Remote terminals require
an explicit restart after disconnection and do not survive closing bEd. Remote
undo history is in memory, and destructors never save documents or history.

This first version uses one root per workspace, supports UTF-8 paths within the
selected root. Local and remote text or binary files up to 128 MiB can be edited. Larger
files show an explicit size-limit error without loading partial content; open
them in another editor or split them into smaller files. Project Search skips
files above this limit. Remote file
deletion asks for permanent deletion rather than using your desktop Trash. Open
and Save As accept remote paths in bEd. Editing local configuration files from a
remote workspace requires switching to a local workspace; Settings controls work
in either. Local and remote workspaces retain separate layouts and documents.

Right-click text for editing and language-server actions. Right-click Files
entries or empty tree space to create files/folders, rename or move to Trash.
Names cannot replace existing entries. Open documents follow renamed paths;
trashed documents stay in memory with autosave paused until explicitly resolved.
Go to Definition jumps directly when there is one result; multiple definitions
open the navigation list. Cmd-click text on macOS (Ctrl-click elsewhere) requests
the definition at the clicked position.

Project Search respects Git ignore rules by default and always skips Git
metadata. Enable Include ignored files to search ignored build/output files.
Folders outside Git include all regular files. Progress and matches arrive
while searching; Cancel stops the scan. Broad queries stop at 100,000 matches
and display the result limit.

Sharp is the default effect preset: subtle scanlines/vignette/bloom with no
jitter, pixelation, color shift, pulse or temporal blur. Off, Legacy and Custom
are available in Settings. Picking Sharp preserves existing profile values;
Customize copies the visible parameters before editing them.

Standalone settings and keybind defaults are seeded into `~/bed/config`.
`--config-dir DIRECTORY` selects another location.
`bed.json` selects the active settings profile. Existing `ned.json` settings
migrate automatically, preserving the selected profile and the original file.
`effects.json` stores the chosen preset; `workspaces.json` stores recent projects
and per-project layouts. Existing theme, keybind and LSP formats remain supported.

Useful shortcuts:

- Cmd/Ctrl+O: open file; Cmd/Ctrl+S: save; Cmd/Ctrl+Shift+S: Save As.
- Cmd/Ctrl+P: find file; Cmd/Ctrl+,: Settings; Cmd/Ctrl+1–9: focus a tab.
- Cmd/Ctrl+Z: undo; Cmd/Ctrl+Shift+Z: redo, including unnamed documents.
- Alt+Up/Down: add caret; Alt+Left/Right: move by word.
- Cmd/Ctrl+F: find; Cmd/Ctrl+Enter in Find: select all matches.
- Cmd/Ctrl+Shift+F: project Search; Cmd/Ctrl+;: go to line.
- Cmd/Ctrl+D: definition; Cmd/Ctrl+R: references; Cmd/Ctrl+I: symbol info.
- Cmd/Ctrl+T: reveal a terminal panel. File/View/Window menus expose new
  documents, terminals, duplicate views, splits, panel tools and layout reset.

Language servers are configured in `~/bed/config/lsp.json`. Rust discovery
checks configured paths, PATH and rustup shims, with the project directory and
inherited environment passed to the server. Language Servers shows executable
resolution, startup errors and stderr, with a Restart action. A workspace keeps
one client per language instead of stopping one server when another file opens.
The server can initialize before Cargo workspace analysis finishes. Loading and
indexing progress appears above the document, in its context menu and in Language
Servers; diagnostics arrive as analysis completes.
Install Rust's server separately with `rustup component add rust-analyzer
rust-src`; `rust-src` enables standard-library analysis.

## Debugging

The dockable **Debug** panel launches local Rust and C/C++ programs with
`lldb-dap` on macOS and Linux. Install an LLVM 18 or newer adapter separately;
bEd does not download LLDB. Adapter discovery checks a configured executable,
then `PATH`, then `xcrun --find lldb-dap` on macOS. An installed `lldb` command
alone is insufficient: the toolchain must also provide `lldb-dap`.
See [LLDB's adapter setup documentation](https://lldb.llvm.org/use/lldbdap.html)
for toolchain and package options.

Open **Debug** from the titlebar or application menus and create a named launch
profile. A **Manual** profile selects a binary built with debug symbols and an
optional shell build command, such as `make debug`. A **Cargo** profile discovers
workspace packages and binary, example, unit-test and integration-test targets
from a manifest. Choose the target and feature settings; bEd builds it and reads
Cargo's artifact messages to find the resulting executable, including hashed
test binaries. Cargo test profiles provide an optional test-name filter.

Rust pretty printers load automatically for Cargo profiles and workspaces with a
`Cargo.toml`, using the project's Rust toolchain. Locals show readable Rust types
such as `String`, `Vec` and `HashMap`, with expandable contents. Watches and hover
show formatted summaries. For a standalone Rust binary, set
**Advanced → Rust pretty printers** to **Enabled**; choose **Disabled** to turn
them off. If the toolchain's pretty printers are unavailable, debugging continues
and **Build Output** explains why. Struct fields load when expanded; bEd disables
Rust's recursive struct summaries so large application values remain inspectable.
Locals load first; expand Globals or Registers to inspect those scopes. A failed
variable load shows its error and a **Retry** button.

Set program arguments, working directory, environment overrides and optional
source-directory mappings in the profile. Arguments are passed literally to the
program. An empty working directory uses the workspace root; build commands also
run from that root. **Stop on Entry** starts enabled. Profiles, the selected
profile and watch expressions are remembered per workspace.

Click the breakpoint gutter beside a source line, or press **F9**, to toggle a
breakpoint. The **Breakpoints** tab lists locations and lets you navigate,
disable, enable or remove them. Breakpoints are shared by views of the same file
and last until the project is closed or bEd exits. They survive program restarts
but are not saved with the workspace.

**Start** saves modified named files, runs the configured build, then launches
only if the build succeeds. **Build Output** keeps build progress and failures.
The program uses a **Program: <executable>** terminal for interactive input and
output; **Show Program Terminal** reveals it. Debugger expressions and LLDB
messages appear in Debug's **Console** tab. At a stop, bEd reveals the source
line and **Inspect** shows threads, stack frames,
expandable variables and watches. Select a frame to navigate and inspect it.
Hover a simple source expression while paused for its runtime value. The
**Console** has separate Expression and LLDB Command modes.

**Stop on entry** can pause in the system loader before application code runs.
Such frames may have no local source; press **F5** to continue to a breakpoint.
Turn off **Stop on entry** to run directly to a breakpoint.

- **F5:** Start or Continue; **Shift+F5:** Stop.
- **F9:** Toggle Breakpoint.
- **F10:** Step Over; **F11:** Step Into; **Shift+F11:** Step Out.

The panel also provides Pause and Restart. Restart stops the program, rebuilds
and launches again. Function keys keep their terminal behavior when a terminal
has focus. Closing Debug leaves the session running; its active-session
indicator reopens the panel. Switching projects or quitting stops the program.
Closing its program terminal hides that tab; **Show Program Terminal** reopens
the same terminal. Stopped program terminals retain their output and do not
restart as shells.

Editing source during a session marks it as changed from the launched build.
Execution highlights and runtime hovers are suppressed for changed files, and
new or moved breakpoints wait for restart. Disable and remove existing
breakpoints at any time. Source mappings pair a build-time directory with its
local directory; unavailable source does not prevent stack or variable
inspection.

The initial debugger supports one launched session per local workspace.
SSH debugging, attaching to existing processes, core dumps, conditional
breakpoints, logpoints, memory/disassembly views and editing variables are not
included.

## Embedding

Embedding uses document views with an explicit service configuration. The host
owns its context, fonts/style, clipboard, containers, windows and frame/GPU
lifecycle. The session can own autosave, monitoring, history, Git, highlighting
and LSP; default options enable no disk/process services and seed no config.

```rust
use bed_document_session::EditorSession;
use bed_editor_ui::{EditorView, EditorViewOptions};

let mut session = EditorSession::new();
let document = session.create_document(b"Hello, bEd\n")?;
let mut view = EditorView::new(&mut session, document)?;
// In the host's existing ImGui frame and chosen container:
let response = view.draw(ui, &mut session, &EditorViewOptions::default())?;
// Poll services once per host iteration; handle actions/errors explicitly.
let report = session.tick();
```

Call `session.request_focus(view.id())` when the host focuses a view. Use
`EditorSession::with_options(SessionOptions { .. })` to opt into services, and
explicit Save/Discard/Cancel close policies. Dropping views or sessions does not
save documents or history. Use `bed-document-session` and `bed-editor-ui` directly.
The [embedding tests](crates/bed-editor-ui/tests/embedding.rs) cover multiple views
of shared documents inside host-owned containers and frames.
Persistent history and LSP options require an explicit `project_root`.
`ViewResponse.definition_request` carries Cmd/Ctrl-click navigation with the
clicked row and UTF-8 byte column. The embedding host decides how to resolve
the request and display its destination.

```sh
cargo fmt --all --check
python3 scripts/check-crate-boundaries.py
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo test --workspace --locked --all-targets
cargo test -p bed-effects --locked --lib native_shader -- --ignored --nocapture
cargo test -p bed-plugin-gltf --locked native_ -- --ignored --nocapture
cargo test -p bed-workbench-api --locked --features gpu native_shared_target -- --ignored --nocapture
cargo run --locked -- --platform-smoke --lifecycle-smoke \
  --config-dir /tmp/bed-native-config path/to/project path/to/file.rs
cargo run --locked -- --viewports-smoke \
  --config-dir /tmp/bed-viewport-config path/to/project path/to/file.rs
cargo run --locked -- --menu-smoke
cargo run --locked -- --plugin-smoke
cargo test --locked -p bed-editor-ui --test embedding
```

Native checks need a desktop session and graphics backend. `--capture-frame
OUTPUT.ppm --capture-after-frames 30` captures GPU output. macOS is verified
first; native Linux CI is configured and must pass before claiming Linux runtime
acceptance. The pinned viewport backend supports native undocking on macOS and
X11; Wayland retains internal docking/floating.

To measure shader cost on your GPU, run:

```sh
cargo run --release --locked -p bed-effects --example profile_effects -- 3024 1964
# Optionally include a settings profile as the third argument:
cargo run --release --locked -p bed-effects --example profile_effects -- 3024 1964 ~/bed/config/solarized-light.json
```

Dimensions are physical pixels. The profiler uses GPU timestamps, discards a
warmup batch, and reports median/p95 times over 300 frames per configuration,
including comparisons with bloom disabled. It measures postprocessing alone in
continuous batches; UI rendering, presentation, CPU work and power consumption
are excluded. GPU clocks and other graphics workloads can affect the results.
The editor redraws even while idle to animate effects, at `fps_target` when
focused and `fps_target_unfocused` otherwise. High FPS targets increase GPU work;
lowering bloom intensity does not reduce its 25-sample cost unless it reaches zero.

bEd's original buffer and editing algorithms were translated from
[nealmick/ned](https://github.com/nealmick/ned). Original source credits and
retained asset notices are recorded in [NOTICE](NOTICE) and accompanying licenses.
Translated terminal portions retain the upstream
[Business Source License](LICENSES/terminal-adapter-BSL-1.1.txt).
The project has not switched to GPLv3: the terminal translation's current
non-commercial terms require permission or an independent replacement first.
Changing the application license would still require retaining third-party
license and attribution notices. `NOTICE` records those components;
the active dependency versions are recorded in `Cargo.lock`. Normal development
uses Rust tests and committed regression fixtures; it does not require the
original C++ checkout or port-verification tooling.

## Workspace architecture

The `bed-workbench` crate owns docking, focus, shared services, workspace
storage, command dispatch and panel composition. The root `bed` package owns
native windows, clipboard and rendering, and selects built-in features in
`src/builtins.rs`. It supplies those instances through `WorkbenchModules`.
All panels are registered by modules, including the text/hex editor,
language tools, debugger, file explorer, project search, project picker, settings
and terminals. Built-in modules use the same hosting contracts as linked plugins.
Reusable components live in `crates/`, with concrete APIs and one workspace lockfile:

| Crate | Owns |
| --- | --- |
| `bed-editing` | Custom text buffer, state, editing commands, selections, events, undo, UTF-8/path helpers and IDs |
| `bed-files` | Bounded reads, monitoring, file discovery/filtering and cancellable project search |
| `bed-highlight` | Tree-sitter grammars, queries, incremental spans, themes and workers |
| `bed-lsp` | JSON-RPC/process transport, synchronization, diagnostics and workspace language servers |
| `bed-debug` | LLDB DAP transport, debugger sessions, asynchronous builds and Cargo target discovery |
| `bed-document-session` | Shared document/view registry, save/autosave, history, Git and service coordination |
| `bed-ui` | Shared control/popup styling, tree animation, readable colors, CPU icon rasterization and file-icon contracts |
| `bed-editor-ui` | Embeddable text/hex widgets, input, find/line-jump, minimap, diagnostic painting and concrete editor extension contracts |
| `bed-settings` | Authoritative profiles, persistence, keybindings, appearance policy and font-atlas configuration |
| `bed-workbench-api` | Module/panel lifecycle, registration, snapshots, host requests, scoped document/terminal/dialog services and optional GPU canvases |
| `bed-module-editor` | Text/hex panels, editor commands and menus, language-server presentation, diagnostics, references and language-server panels |
| `bed-module-debug` | Debugger feature state, inspection panel, commands, terminal coordination and source-debugging editor contributions |
| `bed-module-explorer` | File-tree and file-finder presentation, file inspection and tree extension points |
| `bed-module-search` | Independent project-search panels and their result workers |
| `bed-module-terminal` | Terminal panels over the shared terminal session service |
| `bed-module-projects` | Project-picker presentation and workspace-opening requests |
| `bed-module-settings` | Settings panel UI over the shared settings service and registered settings sections |
| `bed-terminal` | Grid/parser, PTY workers, shell sessions, input/rendering and fonts |
| `bed-effects-config` | Renderer-independent appearance parameters and preset serialization |
| `bed-effects` | wgpu shader passes and viewport postprocessing |
| `bed-remote` | Versioned RPC, SSH transport and automatic helper deployment |
| `bed-headless` | Remote filesystem, search and Git services without a GUI |
| `bed-plugin` | Compatibility reexports of `bed-workbench-api`, including the `Plugin`/`PluginPanel` names |
| `bed-plugin-structure` | Source outline panel and its worker coordination |
| `bed-plugin-image` | Read-only raster/SVG panels, bounded background decoding and GPU drawing |
| `bed-plugin-gltf` | Static GLB/embedded glTF and STL loading, orbit cameras and Bevy PBR rendering |
| `bed-plugin-font` | HarfRust sample shaping, FreeType previews, glyph browsing and font inspection |
| `bed-plugin-audio` | Audio decoding, channel waveforms and native play/pause/seek controls |
| `bed-plugin-csv` | Shared-text CSV/TSV table editing, background indexing, sorting and filtering |
| `bed-workbench` | Application shell, module hosting, docking, document/workspace lifecycle and service adapters |
| `bed` | Native application startup, winit/wgpu integration, platform chrome/menus and built-in module composition |

Editing, files, highlighting, LSP, debugger backend and document-session crates
compile without GUI backends. Generic `bed-ui` has no editor-widget, document-session
or LSP dependency. `bed-editor-ui` depends on Dear ImGui and shared document
services, without workbench panels, native windows or a direct LSP dependency.
`EditorFrame` coordinates one editor widget's drawing; application frames and
docking belong to the host and workbench. Language-server requests, navigation
results, hover interaction and dashboards live in `bed-module-editor`.
The workbench may use native file dialogs and Trash, while the native application
owns window/event-loop and renderer backends. Settings and rendering share
appearance parameters through `bed-effects-config`, so settings does not require
the renderer. The terminal crate keeps its parser/PTY and presentation modules
together; disabling its default
`ui` feature provides the parser and PTY services without Dear ImGui.
Its translated portions retain the Terminal Adapter license recorded in NOTICE.

`EditorSession::with_view` supplies scoped commands, mutable view state and
read-only document/service access. Text edits go through commands; saves,
path changes and document replacement go through the session. The scope restores
view ownership and transforms sibling selections even if a consumer panics.
`EditorView::presentation()` exposes geometry/hover data for host menus and
LSP widgets without exposing mutable frame state. The editor module's LSP widgets
take explicit `bed_module_editor::presentation::LspPresentationOptions`.
File/search panels wrap GUI-free discovery services and return navigation actions
to the application.

`ModuleRuntime` registers the injected built-in features and linked viewers
together. Every tab contains a `HostedPanel` created through the registry; the shell uses the same
creation, drawing, focus, close and restoration path for native and plugin panels.
A module owns feature state independently of its panels; closing the debugger
panel does not stop a debug session. Built-in feature implementations live in
their module crates. The native application decides which modules to load; the
shell does not construct linked viewers itself. Its viewer dependencies used
by regression fixtures are development dependencies.
`Module` supplies registration, commands, background ticks, document events,
workspace persistence and shutdown. `ModulePanel` supplies drawing, focus actions,
document attachment, view-state restoration and persistence eligibility. Panel
metadata declares singleton behavior, initial layout and placement for newly opened
instances, so the shell can host a feature without owning its controller.
Legacy text/hex viewer IDs and saved panel
kinds remain supported.

The explorer module owns its tree and file finder. Each search panel owns its
query, results and worker. The project picker owns its UI and submits concrete
workspace actions. Settings panels borrow the authoritative application settings;
profile changes, persistence and native menu updates continue to use that shared
state. Terminal panels render shared terminal sessions. Closing a terminal panel
respects sessions retained by another feature, and command transcripts are omitted
from workspace restoration.

Implement `bed_workbench_api::Module` to register namespaced commands, panel factories,
file viewers, toolbar buttons, application/file/folder/tree-background/selected-text
menu entries, and settings sections. Panels implement `ModulePanel` and can declare
an attached document so the host includes them in saving, closing and restoration.
Registration validates module namespaces, unique contributions and aliases, one
fallback viewer per document kind, and unambiguous legacy panel kinds before
adding any contribution. `HostContext` provides immutable document snapshots,
settings, read-only LSP diagnostics and texture handles. Commands receive a
`CommandContext` captured at their originating UI surface. `HostRequest` queues
edits, navigation, document commands, panels, file dialogs and resource changes;
modules never need a mutable workbench. Native modules can also use scoped
`ModuleServices` for session operations, terminal launches and file picking.
The host retains document ownership and supplies platform adapters for terminals
and dialogs. Optional native services use `ScopedServices`: the host lends concrete
types for one callback, and taking a service removes it from that scope so distinct
services can be borrowed together safely. Settings panels use this to edit the
actual `Settings` service, with registered sections supplied through the concrete
`SettingsContributions` interface. Existing `bed_plugin::Plugin` and `PluginPanel`
names alias the same contracts for linked plugins.

Feature-specific extension APIs live with the feature. The editor's
`bed_editor_ui::extensions::SourceDebugExtension` supplies breakpoint/execution
presentation, breakpoint actions and runtime hover rendering. The debugger
registers one provider with `EditorExtensions`; all relevant text views consume
it without the workbench interpreting debugger decorations. Registrations are
weak, and callbacks run after releasing the editor's document borrow. This is a
concrete editor capability, rather than a protocol every panel must implement.
Hover callbacks retain the originating editor child's UI scope; hidden views and
presentations whose document changed during drawing do not receive callbacks.
The editor also exposes `EditorMenuExtension` with captured document selections,
and the explorer exposes `TreeMenuExtension` with the originating file or folder.
Features can add their own typed extension points. There is no universal
module-to-module message bus or dynamic plugin loader.

The optional `ModulePanel::action` hook handles focused Find, Go to Line,
Select All, Undo and Redo commands. Before saving or closing, the host sends
Commit Edit to attached panels and applies their revision-checked edits before proceeding; a failed
commit retains the panel and its draft.
Panels can submit `ApplyEditsWithResult` with an `EditToken` and receive
`ModulePanel::edit_result` acknowledgements, keeping pending input until the
session accepts the edit or reports an error.

`EditorSession::apply_edits` takes byte ranges and an expected document revision.
The whole transaction is validated before mutation and becomes one undo unit.
Text transactions retain selection, highlighting and LSP updates. Byte transactions
use exact splice history and skip text services. Existing text-only embedding APIs
retain their behavior; `open_file_auto` and `open_file_with_kind` opt into byte
documents.

Direct embedding through `bed-document-session` and `bed-editor-ui` does not
require the workbench or module host. `bed-editing` owns the GUI-independent
editing model and history; it is shared by text, hex and table panels.
`bed-editor-ui` draws those documents, and `bed-module-editor` adds workbench
panel lifecycle, editor menus and language-service interaction.

Panels receive the host's Dear ImGui `&Ui` in `draw` for controls, popups and
input. With `bed-workbench-api`'s optional `gpu` feature, `render_output` describes a
host-owned color target, optional depth attachment, physical size and content
revision. The host calls `render` with its wgpu instance, adapter, device, queue, encoder and target
after UI/input and before submitting any viewport. Plugins own their pipelines
and source resources; a device generation identifies when to rebuild them.
The shared `gpu::Canvas` presents the target and handles viewport DPI sizing.
Outputs redraw on content changes, resize or device recreation, and are released
when their panel closes. Native windows remain host-owned. Embedded renderers
may submit their own commands on the shared queue during `render`, before the
host submits its canvas and UI commands; they must not depend on commands still
pending in the supplied encoder. Embedded renderers restore the host's device
callbacks after initialization and forward any device loss so host recovery
continues to rebuild all plugin resources.

Image and model viewers use this same GPU output path. Image decoding stays on a worker;
the plugin uploads a source texture once per image/device and draws zoom/pan
into the canvas. Decoded image pixels and each output color target are limited
to 64 MiB. Model loading also runs on a worker, with bounds on buffers, decoded
textures, vertices, indices and nodes. Draco decoding is bounded to 1,000,000
vertices, 3,000,000 indices and 64 MiB of expanded geometry. glTF textures allow
64 MiB per image and 128 MiB in total. Bevy shares the host's wgpu device and
renders directly into its target; the host registers that texture with ImGui.
STL input is limited to 64 MiB and 333,333 facets (fewer than 1,000,000 expanded
vertices). Both formats load from document snapshots in local and SSH workspaces.
Neither viewer passes CPU output pixels
through the host request API.

Run focused suites with `cargo test -p bed-editing`,
`cargo test -p bed-document-session`, `cargo test -p bed-editor-ui` or another crate
name. Workbench integration fixtures live in `tests/unit/workbench`; native
application fixtures live in `tests/unit/native`. Run the full workspace commands
above before submitting changes; plain `cargo run` continues to launch the desktop application.
