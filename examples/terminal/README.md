# Terminal application

An application launched through `bed_app::Application` with the terminal's
launch registration. The SDK supplies the
native window, renderer, Dear ImGui, theme, embedded fonts/icons, workbench and
shared document foundation. The terminal owns shell sessions, rendering and
input. New shells use the launch directory; an application can configure a
different directory through `TerminalLaunchContext`.

```sh
cargo run --locked -p bed-example-terminal
```

The app starts with one shell in the launch directory. Use the built-in panel
picker to add shells, drag tabs to split them, and close tabs to stop their sessions.
Window, appearance, fonts and terminal settings use their plugin defaults.

This example uses the standard frontend and lifecycle. The launch registration
installs the terminal plugin and dispatches its helpers before GUI startup. bEd
configuration, workspace persistence, effects and content viewers are optional
application integrations.

Register `bed_plugin_terminal::launch_registration()` on `Application` to support
shell helpers and file links. See the [application SDK documentation](../../crates/bed-app/README.md)
for launch integration and plugin-owned icon directory contracts.
