# Bedtime 2

The second foundation is developed beside the v1 packages. `bed2` is its demo
application; the existing `bed` remains the workspace's default application.

```sh
cargo run -p bed2
```

The foundation has three crates:

- [`bedtime-clipboard2`](crates/bedtime-clipboard2): independently usable text/file
  clipboard, optional OS integration, in-process fallback.
- [`bedtime-ui2`](crates/bedtime-ui2): egui/Bevy integration, themes, controls,
  asset-backed icons, effects and UI-local motion. It has no workbench dependency.
- [`bedtime-workbench2`](crates/bedtime-workbench2): headless ContextCommands, scoped handlers,
  keybindings, menus, separate workspace SubApps and scoped tasks/processes, plus optional native
  services and egui workbench presentation. Documents/settings/storage follow.

The former desktop crate has been retired. Native menu/window integration now
belongs to optional workbench plugins. After the application's Bevy setup,
`WorkbenchPlugin` and `UiPlugin` supply the usual desktop foundation in either
registration order. Applications can compose individual workbench plugins or
customize the convenience group. Camera and renderer setup stays application-owned.

Workbench's default features are `ui`, `native` and `clipboard`; disabling defaults
leaves a headless core. UI2 and workbench share the independent clipboard service,
which release games can also use without either of them.

The demo contains two independent workspace runtimes, Blender-style panel layouts,
editable notes, managed tasks/processes and a cube viewport. Shared gallery and
theme tools live in the host. The independent game also runs without Bedtime:

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
