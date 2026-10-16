# Stage 3 follow-up: composable workbench and independent clipboard

This revises the original desktop → UI → workbench layering. Native integration
belongs to workbench; clipboard access is independently usable by a release game.
The separate `bedtime-desktop2` crate has been retired. V1 is unchanged.

## Crates and capabilities

| Crate | Responsibility |
| --- | --- |
| `bedtime-clipboard2` | Shared text/file clipboard, optional OS access, in-process fallback |
| `bedtime-ui2` | ImGui/platform input adaptation, rendering, themes, widgets, icons, effects |
| `bedtime-workbench2` | Headless application state and actions, optional native services and UI presentation |

UI2 depends on clipboard, with no dependency on workbench. Workbench uses clipboard
and optionally UI2. Its default features are `ui`, `native` and `clipboard`.
Disabling defaults leaves core dependent on Bevy's headless/input facilities and
serde. Native services and clipboard can compile without a renderer. Native
clipboard support is an independent feature of the clipboard crate.

After Bevy's window/renderer setup, applications can install `WorkbenchPlugin`
and `UiPlugin` in either order. Workbench does not install DefaultPlugins, a window,
a render camera or a GPU device. The existing demo camera setup is retained.

`WorkbenchPlugin` is a Bevy plugin group containing `WorkbenchCorePlugin`,
`ClipboardPlugin`, the native group and `WorkbenchUiPlugin`, according to enabled
features. Applications can disable group members or compose the same plugins
themselves. `WorkbenchNativePlugin` groups independently installable native window
and native menu plugins. Removing the native menu plugin retains native window
integration and in-application menus.

## Actions, handlers and shortcuts

Action identity is derived from a Rust `CommandDefinition` type. Its readable
namespaced KEY remains the persisted identity. `CommandType` is an erased runtime
type descriptor; neither TypeId nor handler entity numbers are saved.

`Invokes::of::<CopyGreeting>()` references an action type. A definition has one
metadata entity, while handlers are registered Bevy system entities. There may be
one global handler and a handler for each panel instance. `InPanel` links handler
and binding lifetimes to their panel. Closing a panel retires its handlers; menu
placements continue to name the same action type.

`register_command::<C, _>` registers a global handler and returns `CommandType`.
`register_panel_command::<C, _>` registers a scoped handler. Duplicate keys and
duplicate handlers in one scope are rejected. Panel handlers override global
handlers; a disabled panel handler blocks global fallback. Explicit global and
panel invocation contexts are also supported.

There is one durable `CommandRequests` queue. Native menus, ImGui menus and
application systems submit directly. `DesktopAction` and command ingress have
been removed. Dispatch admits a batch, captures all handler/panel targets, checks
availability, runs handlers and publishes results. Handlers' follow-up requests
wait for the next pass. A removed admitted handler fails without rerouting.

`KeyBinding` entities map portable Bevy key chords to action types, optionally
scoped with `InPanel`. A focused panel binding overrides a global binding; an
ambiguous same-scope chord invokes nothing. Native accelerators and displayed
menu shortcut labels use the same resolved bindings. Keyboard routing skips
chords owned by installed native menus to avoid duplicate execution. UI capture
suppresses ordinary keyboard bindings; `allow_in_text` opts a binding into text
input contexts. Persistent shortcut editing and multi-stroke sequences remain
future work. Full widget-level Copy/Cut/Paste command routing is also future work;
ordinary ImGui text controls use the shared clipboard directly.
`allow_in_text` currently governs the Bevy keyboard adapter. Native accelerators
follow the OS menu path; integrating focused-widget interception into that path
is part of the remaining widget-level routing work.

## Abstract menus and native services

Menu ownership remains ECS relationships. Core provides portable `MenuBar`,
`Menu`, `MenuItem`, `MenuSeparator`, `InMenu`, and validated owned snapshots.
Snapshot availability resolves the same scoped handlers as dispatch. Root or item
`CommandContext` components can supply explicit action context.

`NativeMenuBar` selects one root for native presentation. Multiple other roots
can be rendered in the application. `draw_menu_bar` and `draw_popup_menu` present
the same model through UI2. Native callbacks retain source/action identity and
revalidate current ownership, binding and availability before queuing requests.
macOS realizes native menus; other platforms retain in-application presentation.

Native window integration forwards close intentions as `CloseRequested`; the
application still accepts or rejects closing. Configure Bevy's WindowPlugin with
`close_when_requested: false` when using that adapter. The workbench UI adapter
projects theme backgrounds to the optional native appearance service, keeping
UI2 independent of workbench/platform policy.

Clipboard is one shared service, usable without the other crates. Its memory
mode supports text and file lists. Native initialization failure preserves a
diagnostic and selects memory mode; errors during native operations remain errors.

## Verification

`bed2` renders the same menu model natively and in its application toolbar. The
gallery's clipboard controls work without workbench. Native smoke uses a memory
clipboard so it does not replace the user's OS clipboard.

```sh
cargo test --offline -p bedtime-workbench2 --no-default-features --lib
cargo test --offline -p bedtime-workbench2 --no-default-features --features native --lib
cargo test --offline -p bedtime-workbench2 -p bedtime-ui2 -p bedtime-clipboard2 --lib
python3 scripts/check-crate-boundaries.py --offline
cargo run --offline -p bed2 -- --workbench-smoke-dir target/split-default
cargo run --offline -p bed2 -- --ui-first --workbench-smoke-dir target/split-ui-first
cargo run --offline -p bed2 -- --compose-workbench --workbench-smoke-dir target/split-composed
cargo run --offline -p bedtime-ui2 --example gallery -- --smoke-dir target/split-gallery
```

Documents/history, settings/storage, restoration and document-aware close policy
remain later work. The earlier intermittent capture observations recorded in
[step 3](step-3.md) remain unresolved.
