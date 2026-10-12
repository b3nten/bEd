# Minimal

A fresh Bevy `App` with `DesktopPlugins`, `WorkbenchPlugin` and `TerminalPlugin`.
The desktop supplies the native window, renderer, Dear ImGui UI, clipboard and
embedded fonts. The workbench owns panels and tiling; the terminal owns shell
sessions, rendering and input. No editor workbench or configuration is installed.

```sh
cargo run --locked -p minimal
```

The app starts with one shell in the launch directory. Use the built-in panel
picker to add shells, drag tabs to split them, and close tabs to stop their sessions.
Window, appearance, fonts and terminal settings use their plugin defaults.

The terminal crate still brings editor API and document-service dependencies
transitively. Editor-only shell helpers such as `+open` and `+workspace` report
that they require an editor workbench. This target exercises the current plugin
boundaries without installing editor services, effects or content viewers.

The host dispatches private shell helpers and drains terminal notifications;
these are required by the current terminal plugin. The UI callback is the
workbench renderer itself. There is no custom menu, shortcut configuration,
command-line parser or test harness.
