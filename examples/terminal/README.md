# Terminal application

A fresh Bevy `App` with `BedCorePlugin` and `TerminalPlugin`. Core supplies the
native window, renderer, Dear ImGui, theme, embedded fonts/icons, workbench and
shared document foundation. The terminal owns shell sessions, rendering and
input. New shells use the launch directory; an application can configure a
different directory through `TerminalWorkspace`.

```sh
cargo run --locked -p bed-example-terminal
```

The app starts with one shell in the launch directory. Use the built-in panel
picker to add shells, drag tabs to split them, and close tabs to stop their sessions.
Window, appearance, fonts and terminal settings use their plugin defaults.

This example uses core's default frontend and lifecycle. It requires no helper
dispatch, notification draining, UI callback or manual schedule wiring. bEd
configuration, workspace persistence, effects and content viewers are optional
application integrations.

Install `TerminalAppIntegrationPlugin` in an application host to opt into shell helpers and
file links; that host also calls `bed_plugin_terminal::run_helper` before opening its
GUI. See the [core documentation](../../crates/bed-core/README.md) for launch
integration and plugin-owned icon directory contracts.
