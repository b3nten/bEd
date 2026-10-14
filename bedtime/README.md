# Bedtime

A Bevy desktop application framework. The `bedtime` crate owns launch dispatch
and builds an ordinary Bevy `App` with windowing, rendering, Dear ImGui, themes,
settings, panels, commands, focus, documents, and undo/redo. Optional feature
plugins live in `crates/bedtime-plugin-*`.

```rust,no_run
fn main() -> std::io::Result<()> {
    let exit = bedtime::awaken("example", "Example")?
        .run(bedtime::DesktopOptions::default(), |app| {
            // Add application panels and ordinary Bevy plugins here.
            Ok(())
        })?;
    std::process::exit(exit);
}
```

The application chooses its menus, initial layout, persistence adapter, and
feature plugins. Documents and file access work without saved workspaces. A
`DocumentWriter` can save a typed document to a database rather than a file.
The optional Workspaces plugin accepts an application-provided repository;
its filesystem implementation is one adapter, not a core requirement.

See the [launch API](crates/bedtime/README.md),
[core](crates/bedtime-core/README.md),
[workbench](crates/bedtime-workbench/README.md),
[storage](crates/bedtime-storage/README.md), and
[workspace persistence](crates/bedtime-plugin-workspaces/README.md).
[Bedside Table](../bedside-table/README.md) is an independent SQLite client using
custom panels and typed documents without the code editor plugin.

The [examples](examples/) cover minimal editors, terminals, models, layouts,
and an ordinary Bevy game with separate editor and runtime targets. The game
runtime has no Bedtime dependencies.

Packages are deliberately marked `publish = false` during this refactor.
Publication needs explicit versions for all internal dependencies, published
replacements for local dependency patches, an audit of package resource/test
paths, and a decision on the existing GPL-3.0-only licensing. No package has
been published. Local patches remain documented under `vendor/*/BED_PATCHES.md`.
