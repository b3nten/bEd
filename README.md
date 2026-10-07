# bed

Bed is a Rust desktop text editor with dockable tools, SSH projects and
embeddable document views. It uses a custom byte buffer with multi-cursor editing
and undo, Dear ImGui for its interface, and winit/wgpu for native windows and
rendering.

```sh
cargo run --locked -- path/to/project path/to/file.rs
```

Run without paths for the project picker. Recent projects remember their open
files, independent views, tool panels, terminal working directories and docking
layout. Documents, Files, terminals, Settings, Search, References, Diagnostics
and Language Servers are ordinary ImGui tabs. Drag tabs between groups, split
panels or detach them into native windows. macOS has native menus through Muda.
Titlebar panel buttons and menu clicks create new instances. New panels and
opened files join the largest dock group; keyboard shortcuts reveal existing
tools. Separate Search tabs keep independent queries and results.
The titlebar offers Files, Terminal, Settings, Search, Diagnostics, Split Right
and Split Down with uniform spacing. Splits duplicate the active document view,
including when a tool panel has focus.
Closing a detached window closes its contained tabs; a failed save or cancelled
Save As keeps the group open.
Restored terminals start fresh shells in their recorded launch directories;
live processes, scrollback and subsequent shell `cd` changes are not restored.

Right-click in Files to toggle **Hide Gitignored Files** or **Hide Hidden Files**
(dot-prefixed names), or right-click a file or folder and choose **Hide from File
Tree**. Choices are saved per project in `~/bed/config/workspaces.json`, separately
for local and SSH projects; both filters start off. **Show Hidden Files** temporarily
reveals filtered entries dimmed, with **Unhide from File Tree** for manually hidden
paths. Hiding a folder covers its subtree. These controls affect only the tree.
The Git-ignore filter needs Git installed on the project’s machine. SSH directory
metadata uses protocol v2; Bed automatically installs the matching helper.

A document can appear in several views. Its text, undo, autosave, highlighting,
Git and LSP services are shared; cursors, selections, find and scrolling belong
to each view. Named files autosave after one second of inactivity. External disk
changes reload clean buffers; dirty buffers offer Reload, Keep Buffer or Save As.

SSH projects use the same local editor: typing, undo, selections and highlighting
stay on your machine. Files, search, Git, language servers and shells run on the
remote host. In Projects, enter an SSH host or config alias and a project path
such as `~/Dev/foo` or `/srv/foo`. Bed resolves `~` using the remote account's
home directory and uses the folder name as the display name. Recent workspaces
can be renamed and retain separate layouts for each local/SSH target and root.
Existing saved local projects migrate automatically.

Desktop packages include prebuilt Linux x86-64 and ARM64 helpers. On first
connection, Bed detects the remote CPU, uploads the matching `bed-headless` to
the SSH user's `~/.cache/bed/helpers/`, and launches it there. Later connections
reuse the same helper; a different bundled binary installs alongside the old
one automatically. Uploads commit atomically after validating the executable.
No sudo, remote download, compiler or PATH changes are needed.

Bed manages the helper automatically. It invokes your system `ssh`, so existing
SSH config, keys and authentication agents apply. No
listening TCP port or additional credentials are configured by Bed. Language
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

Remote saves are asynchronous and check the previously loaded disk baseline.
They preserve BOM/line-ending bytes and clear dirty state only after the relevant
version is acknowledged. Closing a tab during a save keeps it open; retry after
the save finishes. Losing SSH retains local buffers and undo; **Reconnect SSH**
starts a fresh helper and checks for external changes. Remote terminals require
an explicit restart after disconnection and do not survive closing Bed. Remote
undo history is in memory, and destructors never save documents or history.

This first version uses one root per workspace, supports UTF-8 paths within the
selected root, and rejects remote editable files larger than 1 MiB. Remote file
deletion asks for permanent deletion rather than using your desktop Trash. Open
and Save As accept remote paths in Bed. Editing local configuration files from a
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
let document = session.create_document(b"Hello, Bed\n")?;
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
cargo test -p bed-effects --locked --lib native_shader_fixtures -- --ignored --nocapture
cargo run --locked -- --platform-smoke --lifecycle-smoke \
  --config-dir /tmp/bed-native-config path/to/project path/to/file.rs
cargo run --locked -- --viewports-smoke \
  --config-dir /tmp/bed-viewport-config path/to/project path/to/file.rs
cargo run --locked -- --menu-smoke
cargo run --locked --example embed_host -- --smoke-test
```

Native checks need a desktop session and graphics backend. `--capture-frame
OUTPUT.ppm --capture-after-frames 30` captures GPU output. macOS is verified
first; native Linux/Windows CI is configured and its results are required before
claiming every-platform acceptance. The pinned viewport backend supports native
undocking on macOS, Windows and X11; Wayland retains internal docking/floating.

Bed's original buffer and editing algorithms were translated from
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

Run focused suites with `cargo test -p bed-core`, `cargo test -p bed-session`,
or another crate name. Run the full workspace commands above before submitting
changes; plain `cargo run` continues to launch the desktop application.
