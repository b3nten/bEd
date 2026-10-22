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
- [`bedtime-editor2`](crates/bedtime-editor2): embeddable plain text editing,
  persistent indexed text, viewport-only drawing, shared history and animated
  carets. Workbench and Tree-sitter integration are independent optional features.
- [`bedtime-grammars2`](crates/bedtime-grammars2): one pinned native bundle of 30
  selected languages and Markdown inline, with upstream queries and notices.
- [`bedtime-treesitter2`](crates/bedtime-treesitter2): shared incremental trees,
  syntax spans, whole-line folds and code outlines in bounded workers.
- [`bedtime-workbench2`](crates/bedtime-workbench2): headless ContextCommands, scoped handlers,
  keybindings, menus, workspace entities, activation markers, shared typed/text
  documents with history/save/reconciliation, and scoped tasks/processes, plus
  optional native services and egui workbench presentation. Concrete panel
  composition needs no type registry or initialization pass. Creation discovery,
  session formats/restore, source I/O, project data and settings are application-owned.
- [`bedtime-filetree2`](crates/bedtime-filetree2): lazy local filesystem panels,
  native watching, safe file operations and root-scoped hidden preferences.
  Applications explicitly supply each panel's root, handle buffered file-open
  requests and own preference storage. Git integration is deferred.
- [`bedtime-terminal2`](crates/bedtime-terminal2): local Unix PTY terminal panels,
  egui input/rendering, workbench-owned runtime cleanup and opt-in local `+`
  commands with a pre-startup helper. No SSH.

The former desktop crate has been retired. Native menu/window integration now
belongs to optional workbench plugins. After the application's Bevy setup,
`WorkbenchPlugin` and `UiPlugin` supply the usual desktop foundation in either
registration order. Applications can compose individual workbench plugins or
customize the convenience group. Camera and renderer setup stays application-owned.
Applications compose workspace/layout entities and concrete panel components;
there is no panel registry, built-in type picker or core snapshot/restore format.
Active markers select the workspace and its layout; empty workbenches and
workspaces are valid. Required components supply missing defaults and panel
metadata/drawing immediately, preserving explicit overrides. Relationship hooks
maintain membership and selection/focus fallback; reconciliation validates rather
than initializing panels. New panels select their destination tab without changing
area focus. See [application composition](../MIGRATION/application-composition.md)
for the accepted design and phase status.

Workbench's default features are `ui`, `native` and `clipboard`; disabling defaults
leaves a headless core. UI2 and workbench share the independent clipboard service,
which release games can also use without either of them.

`bed2` opens one workspace and a 3D Viewport panel in the application World.
Its `cube.rs` module owns the shared scene and per-panel orbit cameras. Live
Theme controls, a Tasks & Processes panel in `tasks.rs`, and the optional Terminal
plugin remain available. Files opens real documents in code views, and `code.rs`
installs syntax contributions and the application
[Structure panel](../bed2/crates/bed-structure2/README.md). `close.rs` owns application quit decisions and waits for
managed workers to finish cleanup. Memory-only Documents and Hello World remain
removed. The titlebar opens Settings and Files; its application-owned View menu
offers concrete panel creation, including Theme. Its File menu includes New
(unsaved Untitled scratch content), Open Files, Save, Save All, Reload and Quit.
Settings supports inherited global/workspace Editor/Files/Terminal preferences and
application keybindings; Undo/Redo remains widget-owned. Appearance and preferences
have application-owned storage, not session restoration. Save As, recovery and
workspace-management policy remain pending.
The application chooses cwd or an explicit project-directory argument as its
default file-tree root, owns
background text loading, checked saves and root-scoped visibility storage. The
reusable filetree/editor crates retain their explicit host-policy boundaries.
Its entry point contains plugin setup and workspace/panel creation. It has no
demo flags or smoke-test modes.
The optional [automation plugin](crates/bedtime-automation2/README.md) provides
stdin/stdout control; external Python scripts own scenarios and assertions.

The standalone text editor runs without Workbench or language plugins:

```sh
cargo run -p bedtime-editor2 --example standalone
```

The UI-only gallery uses no workbench or native menu plugins:

```sh
cargo run -p bedtime-ui2 --example gallery
```

See [stage 3](../MIGRATION/step-3.md) and the
[workbench split](../MIGRATION/step-3-split.md).

[Stage 4](../MIGRATION/step-4-egui.md) records the egui migration and input ownership model.
