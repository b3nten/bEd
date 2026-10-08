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
layout. Documents, Files, terminals, Settings, Search, References, Diagnostics,
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
overlapping claims. The bundled image plugin handles PNG and JPEG; the glTF
plugin handles GLB and glTF. Other files open in the text editor or the built-in
hex editor according to their content. Files → right-click → **Open With**
selects a viewer explicitly. Image, glTF and hex views share the same exact-byte
document. Switching between text and bytes saves
and closes its views before reopening; a cancelled or failed save keeps them open.

The hex editor shows offsets, hexadecimal bytes and ASCII. Click or Shift-click
bytes to select, type hexadecimal digits to overwrite, use Insert to toggle
insertion, and Backspace/Delete to remove bytes. Copy, cut and paste use hexadecimal
text. Undo/redo and save use the usual shortcuts. Byte documents preserve BOMs,
line endings and arbitrary bytes, and share save/autosave/conflict handling with
text documents; their undo history stays in memory. The image viewer supports
Fit, 100%, zoom and pan, plus an information popup and a default-fit setting.
The glTF viewer previews static, self-contained models with base-color textures,
vertex colors, depth testing, transparency and studio lighting. Drag to orbit,
right/middle-drag to pan, scroll to zoom, and double-click or choose **Frame All**
to reset the framing. Camera state is restored with the workspace. Export GLB or
glTF with embedded buffers and PNG/JPEG textures. Draco-compressed meshes are
decoded in Rust, and skins are shown in their authored pose with up to four
joint influences per vertex. Companion files, morph targets, animation playback
and full PBR materials are not supported yet.

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
`python3 scripts/package-remote-helpers.py stage --target aarch64-unknown-linux-musl`
(use `x86_64-unknown-linux-musl` for x86-64). Cargo does not build these target
binaries automatically when running the desktop from source.

To build, package, and install bEd on macOS:

```sh
bash scripts/build-macos.sh
```

This requires Rust, Xcode Command Line Tools, Python 3, and a running Docker
engine (Docker Desktop or OrbStack). The script builds static Linux x86-64 and
ARM64 helpers in Rust/GCC containers, builds the native desktop, packages
`target/dist/bEd.app` and its ZIP, and copies the app to `/Applications/bEd.app`.
Use `--no-install` to leave the app in `target/dist`. Docker is a build dependency;
the installed app and SSH servers do not need it or a compiler. Container build
caches make subsequent builds incremental.

If the desktop is already built, prepare its helpers and package it with:

```sh
bash scripts/build-remote-helpers.sh
bash scripts/pack-mac.sh
```

Alternatively, extract both matching `bed-remote-helper-*` CI artifacts into
`target/remote-helpers` before packaging; that avoids a local Docker build.

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

Embedding uses document views with an explicit service configuration. The host
owns its context, fonts/style, clipboard, containers, windows and frame/GPU
lifecycle. The session can own autosave, monitoring, history, Git, highlighting
and LSP; default options enable no disk/process services and seed no config.

```rust
use bed_session::EditorSession;
use bed_ui::{EditorView, EditorViewOptions};

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
save documents or history. Use `bed-session` and `bed-ui` directly; the previous
`bed` embedding re-exports and `bed_embed` facade have been removed. See
[the host example](examples/embed_host.rs), which renders two views of one document
inside host-owned windows with winit/wgpu.
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
cargo run --locked -- --platform-smoke --lifecycle-smoke \
  --config-dir /tmp/bed-native-config path/to/project path/to/file.rs
cargo run --locked -- --viewports-smoke \
  --config-dir /tmp/bed-viewport-config path/to/project path/to/file.rs
cargo run --locked -- --menu-smoke
cargo run --locked -- --plugin-smoke
cargo run --locked --example embed_host -- --smoke-test
```

Native checks need a desktop session and graphics backend. `--capture-frame
OUTPUT.ppm --capture-after-frames 30` captures GPU output. macOS is verified
first; native Linux/Windows CI is configured and its results are required before
claiming every-platform acceptance. The pinned viewport backend supports native
undocking on macOS, Windows and X11; Wayland retains internal docking/floating.

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
[nealmick/ned](https://github.com/nealmick/ned). [PORTING.md](PORTING.md) records
source provenance, mappings, dependencies and validation. Original notices and
assets remain attributed in [NOTICE](NOTICE) and accompanying licenses.
Translated terminal portions retain the upstream
[Business Source License](LICENSES/terminal-adapter-BSL-1.1.txt).

## Workspace architecture

The root `bed` package composes the desktop application. Reusable components
live in `crates/`, with concrete APIs and one workspace lockfile:

| Crate | Owns |
| --- | --- |
| `bed-core` | Custom text buffer, state, editing commands, selections, events, undo, UTF-8/path helpers and IDs |
| `bed-files` | Bounded reads, monitoring, file discovery/filtering and cancellable project search |
| `bed-highlight` | Tree-sitter grammars, queries, incremental spans, themes and workers |
| `bed-lsp` | JSON-RPC/process transport, synchronization, diagnostics and workspace language servers |
| `bed-session` | Shared document/view registry, save/autosave, history, Git and service coordination |
| `bed-ui` | Custom document widgets, input, find/line-jump, minimap and LSP presentation |
| `bed-terminal` | Grid/parser, PTY workers, shell sessions, input/rendering and fonts |
| `bed-effects` | wgpu shader passes and viewport postprocessing |
| `bed-remote` | Versioned RPC, SSH transport and automatic helper deployment |
| `bed-headless` | Remote filesystem, search and Git services without a GUI |
| `bed-plugin` | Native plugin contracts, contributions, snapshots, typed host requests and optional GPU canvases |
| `bed-plugin-structure` | Source outline panel and its worker coordination |
| `bed-plugin-image` | Read-only PNG/JPEG panels, bounded background decoding and GPU drawing |
| `bed-plugin-gltf` | Static GLB/embedded glTF loading, orbit cameras and GPU scene rendering |
| `bed` | Workbench docking, tool panels, settings, resources and native application lifecycle |

Core, files, highlighting, LSP and session compile without GUI backends.
Document UI depends on Dear ImGui and the shared services, without native
windows, GPU backends, terminal or application settings. The terminal crate
keeps its parser/PTY and presentation modules together; disabling its default
`ui` feature provides the parser and PTY services without Dear ImGui.
Its translated portions retain the Terminal Adapter license recorded in NOTICE.

`EditorSession::with_view` supplies scoped commands, mutable view state and
read-only document/service access. Text edits go through commands; saves,
path changes and document replacement go through the session. The scope restores
view ownership and transforms sibling selections even if a consumer panics.
`EditorView::presentation()` exposes geometry/hover data for host menus and
LSP widgets without exposing mutable frame state. LSP widgets take explicit
`LspPresentationOptions`. File/search panels wrap GUI-free discovery services
and return navigation actions to the application.

Text Editor, Hex Editor, Files, Settings and Search are host-owned features.
Structure, Image Viewer and glTF Viewer are explicitly linked Rust plugins
registered in the workbench's `PluginRuntime` constructor. Adding a feature means adding its crate
dependency and one constructor to that list. There is no dynamic loading or
plugin-to-plugin event bus.

Implement `bed_plugin::Plugin` to register namespaced commands, panel factories,
file viewers, toolbar buttons, application/file/folder/tree-background/selected-text
menu entries, and settings sections. Panels implement `PluginPanel` and can declare
an attached document so the host includes them in saving, closing and restoration.
`HostContext` provides immutable document snapshots, captured command context,
settings, read-only LSP diagnostics and texture handles. `HostRequest` queues
edits, navigation, document commands, panels, file dialogs and resource changes;
plugins never need a mutable workbench or direct file-writing path.

`EditorSession::apply_edits` takes byte ranges and an expected document revision.
The whole transaction is validated before mutation and becomes one undo unit.
Text transactions retain selection, highlighting and LSP updates. Byte transactions
use exact splice history and skip text services. Existing text-only embedding APIs
retain their behavior; `open_file_auto` and `open_file_with_kind` opt into byte
documents.

Panels receive the host's Dear ImGui `&Ui` in `draw` for controls, popups and
input. With `bed-plugin`'s optional `gpu` feature, `render_output` describes a
host-owned color target, optional depth attachment, physical size and content
revision. The host calls `render` with its wgpu device, queue, encoder and target
after UI/input and before submitting any viewport. Plugins own their pipelines
and source resources; a device generation identifies when to rebuild them.
The shared `gpu::Canvas` presents the target and handles viewport DPI sizing.
Outputs redraw on content changes, resize or device recreation, and are released
when their panel closes. Plugins do not own native windows or submit frames.

Image and glTF use this same GPU output path. Image decoding stays on a worker;
the plugin uploads a source texture once per image/device and draws zoom/pan
into the canvas. Decoded image pixels and each output color target are limited
to 64 MiB. glTF loading also runs on a worker, with bounds on buffers, decoded
textures, vertices, indices and nodes. Draco decoding is bounded to 1,000,000
vertices, 3,000,000 indices and 64 MiB of expanded geometry. glTF textures allow
64 MiB per image and 128 MiB in total. Neither viewer passes CPU output pixels
through the host request API.

Run focused suites with `cargo test -p bed-core`, `cargo test -p bed-session`,
or another crate name. Run the full workspace commands above before submitting
changes; plain `cargo run` continues to launch the desktop application.
