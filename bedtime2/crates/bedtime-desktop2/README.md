# bedtime-desktop2

Platform services for an application-owned Bevy window. Install `DesktopPlugin`
after Bevy's `WindowPlugin`, with `close_when_requested: false`. The application
owns window configuration, rendering and exit decisions. The plugin adds no
renderer, command handlers, ImGui or document services.

## ECS menus

Spawn a `MenuBar` entity, then attach `Menu` entities with `InMenu(bar)`.
A menu can contain submenus, `MenuItem` entities and `MenuSeparator` entities.
`InMenu` maintains an ordered `MenuEntries` relationship target. Use Bevy's
`insert_related::<InMenu>` and `replace_related::<InMenu>` to change order.

Attach `Activates(target)` to a menu item to reference an opaque action entity.
Bevy maintains `ActivationSources` on the target. The target can be a registered
workbench command, but desktop has no knowledge of its handler or persistent key.
Multiple presentations can reference the same target.

Removing a menu despawns its owned entries recursively. It does not despawn
shared actions. Removing an action detaches its `Activates` relationships; those
menu items remain visible but disabled. Items are enabled only when their own
`enabled` field is true, their action exists, and its optional `ActionEnabled`
component allows activation. Command owners use that same component for dispatch.

Labels, shortcuts, availability, membership and order can be modified through
ordinary ECS operations. Desktop builds an immutable snapshot in native polling
and rebuilds the menu when it changes. Invalid trees emit `DesktopError` and
retain the last successfully installed native menu. Native activations are checked
against the current valid tree, so removed, detached, retargeted or disabled items
cannot activate an old binding. An invalid tree suppresses activations until fixed.

Only one menu bar is supported. Its immediate children must be menus. Every
reachable entry must have exactly one presentation kind, with nonempty labels
where applicable. Nested submenus are supported.

macOS realizes the tree as the application menu bar. Linux currently reports
`DesktopCapabilities::native_menus == false` and adds no GTK menu backend.
Desktop owns the menu bar without replacing Winit's window or application delegate.

## Actions and lifetime

Native menu clicks emit `DesktopAction::Activated { source, target }`, retaining
both the item entity and the action entity captured by its native binding.
The workbench's command ingress queues these activations for controlled dispatch.

Window-close requests emit `DesktopAction::CloseRequested { window: Some(entity) }`.
An application's Quit command can emit the same action with `window: None`.
Desktop never performs immediate process termination. Applications retain pending
close decisions in their own state and write `AppExit` when ready to exit.

`DesktopSystems::Native` runs in `PreUpdate`. Shutdown retires native services
in `Last` after Bevy's `OnAppExitSystems`; dropping native state removes its menu.

## Window background and clipboard

Attach `WindowBackground::new([r, g, b, a])` to a window to set its native sRGB
backing color. On macOS this fills the space behind a transparent title bar and
selects contrasting native light/dark chrome. Changed values are applied on the
main thread in `Last`; other platforms retain their normal decorations.
`bedtime-ui2` supplies this component from its selected theme.

Once Bevy creates the real primary window, `DesktopClipboard` becomes a non-send
resource for text and file operations. Initialization failures remain operation
errors. ImGui adaptation belongs in the UI crate.

Headless tests can install `DesktopPlugin` with `WindowPlugin` alone. Without a
real native window, it opens no clipboard and installs no native menus.

```sh
cargo test --locked -p bedtime-desktop2 --lib
cargo check --locked -p bed2
```

See [bed2](../../../bed2/src/main.rs) for composition and
[workbench commands](../bedtime-workbench2/README.md) for typed action registration.
