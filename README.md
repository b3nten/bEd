bed.

## Keyboard shortcuts

Settings → Keybindings lists application commands and shared content actions under
**Common**, followed by panel-specific actions. Click
an existing shortcut to replace it, or choose **Add binding** for another
combination. **Clear** unbinds an action; **Restore defaults** removes its override.
Recording accepts one key with any modifiers. Escape cancels recording; **Assign
Escape** binds Escape itself. A pulsing red dot indicates recording. Sections
start collapsed and expand when searching.

Defaults use Command on macOS and Control elsewhere for `Primary`. Common defaults
include Primary+N/O/S/W for documents, Primary+P for file search, Primary+Shift+F
for project search, Primary+Comma for settings, Ctrl+Grave for the terminal, and
F5/F9/F10/F11 for debugging. Find, copy, cut, paste, select all, undo and redo
share one binding each across panels. Save, Save As and Open File are application
commands. Arrow navigation and Shift+arrow selection share bindings across text,
hex, CSV, and Explorer where supported. Explorer uses Left/Right to collapse and
expand folders. Word, line, and document navigation retain the Editor keymap.

Primary+/ toggles comments on the current line or selected lines using the
document's language. Rebind **Toggle Line Comment** under Settings → Keybindings
→ Editor. Each toggle is one undo step.

Primary+plus/minus and Primary+0 zoom and reset the focused text editor, Docs,
image, or model view. Images reset to 100%; models reset through Frame All.
Editor menu/API zoom commands remain text-specific and have no default shortcuts.

Cursor movement has an elastic smear by default. Turn it off under
Settings → Effects → Cursor Smear. General → UI Animations also disables it.

Global shortcuts with modifiers take priority over editor and panel bindings.
Shared editing commands follow the focused widget or panel. A panel type shares
its bindings across instances; embedded editors share the Editor keymap. Settings
warns about conflicting bindings and permits saving them. Within the same scope,
the newest user assignment wins. Show-panel commands focus the most recently used
instance or open one; new-panel commands create another instance. Shortcuts open
panels in the focused tab area; menu commands use the largest layout area by
default. Drag a titlebar panel button into an area to choose its destination, just
like dragging a file from the Explorer.

Overrides are stored in `keybindings.json` in the configuration directory. The
file uses current action IDs directly. Defaults come from action declarations.
Custom bindings for the removed panel-specific navigation and Docs zoom IDs
must be reassigned under Common; their old entries remain in the file.
Plugin registration and host integration are described in
[the workbench documentation](crates/bed-workbench/README.md#keyboard-shortcuts).

## Saved layouts

**Show Docs** opens a compact panel with usage notes and current shortcuts for
the focused panel. It keeps that context while you read or interact with Docs.
Collapse sections to save space; use Primary+plus/minus while Docs is focused or
pinch over it to zoom, and Primary+0 to reset. Zoom and section choices
are remembered with the panel. Built-in layout 1 includes Docs on the right.

Command+1–9 (Control+1–9 elsewhere) switches between nine fixed layout slots.
Command+Shift+1–9 focuses panels within the active slot. Both sets of shortcuts
are configurable in Settings → Keybindings.

Each slot keeps its own panels, geometry, tabs, and focus for the workspace.
Switching keeps unsaved documents and terminal processes alive. Slot 1 starts
with the usual arrangement and a narrow Docs column, slot 2 with Git changes,
diff, and a terminal, slot 3 with four terminals in a grid, and slots 4–9 with
one editor area.

**Startup** appears below Layout 9 without a default shortcut. Every new window
without an explicitly selected workspace opens its Startup template, which
contains the **Workspaces** picker by default. File arguments open afterward.
The previous Startup/Terminal/Last project preference is no longer used.

**Window → Set Current Layout** saves the current arrangement as the active
layout's global template. **Set Other Layout → Layout 1–9 / Startup** saves it
for the selected layout. Existing workspace versions keep their changes.
**Reset Current Layout** restores the global arrangement, reusing live panels
and retaining extra documents and terminals as tabs. **Reset Other Layout**
removes a layout's global override; reset its current session to apply the
built-in arrangement. Both submenus include every layout, with Startup last.

Workspace sessions remember the last active layout and all initialized layouts.
They use the existing configuration-directory workspace store. Restarting launches
fresh terminal processes; the existing unsaved-scratch persistence rules apply.
Standalone launches start from the global Startup template rather than their
previous session. Embedding can still request standalone-session restoration.

Tools opens utility panels, including Terminal, Git Changes, and HTTP Client.
Window contains layout selection and native window controls. Document view
creation and splits live in File; file and project search live in Edit.

## Shared runtime and rendering

Bed runs one headful Bevy 0.20 `App`. The `bed-desktop::DesktopPlugins` group installs Bevy's window,
event-loop and rendering plugins, owns the main-thread Dear ImGui integration,
and composites owned ImGui frame snapshots through a window-targeting render
graph. Bevy initializes the GPU, acquires the surface and presents the result.
The adapter uses Bevy's borrowed native window handle; no second event loop or
retained ImGui frame crosses into the render world.

The frame order is native input, ImGui frontend, captured editor requests,
gameplay systems, then rendering. Input capture is resolved before Bevy's input
systems; releases and focus loss clear held input even when the UI captures new
presses. Native menus, clipboard, file drops and titlebar styling belong to the desktop base.
Terminal input belongs to the terminal plugin; document quit barriers belong to
the editor transition owner. IME and input routing belong to the desktop plugin.

`bed-runtime` supplies windowless core schedules, time and input without editor
dependencies. Games can add ordinary Bevy plugins to the editor's app. Embedding
hosts can install `BedRenderPlugin` with their own GPU resources for windowless
rendering; the plugin explicitly selects renderer plugins so linking the desktop
does not accidentally install a second native runner.

Scene/model previews have separate cameras, render layers and texture targets in
this same world. The render schedule produces panel textures, runs custom canvas
commands, composites the ImGui snapshot and applies the editor's window effects.
`bed-desktop` has no dependency on editor effects or scene rendering. GPU recovery
rebuilds device resources while retaining the main world and live panel entities.

Viewer and scene features install ordinary Bevy plugins into the desktop app.
Panel entities register canvas rendering systems; scene/model panels use cameras
and texture targets in the shared world without advancing a separate app.

## Reusable workbench and feature composition

`bed-workbench-api` defines validated panel and command contributions.
Definitions live in a read-only Bevy `Registry` resource; commands and panel
factories are registered Bevy systems. `bed-workbench` owns live panel entities,
focus, tiling, ordered requests and layout persistence. Its panel frontend objects
are main-thread state, with a scoped UI borrow. Commands execute after that borrow
returns and retain their originating entity and area rather than using later
focus. Saved layout numbers remain independent of live entity identities.

`bed-editor-api` adds document and editor service contracts, and
`bed-editor-workbench` composes the editor-specific services and panels. It reuses
the generic panel presentation, tiling and command registration. Features install
ordinary Bevy plugins. Typed components and resources own document associations,
controller state and background workers; ordered systems process feature work and
commands. The frontend holds widget state and emits owned intentions. Stable tab
entities own separate presentation entities, so switching viewers retains each
viewer's state and workers.

The document session and settings remain installed as main-thread Bevy resources.
Each feature owns its controller and widgets in typed resources/components.
Systems borrow what they need, and one editor reducer resolves captured requests
in FIFO order. Command effects finish before the next independent request.
Saving, closing and viewer/workspace transitions flush active and retained drafts;
remote save receipts wait for matching acknowledgements. The legacy Workbench
loan protocol and ModulePanel/Module interfaces have been removed.

The root binary supplies bEd's configuration, icons, workspace storage, working
directory and helper directory, then composes `BedAppPlugin`, `DesktopPlugins`
and the enabled feature plugins. `bed-config` owns stored settings and profiles;
`BedAppPlugin` projects them into runtime appearance, workbench, input and
terminal resources before UI preparation. Feature plugins consume those typed
resources instead of discovering bEd configuration themselves.

`bed-ui` contains portable colors, presentation data, font resolution and canvas
contracts. Its default build has no dependencies; its `imgui` and `gpu` features
add only the presentation capabilities requested by a consumer. `bed-desktop`
owns the canonical live font atlas and embedded default fonts, so a new desktop
does not need a configuration directory or repository resource files. Runtime
palette changes update styling without rebuilding fonts; font changes validate
their sources before replacing the atlas. Native presentation accepts menus,
toolbar buttons and command IDs and emits owned actions for application systems.
Platform capability flags describe which native features are available.

Feature crates have plain descriptive names such as
`bed-editor`, `bed-git` and `bed-model-viewer`. GUI-free service implementations use
`-core` where a feature crate shares the name, such as `bed-git-core` and
`bed-terminal-core`. Existing persisted `bed.*` identifiers are unchanged.

`bed-automation` provides an optional typed Rust API for editor operations,
revision-bound document edits, explicit input frame steps, readiness observations
and asynchronous window/panel captures. It uses the same requests and save
barriers as the UI. Captures share one final composited frame and can export PNG
or PPM; captures after an operation wait for its actual receipt and a subsequent
UI frame. See its [API guide](crates/bed-automation/README.md).

The full editor enables all tools, effects, idle, viewers and scenes by default.
Tools are individually optional: `editor`, `debug`, `git`, `explorer`, `search`,
`terminal`, `projects`, `color` and `settings`; `tools` enables the set. Builds can
also select `structure`, `images`, `models`, `fonts`, `audio`, `csv`, `markdown`, `json`,
`sqlite`, `http` and `scenes`; `viewers` enables all viewer features. For example:

```sh
cargo build -p bed --no-default-features --features editor,images,csv
cargo run -p bed --features automation -- --smoke-test --capture-frame target/editor.png
cargo run -p bed-layout-app
cargo run -p minimal
```

The [independent desktop consumer](tests/fixtures/desktop-app/README.md) has its
own Cargo workspace and lockfile. It builds the base alone, with layout, with
effects, and with both. Its native smoke runs from an unrelated directory and
checks input, clipboard, runtime appearance, font replacement and screenshots.
The current desktop integration still requires the documented local dependency
patches listed in its manifest; these crates are not yet a published toolkit.

The [layout-only example](examples/layout-app/src/main.rs) composes `bed-desktop`
and `bed-workbench` with ordinary application plugins. It excludes editor/document
services, terminal, Git, LSP, audio, PBR and glTF. Unavailable feature panels retain
their saved record in a placeholder, so saving a reduced build preserves state
for a later build that enables the feature again. The dependency boundary check
resolves each consumer separately and enforces this minimal composition.

The [minimal target](examples/minimal/README.md) starts a fresh Bevy app with
`DesktopPlugins`, `WorkbenchPlugin` and `TerminalPlugin`: a window, UI, tiled
panels and live shells. It uses plugin defaults and the built-in panel picker,
with no editor workbench or test harness.

`bed-headless` remains a separate bEd executable for remote helper services.
It does not install Bevy, a window, ImGui or a renderer. Its protocol version,
CLI and helper packaging remain independent of desktop composition.

Syntax highlighting uses Arborium's bundled collection of 112 language grammars
and queries, including Markdown headings, inline formatting and embedded code.
The Markdown inline grammar comes from tree-sitter-md. Grammars and queries are
compiled into Bed; no separate grammar downloads or query resource files are needed.

Code folding uses the bundled syntax grammars. Click a gutter triangle to fold
or unfold a block, or use the editor context menu's Toggle Fold, Fold All and
Unfold All commands. Cmd/Ctrl+Alt+[ toggles the fold at the caret,
Cmd/Ctrl+Alt+Shift+[ folds all blocks, and Cmd/Ctrl+Alt+] unfolds all blocks.
Folds are independent in each pane and work with word wrapping. Navigation to
hidden text reveals it; copying a selection includes its hidden lines.
Opening and closing animate over 160 ms, including the gutter arrow; toggling
again reverses the motion. Disabling UI animations makes folding immediate.
Folding is disabled in inline diff and conflict views.
Supported languages include Rust, Python, JavaScript/TypeScript, JSON, C/C++, Go,
HTML, CSS, Java, C#, Kotlin, Ruby, Bash, HCL and TOML.

## Language servers

Bed bundles 35 language configurations. Language servers provide diagnostics,
hover information and definition/reference navigation. Install the executables
and their required runtimes separately, either locally or on the SSH host where
the project lives. Servers start when matching documents open. Compatible
languages share a process when their server name and resolved project root match.

| Languages | Server executable |
| --- | --- |
| Rust | `rust-analyzer` |
| C, C++ | `clangd` |
| JavaScript, JSX, TypeScript, TSX | `typescript-language-server` |
| Python | `pyright-langserver` |
| Go, go.mod | `gopls` |
| Java | `jdtls` |
| C# | `omnisharp` or `OmniSharp` |
| Lua | `lua-language-server` |
| Bash | `bash-language-server` |
| Markdown | `marksman` |
| TOML | `taplo` |
| JSON, JSONC | `vscode-json-language-server` |
| YAML | `yaml-language-server` |
| HTML | `vscode-html-language-server` |
| CSS, SCSS, Less | `vscode-css-language-server` |
| Ruby | `ruby-lsp` |
| PHP | `intelephense` |
| Swift | `sourcekit-lsp` |
| Elixir | `elixir-ls` |
| Erlang | `erlang_ls` |
| OCaml | `ocamllsp` |
| Clojure | `clojure-lsp` |
| Dart | `dart` |
| CMake | `cmake-language-server` |
| Dockerfile | `docker-langserver` |
| Terraform | `terraform-ls` |
| Nix | `nil` |

The bundled catalog excludes Zig. One server is associated with each language;
completion, inlay hints, formatting, rename and code actions are not implemented
by this foundation.

Diagnostics use server push notifications and document/workspace pulls when the
server supports them. Document pulls refresh diagnostics for unsaved buffers on
open, edit, save and server refresh requests; outdated replies are discarded.
Project-wide diagnostic coverage still depends on workspace pulls or a configured
project check.

### Configuration overrides

Desktop projects load the embedded [catalog](resources/config/lsp.json), then
`~/.config/bed/lsp.json`, then `<project>/.bed/lsp.json`. SSH projects use the local
user file and remote project file. New user files contain `{}` so catalog updates
are inherited. Missing override files are optional. Invalid configuration is
reported; failed reloads preserve running servers.

Languages merge by `name`, servers by their map key. Objects merge recursively;
scalars and arrays replace earlier values. Empty arrays clear inherited lists.
`enabled: false` stops language detection. `language_server: null` retains language
detection without attaching a server. Detection conflicts follow project, user,
then bundled ordering, and declaration order within each layer. Explicit file
globs take priority over extensions, followed by shebangs from document content.
Globs match suffixes of full paths and support brace alternatives such as
`{t,j}sconfig.json`. Path separators are normalized; uppercase C/C++ extensions
retain their distinct associations. `.h` belongs to C++ by default.

For example, a user or project override can configure Rust's existing server,
add a filename association, and detach Python from its default server:

```json
{
  "languages": [
    {
      "name": "rust",
      "file_types": ["rs", {"glob": "*.rust"}],
      "workspace_lsp_roots": ["packages/compiler"]
    },
    {"name": "python", "language_server": null}
  ],
  "language_servers": {
    "rust-analyzer": {
      "command": ["rust-analyzer", "/opt/tools/rust-analyzer"],
      "args": [],
      "environment": {"RUST_LOG": "warn"},
      "settings": {"rust-analyzer": {"check": {"command": "clippy"}}},
      "initialization_options": {"cargo": {"allFeatures": true}},
      "timeout_secs": 30
    }
  }
}
```

Language fields are `name`, `language_id`, `file_types`, `shebangs`, `roots`,
`workspace_lsp_roots`, `language_server` and `enabled`. New languages default to
their name as the protocol ID, empty detection/root lists and `enabled: true`.
Use `javascriptreact`/`typescriptreact` for JSX/TSX protocol IDs. Server `command`
accepts one executable or an ordered list of candidates. Server recipes include
system-location candidates for GUI launches. `args` are literal
arguments, and `environment` affects only the child process; an explicit `PATH`
also controls executable discovery. `settings` is an object delivered after
initialization and used to answer configuration requests. Match settings namespaces to the server’s requested
sections, such as `rust-analyzer`, `python`, `json` or `css`. Request timeouts
accept 1–3600 seconds and default to 20; Java uses 60 seconds.

Roots are found by searching ancestor directories for configured marker names
or globs, stopping at the opened project boundary. The highest matching ancestor
wins; without a marker, the project root is used. `workspace_lsp_roots` contains
relative subdirectories that bound discovery in monorepos. Paths cannot escape
the project. Files outside the project use user configuration and their own
parent directory as the server root.

Configuration files use the schema shown above. Unknown configuration fields are
reported as errors.

Embedded hosts can load an exact configuration with
`SessionOptions.lsp_config_mode = LspConfigMode::Snapshot` (the
default). Snapshot files do not inherit the bundled catalog. Hosts can select
`Layered` to use desktop precedence and project
root discovery. The LSP dashboard exposes server discovery, resolved roots,
startup errors, stderr and restart controls.

Completion opens while typing or with **Ctrl+Space**. Use Up/Down to choose an
item, Tab or Enter to accept, and Escape to dismiss. The compact list shows
documentation beside the selected item when the server provides it. Accepting
a symbol also applies its import edits. Completion currently requires one
collapsed caret; function-argument snippets are not supported.

**Primary+Shift+I** formats the document, **F2** renames a symbol, and
**Primary+Period** opens quick fixes. These shortcuts can be remapped in
Settings. Rename and quick fixes apply immediately. Changes across files form
one undo/redo action; newer edits in affected files must be undone first.
Previously unopened files are saved immediately, including when the action is
undone or redone. Open files keep their ordinary save behavior.

The General settings panel exposes automatic LSP completion, its delay, and
formatting before explicit saves. Their JSON settings are `lsp_autocomplete`
(default `true`), `lsp_completion_delay_ms` (default `150`), and
`lsp_format_on_save` (default `true`). Existing explicit preferences are
preserved. Background autosave does not format. These actions also work in SSH
workspaces. Hover and completion documentation wrap prose, signatures, and
fenced code to the popup width while preserving code indentation and highlighting.

### Helix provenance

Launch recipes and selected file associations are adapted from
[Helix's language catalog](https://github.com/helix-editor/helix/blob/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc/languages.toml)
at revision `ba40e547426b0f9896c8bdc699a4ab11f2b37dbc`. The named language/server
separation, layered overrides and bounded root selection follow Helix's approach.
Bed retains its own transport and document ownership; no Helix crates or runtime
catalog downloads are required. Bed uses its own client identity and enables
Python workspace diagnostics in the bundled recipe.
Helix is licensed under MPL 2.0; the upstream license is retained beside the
[catalog](resources/config/LICENSE.helix).

Bed is licensed under GNU GPL version 3 only (`GPL-3.0-only`); see [LICENSE](LICENSE).
Third-party code, fonts and artwork retain their original licenses and notices
in [NOTICE](NOTICE) and the corresponding resource/vendor directories.
The original MIT notice for portions adapted from ned is retained in
[NOTICE](NOTICE), in the ned section.

When distributing binaries, provide the corresponding source and build scripts,
including required dependency sources, using a method permitted by GPLv3 section 6.
