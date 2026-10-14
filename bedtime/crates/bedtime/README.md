# bedtime

Owned startup for Bevy desktop applications. `awaken` collects process launchers
and dispatches them before configuration, native windows or Bevy construction.
The desktop path supplies windowing, rendering, ImGui, themes, fonts, icons,
workbench panels, documents, settings and shared asset sources. Applications
install ordinary Bevy plugins explicitly in the configuration callback.

```rust,no_run
fn main() -> std::io::Result<()> {
    let exit = bedtime::awaken("example", "Example")?
        .with_launchers(bedtime_plugin_terminal::launchers())
        .run(bedtime::DesktopOptions::default(), |app| {
            app.add_plugins(bedtime_plugin_terminal::TerminalPlugin);
            Ok(())
        })?;
    std::process::exit(exit);
}
```

`LaunchBuilder::with_args` supplies an explicit native argument vector, including
argv[0]; the default is `std::env::args_os()`. `with_launchers` adds one
`LaunchRegistration`, containing an owner and early process selectors. It does
not install plugins. Terminal helpers match the executable's `+` prefix;
debugger helpers match their first argument. Executable selectors precede flag
selectors, so opaque helper arguments belong to their handler. Duplicate owners,
empty selectors and overlapping selectors fail before any handler runs.

For application-specific CLI parsing, call `dispatch()` and match
`LaunchOutcome`. A `DesktopLaunch` exposes the remaining arguments. Supply
`DesktopOptions` after parsing configuration: its fields group native desktop
options, theme, icon directories, config directory, workspace directory name
and the default theme. `build(options, configure)` returns a real Bevy `App`;
`run(options, configure)` uses Bevy's runner and plugin lifecycle. The callback
runs after the foundation and settings loading, before registration is sealed.
There is one normal plugin-installation path, in that callback.

`ApplicationPaths` defaults global configuration to `~/.config/<app_id>` and the
local workspace directory name to `.<app_id>`. Applications choose whether and
when workspace storage is attached. bEd keeps remote workspace snapshots in its
client-local configuration directory's `remote` subdirectory. Settings use the
versioned `settings.json` store; plugins and applications register defaults and
validators before launch finalization. A malformed store fails launch without
replacing its contents.

`core`, `desktop`, `ui`, `api` and `bevy` are re-exported. The foundation
plugin is an internal composition step. Applications supply `MenuLayout`,
`StatusBarIdentity` and ordered overlays while using the shared UI shell and
request resolver. bEd startup, themes, rich Settings pages, numbered layouts and
native instance handoff remain in `bed/src`.

Native smoke examples build independent applications with temporary settings:

```sh
cargo run --locked -p bedtime --example workbench_smoke
cargo run --locked -p bedtime --example effects_smoke
cargo run --locked -p bedtime --features language-services --example viewers_smoke
cargo run --locked -p bedtime --features language-services --example git_smoke
cargo run --locked -p bedtime --features language-services --example debugger_smoke
cargo run --locked -p bedtime-example-terminal --example terminal_smoke
```

Run these sequentially on a visible desktop. They require real native
presentations; captures also verify GPU output. Debugger smoke additionally
requires Rust and LLDB. `workbench_smoke` publishes status widgets without any
feature panel, verifies commands and compression, withdraws contributions,
changes a font setting, and writes `target/workbench-smoke.png`.

The [gameplay example](../../examples/gameplay/README.md) shares an ordinary Bevy
game plugin between a desktop editor and a runtime whose dependencies exclude
Bedtime. Game execution does not require editor services or their entrypoint.
