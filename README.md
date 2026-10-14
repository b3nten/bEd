# bEd

bEd is an IDE built on the Bedtime Bevy desktop framework. This repository also
contains Bedside Table, a small independent SQLite client.

**macOS and Linux only. Linux is currently untested. bEd is very experimental.**

![bEd workspace](bed/assets/screenshot.png)

## Features

- Code editing with syntax highlighting, folding, completion, diagnostics, and language server support.
- File explorer, project search, Git diffs, debugger, and integrated terminals.
- Draggable panels, split views, and nine saved workspace layouts.
- Viewers for Markdown, images, 3D models, fonts, audio, CSV, JSON, and SQLite, plus an HTTP client.
- SSH workspaces, themes, and configurable keyboard shortcuts.

## Build and run

You’ll need Rust and Cargo (stable), Git, and a C/C++ compiler. On macOS, install the Xcode Command Line Tools:

```sh
xcode-select --install
```

On Debian/Ubuntu, install the native dependencies:

```sh
sudo apt-get install build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libasound2-dev libvulkan1 mesa-vulkan-drivers
```

Clone and build:

```sh
git clone https://github.com/b3nten/bEd.git
cd bEd
cargo build --release -p bed
```

Run bEd, optionally passing a project directory or files:

```sh
./target/release/bed
./target/release/bed /path/to/project
./target/release/bed bed/src/main.rs
```

For development:

```sh
cargo run -p bed
```

## Build another desktop application

[`bedtime::awaken`](bedtime/crates/bedtime/README.md) owns startup and installs the
window, renderer, Dear ImGui, theme, fonts, icons, workbench and shared document
editing before feature plugins. Register plugin entrypoints before dispatch;
the desktop callback adds panels and application behavior. bEd keeps its CLI,
configuration projection, numbered-layout policy, themes and smoke commands in `bed/src`.

[Bedside Table](bedside-table/README.md) uses custom Query, Tables, Data and
Schema panels, shared documents and SQLite-backed application metadata without
the code editor or workspace plugins:

```sh
cargo run -p bedside-table -- /path/to/database.sqlite
```

The reusable packages live in [`bedtime/`](bedtime/README.md); bEd owns its
application code and assets in `bed/`. The root is a virtual Cargo workspace.

The examples show the supported composition boundaries:

- [`bedtime-example-terminal`](bedtime/examples/terminal/README.md): core and ordinary terminal panels.
- [`bedtime-example-editor`](bedtime/examples/editor/README.md): text/hex editors with shared
  documents and undo/redo, without workspace or language-server services.
- [`bedtime-example-model`](bedtime/examples/model/README.md): a typed model shared by two panels,
  using the same document/history commands.
- [`bedtime-example-layouts`](bedtime/examples/layouts/README.md): optional numbered layouts from
  [`bedtime-plugin-layouts`](bedtime/crates/bedtime-plugin-layouts/README.md), with application-owned disk persistence.
- [Gameplay editor and runtime](bedtime/examples/gameplay/README.md): one Bevy game runs in
  an editor viewport, or directly in a standalone runtime without bEd foundations.

Plugins register their own namespaced artwork and filename contracts. Hosts can
override directories per plugin; embedded defaults make the examples independent
of bEd's resource directory. See the [icon contract](bedtime/crates/bedtime-ui/README.md).

Install language servers separately for the languages you use. Open **Settings → Keybindings** to see or change shortcuts.

## Testing during development

Use a focused check while iterating:

```sh
cargo bed-test --lib application::runtime::layouts::tests
cargo bed-test --test lsp_transport initialize_and_latest_queued_open -- --exact
cargo bed-test --test app_remote_editor -- --list
cargo test --locked -p bedtime-editing --lib
```

Bed always includes its built-in plugins and automation; it has no Cargo feature
switches. Reusable Bedtime crates retain their own features. Both subprocess
fixture suites (`lsp_transport` and `app_remote_editor`) support name filters,
`--exact`, `--skip`, and `--list`, and print per-test timings. They run sequentially
because some cases own the process-global ImGui context.

Use `cargo test-full` for the full workspace check. It also compiles all targets;
it is intended for broad validation, rather than each edit. CI separately runs the
reusable-crate feature matrix and native smokes. Native smokes can focus windows and accept real
keyboard input, so run them in a deliberate desktop test session.

Cargo caches feature combinations separately. Keep the same command, features,
profile, and target directory for routine reruns. Switching between a package
command and a workspace command can also change unified dependency features.
The first run of another combination may need new artifacts; returning to an
unchanged cached combination should reuse them.

## Crates and vocabulary

Optional features use `bedtime-plugin-*`, such as `bedtime-plugin-editor`,
`bedtime-plugin-terminal` and `bedtime-plugin-layouts`. Foundations and support libraries
keep domain names: `bedtime-core`, `bedtime-desktop`, `bedtime-documents`, `bedtime-terminal` and
`bedtime-git`. The latter two supply backends; their `bedtime-plugin-*` counterparts
supply panels and commands. Example applications use `bedtime-example-*`.

| Term | Responsibility |
| --- | --- |
| Editor | Code editor plugin and its text/hex widgets in `bedtime-editor-widgets` |
| Documents | Content, revisions, views, saving and undo/redo |
| Workbench | Panels, commands, focus and layout |
| Host | Application supplying policy and services; core resolves shared requests |
| File context | Selected directory, provider identity and connection availability |
| Workspace | Optional saved arrangement and plugin state associated with a location |
| Launcher | Helper process dispatch and desktop initialization in `bedtime` |
| Application | Bevy `App` composed from core, feature plugins and application policy |

[`bedtime-api`](bedtime/crates/bedtime-api/README.md) holds shared contracts in `workbench`,
`documents`, `features`, `files`, `storage`, `settings`, `presentation`, `status`
and `gpu` modules.
Use `bedtime::api`, `bedtime::core`, `bedtime::desktop` and `bedtime::ui` when
building an application. The API's default build supports a
workbench without document services; its `documents` feature adds document
contracts. `gpu` and `language-services` are separate opt-ins.

The optional Workspaces plugin accepts an application-supplied repository.
Documents, Search, Explorer and Git use the live file context without requiring a
saved workspace. The workspace picker and File → Open Folder create or reopen a
saved workspace. New command-line and dropped directories stay temporary until
Save Workspace is requested. Explicit paths use layout 1; saved workspaces restore
their selected numbered layout. Launching without a path uses Startup. bEd stores
saved local workspace snapshots and document history in `<root>/.bed`,
remote snapshots locally in `~/.config/bed/remote`, and recent locations and
layout templates in its global configuration directory.
The workspace directory name is application-configurable. Local `workspace.json`
holds opaque layout, plugin-state and document-history snapshots in one commit.
The client index is `registry.json`, and bEd-owned numbered layout templates are
`layouts.json`. Workspace repositories can persist the same snapshots in a database.

Core owns registered, namespaced preferences in
`~/.config/<app_id>/settings.json`, including keybindings. The optional
[`SettingsPlugin`](bedtime/crates/bedtime-plugin-settings/README.md) renders those definitions;
bEd contributes richer Settings pages from `bed/src/settings_view`. Preferences
remain global and independent of saved workspace state. Old preferences,
keybinding, workspace and history formats are not imported.

[`bedtime-storage`](bedtime/crates/bedtime-storage/README.md) holds app-neutral plugin snapshots.
The application chooses their persistence scope and storage implementation.

The [`bedtime-remote-agent`](bedtime/crates/bedtime-remote-agent/README.md) executable serves
remote filesystem, Git, language-server and process requests without a window.
Desktop packages include native Linux agents for SSH deployment.

Reusable packages remain unpublished during the refactor. See the
[publication prerequisites](bedtime/README.md).

## License

[GPL-3.0-only](LICENSE). Third-party attribution and licenses are in [NOTICE](NOTICE).
