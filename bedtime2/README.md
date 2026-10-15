# Bedtime 2

The second foundation is developed alongside the existing Bedtime packages in
the root Cargo workspace. `bed2` is its application host. Run it with:

```sh
cargo run -p bed2
```

The first crate is [`bedtime-desktop2`](crates/bedtime-desktop2): native platform
services added to an application-owned Bevy app. It has no dependency on the
existing Bedtime framework, ImGui, or Bevy's renderer.

The intended layers are:

- `bedtime-desktop2`: native menus, clipboard, window integration and input admission.
- `bedtime-ui2`: ImGui lifecycle, Bevy rendering, themes, widgets and effects.
- `bedtime-workbench2`: panels, layout, documents/history, workspaces and scoped persistence.

The desktop crate supplies ECS-based macOS menus, text/file clipboard access and
explicit close requests. The first service in
[`bedtime-workbench2`](crates/bedtime-workbench2) registers consumer-defined command
types as ECS entities and runs their queued invocations. It now also owns always-present workspaces, retained layouts and panel instances.
Its optional UI frontend supplies tabs, splitting/resizing and panel content
windows. Documents, settings and storage follow in later slices of
[stage 3](../MIGRATIONS/step-3.md).

[`bedtime-ui2`](crates/bedtime-ui2) now provides the ImGui/Bevy integration,
ECS surfaces, shared view state and theme selection, reusable controls, asset-backed
SVG icon packs and UI-local motion. Its
standalone gallery runs without workbench:

```sh
cargo run -p bedtime-ui2 --example gallery
```

`bed2` embeds that gallery and connects its action buttons to the same typed
commands as the native menus. See [migration step 2](../MIGRATION/step-2.md).
It also embeds a spinning-cube render target and can alpha-compose UI over a
full-window scene. The shared scene runs without Bedtime using
`cargo run -p bed2-demo-game --example standalone`.

Applications install Bevy's plugins themselves. Desktop services attach to the
real primary window; they do not create a second window or GPU device. UI integration
uses Bevy's renderer directly and translates platform services into ImGui input.

The existing `bed` remains the workspace's default application.
