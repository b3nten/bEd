# bEd

An desktop application platform and IDE with a tiled workspace.

**macOS and Linux only. Linux is currently untested. bEd is very experimental.**

![bEd workspace](assets/screenshot.png)

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

Run the editor, optionally passing a project directory or files:

```sh
./target/release/bed
./target/release/bed /path/to/project
./target/release/bed src/main.rs
```

For development:

```sh
cargo run -p bed
```

Install language servers separately for the languages you use. Open **Settings → Keybindings** to see or change shortcuts.

## License

[GPL-3.0-only](LICENSE). Third-party attribution and licenses are in [NOTICE](NOTICE).
