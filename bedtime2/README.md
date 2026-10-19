# Bedtime 2

The second foundation is developed beside the v1 packages. `bed2` is its demo
application; the existing `bed` remains the workspace's default application.

```sh
cargo run -p bed2
```

The foundation and its optional panels live in these crates:

- [`bedtime-clipboard2`](crates/bedtime-clipboard2): independently usable text/file
  clipboard, optional OS integration, in-process fallback.
- [`bedtime-ui2`](crates/bedtime-ui2): egui/Bevy integration, themes, controls,
  asset-backed icons, effects and UI-local motion. It has no workbench dependency.
- [`bedtime-workbench2`](crates/bedtime-workbench2): headless ContextCommands, scoped handlers,
  keybindings, menus, workspace entities, activation markers, shared typed/text
  documents with history/save/reconciliation, and scoped tasks/processes, plus
  optional native services and egui workbench presentation. Workspace/layout
  snapshots support application-owned storage; source I/O, document restore
  descriptors, project data and settings remain application-owned.
- [`bedtime-filetree2`](crates/bedtime-filetree2): lazy local filesystem panels,
  native watching, safe file operations and root-scoped hidden preferences.
  Applications explicitly supply each panel's root, handle buffered file-open
  requests and own preference storage. Git integration is deferred.
- [`bedtime-terminal2`](crates/bedtime-terminal2): local Unix PTY terminal panels,
  egui input/rendering, and workbench-owned runtime cleanup. No SSH or `+` commands.

The former desktop crate has been retired. Native menu/window integration now
belongs to optional workbench plugins. After the application's Bevy setup,
`WorkbenchPlugin` and `UiPlugin` supply the usual desktop foundation in either
registration order. Applications can compose individual workbench plugins or
customize the convenience group. Camera and renderer setup stays application-owned.
Applications register panel types once, then compose workspace/layout/panel
entities or restore snapshots. Active markers select the workspace and its layout;
empty workbenches and workspaces are valid. Required components and reconciliation
supply defaults and derived membership without initialization helper sequences.

Workbench's default features are `ui`, `native` and `clipboard`; disabling defaults
leaves a headless core. UI2 and workbench share the independent clipboard service,
which release games can also use without either of them.

`bed2` opens one workspace and a 3D Viewport panel in the application World.
Its `cube.rs` module owns the shared scene and per-panel orbit cameras; Hello
World, live Theme controls, a Tasks & Processes panel in `tasks.rs`, and memory-only
shared document viewers in `documents.rs`, Files/read-only previews and preference
storage in `files.rs`, plus the optional Terminal plugin, remain available. The
folder titlebar button opens Documents; the stacked-files button opens Files. bEd2
explicitly chooses a startup directory from its positional argument or cwd; this
is application policy, not a library workspace root. Its application-owned quit
flow preflights all workspaces and waits for admitted file operations to reconcile
before stopping preference storage.
Its entry point contains plugin
setup, workspace/panel creation and window-close handling. It has no demo flags
or smoke-test modes.
The optional [automation plugin](crates/bedtime-automation2/README.md) provides
stdin/stdout control; external Python scripts own scenarios and assertions.

The independent game examples also run without Workbench:

```sh
cargo run -p bed2-demo-game --example standalone
```

The UI-only gallery uses no workbench or native menu plugins:

```sh
cargo run -p bedtime-ui2 --example gallery
```

See [stage 3](../MIGRATION/step-3.md) and the
[workbench split](../MIGRATION/step-3-split.md).

[Stage 4](../MIGRATION/step-4-egui.md) records the egui migration and input ownership model.
