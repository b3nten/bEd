# bEd (Ben's EDitor)

bEd is a next-gen IDE built with [Bevy](https://bevy.org/). it brings code editing, terminals, git, and viewers for nearly any file together in a customizable workbench.

**prerelease:** bEd currently runs on macOS with Apple silicon. expect rough edges and breaking changes to settings and saved workspaces. Linux support is planned.

![screenshot of bed](bed/assets/screenshot_01.png)

## features

- **code editing:** tree-sitter syntax highlighting for 30+ languages, multiple carets, folding, a structure outline with breadcrumbs, find and replace, auto-closing pairs, comment toggling, soft wrap, undo/redo, and VS Code-style keybindings ootb.
- **language intelligence:** LSP completions, diagnostics, hover docs, go to definition, references, rename, code actions, and formatting. language servers are managed in the **Language Servers** panel.
- **command palette:** every action in one searchable list (<kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> or <kbd>F1</kbd>).
- **project search:** search and replace across the workspace, plus a panel that collects TODO/FIXME/HACK comments.
- **git:** browse changes and diffs, stage or unstage individual changes, commit and amend, switch branches, fetch, pull, push, and resolve conflicts.
- **terminals:** high performance shell sessions with command navigation, completion notifications, and clickable file paths and links.
- **previews and data:** live Markdown previews, JSON/TOML trees, editable CSV/TSV tables, an SQLite browser and editor, and a hex viewer for any binary file.
- **media viewers:** images (including SVG and animated formats), audio with waveform, spectrum and spectrogram views, video, glTF/GLB/STL models, and fonts.
- **more panels:** an HTTP client, a web browser, workspace notes, and a file explorer.
- **the workbench:** tabs and split panels, saved layout slots, recent workspaces, multiple windows, and persisted workspace state.
- **make it yours:** 20 bundled color themes plus a theme editor, configurable fonts and keybindings, global and workspace settings, and shader effects.

![screenshot of bed](bed/assets/screenshot_02.png)

![screenshot of bed](bed/assets/screenshot_03.png)

## installation

### prerequisites

- a Mac with Apple silicon
- a current stable [Rust toolchain](https://www.rust-lang.org/tools/install)
- Python 3 for packaging the macOS app

language servers are separate tools installed on your os. install the ones you need and make their executables available on `PATH`, or set their paths in the **Language Servers** panel. git features use the installed `git` executable.

### build and install the macOS app

clone the repository and run the packaging script. it builds `bEd.app` and a ZIP in `target/dist`, `--install` also copies the app into `/Applications`:

```sh
git clone https://github.com/b3nten/bEd.git
cd bEd
bash tools/package-macos.sh --install
open /Applications/bEd.app
```

### run from source

from the repository root:

```sh
# launch with the default workspace
cargo run --locked --release -p bed

# open a project folder
cargo run --locked --release -p bed -- /path/to/project
```

## development

bEd is the reference app for **bedtime**, a set of crates for building desktop apps with Bevy and egui. they live in [`bedtime/crates`](bedtime/crates):

- **`bedtime-ui`:** themes, fonts, widgets, and layouts of tabbed panels.
- **`bedtime-workbench`:** documents and viewers, actions and keys, menus, settings, background work, and workspaces.
- **plugins:** each feature above is its own crate (`bedtime-code`, `bedtime-git`, `bedtime-terminal`, `bedtime-image`, ...) that an app adds as a Bevy plugin.
- **`bedtime`:** re-exports the core crates behind Cargo features.

bed is simply [`bed/main.rs`](bed/main.rs), which composes those plugins and adds bEd's own menus, keys, layouts, and settings.

useful cargo features for the `bed` package:

- `automation`: lets scripts drive the running app over the Bevy Remote Protocol. [`tools/bed.py`](tools/bed.py) uses it to click, type, and take screenshots.
- `profile`: times every system for the **Schedules** panel's timeline.

```sh
cargo test --workspace
cargo run -p bed --features automation
```

## license

bEd is licensed under [GPL-3.0-only](LICENSE). see [NOTICE](NOTICE) for third-party attribution; packaged apps include dependency and asset notices.
