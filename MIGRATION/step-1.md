# Step 1: Desktop foundation, ECS menus and command registration

This step establishes the first working implementation of the architecture in
[overview.md](overview.md). It started with a platform-only desktop crate and a
small application host. The subsequent menu/identity discussion replaced the
initial menu-description resource with ECS relationships and added the first
command service to `bedtime-workbench2`.

**Status:** the desktop scaffold, ECS menus and typed command system are
implemented. A macOS native launch and successful Copy Greeting execution were
observed. Comprehensive native interaction checks, toolbar/window appearance and
UI input integration remain pending.

## Files and workspace integration

| Location | Purpose |
| --- | --- |
| [`bedtime2/README.md`](../bedtime2/README.md) | New foundation's entry point and layer ownership. |
| [`bedtime-desktop2`](../bedtime2/crates/bedtime-desktop2/) | Native platform services and ECS menu presentation. |
| [`bedtime-workbench2`](../bedtime2/crates/bedtime-workbench2/) | Typed command definitions, registration and controlled dispatch. |
| [`bed2`](../bed2/) | Application-owned Bevy setup and native demonstration. |
| [`Cargo.toml`](../Cargo.toml) and [`Cargo.lock`](../Cargo.lock) | Workspace membership and dependency registration. |
| [`scripts/check-crate-boundaries.py`](../scripts/check-crate-boundaries.py) | Separation from the original framework, ImGui and renderer dependencies. |

The new packages inherit the workspace version and license and are unpublished.
The existing `bed` remains the default workspace application.

## Desktop ownership and composition

`DesktopPlugin` attaches native services to the real primary window created by
Bevy. The application installs Bevy's plugins first and configures
`WindowPlugin::close_when_requested` to `false`; installation checks this
requirement. Close decisions stay with the application or future workbench
lifecycle services.

Desktop depends on Bevy's window support, `arboard` for clipboard operations and
`muda` for menus and accelerator types. It does not depend on ImGui, the original
Bedtime framework, `wgpu`, `bevy_render` or `bevy_core_pipeline`. It installs no
`DefaultPlugins` group and creates no window or GPU device.

## Menu entities and relationships

| Component or relationship | Meaning |
| --- | --- |
| `MenuBar` | The single active native menu-bar entity. |
| `Menu { label }` | A top-level group or submenu. |
| `MenuItem` | A placement's label, enabled state and optional accelerator. |
| `MenuSeparator` | A separator entry. |
| `InMenu(parent)` / `MenuEntries` | Ordered ownership with recursive despawning. |
| `Activates(target)` / `ActivationSources` | References to action entities without linked ownership. |
| `ActionEnabled(bool)` | Optional availability on a shared action, also used by command dispatch. |

Bevy maintains both ends of each relationship. Order is the target collection's
order, not ECS query order. `insert_related::<InMenu>` and
`replace_related::<InMenu>` support insertion and reordering. Replacing `InMenu`
moves an entry between menus.

Removing a menu removes its descendants but leaves shared commands alive.
Removing a command detaches its activation references. Referencing items remain
visible but disabled. An item is effectively enabled only when its local flag
and the target's availability both permit activation.

Desktop reads an immutable snapshot of the reachable tree when polling native
services. Changes to labels, shortcuts, membership, order or availability cause
the native menu to rebuild. Nested submenus are supported. Exactly one menu bar
may be active; its immediate children must be menus. Reachable entries must have
one presentation kind and valid labels.

Snapshot validation and native construction finish before replacement. Failures
emit `DesktopError` and preserve the last installed menu. Repeated identical
invalid requests do not produce an error every frame. Activations are admitted
only against the current valid snapshot, so stale bindings cannot invoke an item
that has been removed, disabled, detached or retargeted. Invalid current trees
suppress activations until corrected.

Native item IDs are internal implementation details. Desktop emits
`DesktopAction::Activated { source, target }` with captured ECS entity references.
It has no knowledge of command keys or handlers.

macOS realizes the menu bar without replacing Winit's window or application
delegate. Linux reports `DesktopCapabilities::native_menus == false` and adds
no GTK menu dependency. Native updates currently rebuild the full menu.

## Typed commands

Consumers implement `CommandDefinition` for their command types:

```rust,ignore
struct SaveDocument;
impl CommandDefinition for SaveDocument {
    const KEY: &'static str = "example.save_document";
    const LABEL: &'static str = "Save Document";
}

let mut workbench = Workbench::new(app.world_mut());
let save = workbench.register_command::<SaveDocument, _>(save_document)?;
```

The second generic argument infers Bevy's system marker. Handlers are ordinary
Bevy systems with `In<CommandInvocation>` input and `CommandResult` output,
including systems that borrow resources/queries or receive exclusive world access.

Registration adds `CommandInfo` to the registered-system entity. The returned
entity is the live command identity; there is no separate registry of handlers.
`CommandInfo` requires `ActionEnabled`, initially true. Menu items reference the
entity through `Activates(save)`. The same command can appear in several places.

Keys must have at least two nonempty dot-separated segments, containing ASCII
letters, digits, underscores or hyphens. Duplicate keys or definition types are
rejected before a handler is installed. Keys are immutable through the metadata
API; labels remain editable presentation metadata.

Typed lookup uses `workbench.command::<SaveDocument>()`. `CommandInfo::key()`
provides a serializable `CommandKey`, encoded as a readable string. On restart,
`command_by_key` resolves that key to the new live entity, or returns `None` for
an unavailable definition. This supplies the identity boundary for future saved
shortcuts without persisting entity numbers or Rust `TypeId` values.

## Execution and close handling

Native services run in `DesktopSystems::Native` during `PreUpdate`, forwarding
Bevy window-close requests and polling native menu activations. Workbench command
ingress originally ran afterward in `PreUpdate`; [step 2](step-2.md) moves it to
`PostUpdate`, after UI drawing and before dispatch. Activations retain their original source entity.
Application producers can also submit to the durable FIFO `CommandRequests`.

The command owner dispatches the current batch in `PostUpdate`, in
`CommandSystems::Dispatch`. It checks command existence and shared availability
before each execution. `CommandFinished` reports success or failure; one handler's
ordinary error does not prevent later requests from executing. Requests emitted
by handlers remain queued until the next pass rather than executing recursively.
Results are Bevy messages, not durable asynchronous receipts.

Quit is now an application-defined typed command instead of a special native
menu-entry variant. Its handler emits `DesktopAction::CloseRequested { window:
None }`. Native window close emits the same action with a window entity.
Applications retain pending decisions and commit shutdown with `AppExit`.

Desktop retires native services in `Last`, after Bevy's `OnAppExitSystems`.
Dropping native state removes the installed menu.

## Clipboard

`DesktopClipboard` is installed as a non-send resource only when Bevy publishes
a real primary native window. It provides text/file read and write operations and
an availability query. Initialization errors remain operation errors; contents
are not replaced with invented empty values. ImGui adaptation belongs to UI.

Both command and desktop tests can run without a native window or renderer.
No native clipboard or menu is opened by those headless tests.

## bed2 demonstration

[`bed2/src/main.rs`](../bed2/src/main.rs) installs Bevy's configured window and
renderer, then `DesktopPlugin` and `WorkbenchPlugin`. It registers Copy Greeting,
Toggle Copy Greeting and Quit Application command types during startup and builds
its menu entities using the returned references.

The macOS menu offers **Copy Greeting**, **Tools → Toggle Copy Greeting**, and
**Quit bEd 2**. The toggle changes the copy command's shared availability.
Copy Greeting writes `Hello from bEd 2` to the clipboard. Escape queues the same
Quit command on macOS and Linux. The application's demonstration close handler
accepts window-close and Quit requests and writes `AppExit::Success`.

```sh
cargo run -p bed2
```

## Validation and remaining work

The original scaffold passed build/check, formatting, dependency checks and two
headless tests. The ECS/command revision adds coverage for menu reorder/reparent
and presentation changes, recursive ownership, shared command lifetime,
stale native bindings, malformed menu trees, typed lookup, readable key round trips,
registration collisions, FIFO execution, handler errors, disabled/removed commands,
native ingress and deferred follow-up commands.

Run the focused checks from the repository root:

```sh
cargo test --offline --locked -p bedtime-desktop2 -p bedtime-workbench2 --lib
cargo build --offline --locked -p bed2
cargo fmt --check -p bed2 -p bedtime-desktop2 -p bedtime-workbench2
python3 scripts/check-crate-boundaries.py --offline
```

All 12 focused headless tests pass, and the dependency check passes for all 57
workspace crates. A native macOS run initialized the Bevy window and renderer
without desktop errors and logged successful Copy Greeting execution through the
new command system. This was an interactive startup check, not a complete automated
native suite; the process was stopped from the terminal. Native mutation/close
scenarios and Linux runtime behavior still need validation.

Native toolbar/window appearance, content insets, Dock integration,
image clipboard support and UI input admission remain pending. The command service
does not implement persisted shortcut bindings, argument serialization, asynchronous
barriers or the rest of workbench. No ImGui UI, panel/document lifecycle, workspace
persistence or remote services have been introduced in this step.
