# Terminal application

An application launched through `bed_launch::LaunchBuilder` with the terminal's
early launchers and explicit Bevy plugin installation. The SDK supplies the
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

This example uses the standard frontend and lifecycle. Launchers dispatch helpers
before GUI startup; the desktop callback installs the terminal plugins. bEd
configuration, workspace persistence, effects and content viewers are optional
application integrations.

Register `bed_plugin_terminal::launchers()` on `LaunchBuilder` to support
shell helpers and file links. See the [application SDK documentation](../../crates/bed-launch/README.md)
for launch integration and plugin-owned icon directory contracts.

Native smoke verifies independent split shells, Unicode glyphs, hyperlinks, Kitty
images, CSV/image companions and captured GPU output without bEd configuration
or workspace persistence:

```sh
cargo run --locked -p bed-example-terminal --example terminal_smoke
```
