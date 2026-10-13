# bed-app

Owned startup for Bevy desktop applications. `Application` dispatches registered
process entrypoints before configuration, native windows or Bevy construction.
Its desktop path installs windowing, rendering, ImGui, themes, fonts, icons,
workbench panels, editing, settings and shared asset sources in a fixed order.
Applications add optional feature plugins and their own policy afterward.

```rust,no_run
fn main() -> std::io::Result<()> {
    let exit = bed_app::Application::new("example", "Example")?
        .register(bed_plugin_terminal::launch_registration())
        .run(|app| {
            app.add_plugins(bed_plugin_settings::SettingsPlugin);
            Ok(())
        })?;
    std::process::exit(exit);
}
```

A `LaunchRegistration` has one owner, optional early selectors and a deferred
normal plugin installer. Terminal helpers match the executable's `+` prefix;
debugger helpers match their first argument. Executable selectors precede flag
selectors, so opaque helper arguments belong to their handler. Duplicate owners,
empty selectors and overlapping selectors fail before any handler runs.
Early handlers can return an exit code without constructing a GUI.

For application-specific CLI parsing, call `dispatch(std::env::args_os())` and
match `LaunchOutcome`. A `DesktopLaunch` exposes arguments and accepts desktop
options, theme, per-plugin icon directories, configuration directory and
workspace directory name. `build` returns the configured Bevy `App`; `run`
uses its normal runner and Bevy plugin readiness/finish/cleanup lifecycle.
The callback runs after foundation and registered plugin installation, before
settings registration is sealed.

`ApplicationPaths` defaults global configuration to
`~/.config/<app_id>` and the local workspace directory name to `.<app_id>`.
Applications choose whether and when workspace storage is attached. bEd stores
remote snapshots in its client-local configuration directory's `remote`
subdirectory. Core
settings use the versioned `settings.json` store; applications and plugins
register defaults and validators before launch finalization. A malformed store
fails launch without replacing its contents.

`bed_core` remains the service implementation and is re-exported for document,
model and transaction APIs. Its foundation plugin is an internal composition
step, rather than an application setup requirement. `CorePresentationMode::Application`
lets an application supply its own chrome and command policy.

bEd-specific startup, settings view, workspace transitions and native instance
handoff are implemented in the root application's `src/application`, `src/config`
and `src/settings_view`. The reusable launcher has no dependency on those modules
or optional panels.

The [gameplay example](../../examples/gameplay/README.md) shares an ordinary Bevy
game plugin between an owned editor launcher and a runtime whose dependency
graph excludes Bed. Games do not depend on the editor's services or entrypoint.
