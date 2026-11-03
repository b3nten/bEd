# bEd (Ben's EDitor)

bEd is a next-gen IDE built with [Bevy](https://bevy.org/). it brings code editing, terminals, git, and media viewers together in a customizable workbench.

**work in progress:** currently supported on macOS with Apple silicon. Linux support is planned.

![screenshot of bed](bed/assets/screenshot_01.png)

## features

- **code editing** — tree-sitter syntax highlighting, folding, structure outline, find and replace, undo/redo, etc.
- **language intelligence** — lsp for completions, diagnostics, hover documentation, go to definition, references, rename, code actions, and formatting.
- **git integration** — browse changes and diffs, stage or unstage individual hunks, commit and amend, switch branches, fetch, pull, push, and resolve conflicts.
- **integrated terminals** — local shell sessions with command navigation, completion notifications, and clickable file paths and links.
- **previews and data views** — Markdown previews, JSON/TOML tree views, editable CSV/TSV tables, and a hex viewer for binary files.
- **media viewers** — images (including SVG and animated formats), audio with waveform and spectrum views, video playback, glTF/GLB/STL models, and fonts.
- **a workbench that remembers** — tabs and split panels, saved layout slots, recent workspaces, and persisted workspace state.
- **make it yours** — bundled color themes, configurable fonts and keybindings, global and workspace settings, and shader effects.

![screenshot of bed](bed/assets/screenshot_02.png)

![screenshot of bed](bed/assets/screenshot_03.png)

## installation

### prerequisites

- a Mac with Apple silicon
- a current stable [Rust toolchain](https://www.rust-lang.org/tools/install)
- Python 3 for packaging the macOS app

language servers are separate tools, install the ones you need and make their executables available on `PATH`, or configure their paths in the **Language Servers** panel. git features use the installed `git` executable.

### build and install the macOS app

clone the repository and run the packaging script. `--install` adds bEd to your applications folder:

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

settings and workspace data default to `~/.config/bed`. set `BED_CONFIG_DIR` to override the location.

## license

bEd is licensed under [GPL-3.0-only](LICENSE). see [NOTICE](NOTICE) for third-party attribution; packaged apps include dependency and asset notices.
