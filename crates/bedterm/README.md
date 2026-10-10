# Terminal shell helpers for bEd

This crate dispatches the private shell helpers used by ordinary bEd terminal
panels. Terminal rendering, shells, and workspace promotion belong to bEd's
terminal module. There is one application binary, `bed`.

To start with terminals, arrange the window and choose **Window → Save Default
Layout**. Future bare launches and New Window reuse that arrangement with fresh
shells. Bare launches use `$HOME`. A folder argument opens its workspace only if
that exact root is already saved; other folders stay standalone. Files open
afterward. Use `--cwd` to choose the initial working directory without opening a
workspace. Explicit directories, including `/`, are used as supplied.

## Companion viewers

The ordinary terminal supports `+open` and its `+o` alias, including terminals
opened from editor workspaces:

```sh
+open crates/bedterm/examples/results.csv
+open crates/bedterm/examples/chart.svg
+open "a file with spaces.png" results.csv
+o crates/bedterm/examples/chart.svg
```

Paths resolve from the calling shell's current directory. The first viewer opens
beside that shell; later files join its companion group. Focusing another shell
does not redirect the result. Viewers use bEd's existing image, CSV, model, font,
audio, text, and hex panels.

The shell receives private `+open`, `+o`, `+workspace`, and `+w` helpers on PATH and
connection information for the window and terminal pane that launched it.
`bed open FILE...` is also supported from those shells. If startup files
replace PATH, use `"$BEDTERM_EXE" open FILE`.

## Promote to an editor workspace

Run `+workspace` or `+w` to open the calling shell's current directory as an editor
workspace. It takes no arguments. `bed workspace` and
`"$BEDTERM_EXE" workspace` also work from these shells. The former `+upgrade` and
`+u` names have been removed.

**Open Folder** and the Projects panel also open or create local workspaces
through an explicit selection. There is no terminal-directory toolbar button.
An attached local workspace opens other workspaces in new windows, creating a
workspace with the default layout when necessary; selecting its current root
does nothing. These commands do not upgrade SSH workspaces.

In a standalone window, promotion opens the workspace in place and preserves all
running terminals, jobs, scrollback, live viewer panels, and unsaved documents:

- For a saved workspace, bEd restores its layout and adds the window's live
  panels as tabs in the largest tiled area. Startup closes. Earlier terminal
  split groups are combined into that group.
- For a new workspace, bEd applies the saved default layout, or the built-in
  workspace arrangement if no default is saved. The calling shell fills the
  first terminal slot; other live panels join the largest area. If the default
  has no terminal slot, the calling shell joins that area too.

Saved workspace layouts retain area geometry, tab order, selection, and
deliberately empty areas. Older docking layouts migrate into the main window.

Files, folder search, Git, Structure, terminals and document editing are
available before promotion. Window tools and fresh terminals use the fixed
launch directory, independent of terminal focus and shell `cd`. Promotion resets
that base to the selected workspace root. Existing shells remain where they are;
restored terminals retain their saved directories. Git uses the discovered
repository root. The titlebar shows the window directory and panels show their scope.
Language servers, project diagnostics, references, debugging and SSH require a
workspace. Workspace sessions, undo/redo, and module data persist after
promotion; default layouts and global settings persist throughout.

Run the helper in a local shell at the directory you want to open. Application
restarts start fresh terminal processes; preserving running jobs and scrollback applies to promotion within the
existing window.

## Shared configuration

All startup paths use one configuration directory, `~/.config/bed`:

- `settings.json` contains application preferences and shader effect settings.
- `keybinds.json` contains the ordinary bEd keybindings.
- `themes/` contains custom themes.

There is no separate terminal profile or terminal shortcut scheme. Settings and
theme changes apply to the same workbench before and after promotion. Use
Settings → Effects for the shared renderer's shader effects.

`--config-dir DIRECTORY` uses an isolated configuration directory for the entire
application, including promotion and workspace storage. It works identically
for ordinary launches and New Window.

## Validation

```sh
cargo test --locked -p bed startup::tests
cargo test --locked -p bed-terminal
cargo test --locked -p bed-workbench shell::promotion::tests
cargo run --locked --bin bed -- --terminal-smoke \
  --config-dir "$(mktemp -d)" --capture-frame /tmp/bed-term-smoke.ppm
cargo run --locked --bin bed -- --term-promotion-smoke \
  --config-dir "$(mktemp -d)" --capture-frame /tmp/bed-term-promotion.ppm
```

Native smoke checks exercise a terminal fixture, companion viewers, and promotion
while checking that running terminal processes and native windows survive. They
exit automatically and require a desktop session and GPU.
