# bedtime-menus

Menus as entities, drawn with egui (`ui` feature) or as the operating
system's menu bar (`native` feature, macOS). Items run
[`bedtime-actions`](../bedtime-actions/README.md) actions.

```rust,ignore
let bar = world.spawn((MenuBar, NativeMenuBar)).id();
let file = world.spawn((Menu { label: "File".into() }, InMenu(bar))).id();
world.spawn((MenuItem::action::<Save>(), InMenu(file)));
world.spawn((MenuSeparator, InMenu(file)));
world.spawn((MenuItem::action::<Quit>(), InMenu(file)));
```

- **The tree.** A `MenuBar` or `Menu` holds entries (`InMenu` / `MenuEntries`,
  in order): submenus, `MenuItem`s and `MenuSeparator`s. A menu bar's entries
  must be menus.
- **Items** with `Invokes` run that action, and are enabled when it's available
  in the menu's context. `MenuItem::action::<A>()` makes one labelled with the
  action's label. Shortcut hints show the chord that would run the action
  there.
- **Context.** Menus use the focused entity (`ActionFocus`), captured when the
  menu is read, so a menu opened for one panel still acts on it. An
  `InvocationContext` component on a root or entry overrides it.
- **Snapshots.** `world.menus().snapshot(root, context)` reads a tree with
  that state filled in, and checks its shape. Both renderers draw snapshots,
  and `MenuSnapshot::request` turns a chosen item back into an action request,
  refusing items that were removed, disabled, detached or retargeted since.

## `ui`

`ui::draw_menu_bar(&mut frame, world, root, inset)` draws an egui menu bar
with a bottom border, `inset` points from each side. Attach a
`bedtime_ui::UiView` to the `MenuBar` entity to draw controls after its menus
(icon buttons, say); it receives the menu root as its view. The bar draws
nothing while the native menu bar shows that root.
`ui::draw_popup_menu(&mut frame, world, &response, root, context)` shows a menu
as an egui response's context menu. Entries record where they were drawn
(`MenuPresentation`), for tests and automation.

## `native`

`NativeMenuPlugin` shows the `MenuBar` marked `NativeMenuBar` as the operating
system's menu bar (on macOS; elsewhere nothing is installed). Removing the
marker returns to the egui bar. Native clicks and supported accelerators send
the same revalidated action requests; unsupported key mappings keep a text
hint and the ordinary shortcut route. Only one root may be marked; problems
are `NativeMenuError` messages.

```sh
cargo test -p bedtime-menus --all-features
```
