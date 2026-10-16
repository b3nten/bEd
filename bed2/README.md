# bEd 2

Application host for the second Bedtime foundation:

```sh
cargo run -p bed2
```

The application configures Bevy's window and renderer and installs
`WorkbenchPlugin` and `UiPlugin`. Its workbench starts with the gallery and cube in a split layout. The gallery demonstrates
Everforest Medium light/dark themes, reusable controls, motion and clipped lists.
Use **Browse icons** to explore the SVG catalog and switch replacement packs at runtime.
Theme entities and shared view state live in ECS; controls own UI-local animations.

The toolbar selects workspaces and layouts, creates/deletes them, and reopens
panels. **Playground / Split** contains the gallery and cube; **Reading** has an
independent gallery; **Scratch** has editable notes. Switching retains panel
instances and state. Everything is in memory in this first workbench slice.

Click tabs to select; close with ×, middle click or the context menu. Drag along
the strip to reorder, into a panel's center to group, or toward an edge to split.
Drag panel corners inward to split, across an adjacent border to join, or farther
into another panel to relocate. Shared borders resize their connected span.
The area's + picker creates panels; **Drag notes** demonstrates dropping a new
panel from a registered factory. Escape or right click cancels a gesture.
The **3D viewport** embeds the same Bevy game camera as before.
**Full-window scene** renders beneath the UI; **Return to workbench** restores
panel rendering. Hidden tabs/layouts stop the camera from rendering.

The scene is a separate Bevy-only crate. Run it without Bedtime:

```sh
cargo run -p bed2-demo-game --example standalone
```

For native renderer verification:

```sh
cargo run -p bed2 -- --workbench-smoke-dir target/workbench2-smoke
```

The toolbar menu and macOS native menu present the same abstract menu tree.
Copy Greeting and Toggle Copy Availability name action types; the dispatcher
resolves their current handlers. Availability affects both presentations. The
gallery's Copy/Paste controls use the shared clipboard independently of workbench.
The Copy Greeting shortcut is Cmd/Ctrl+Shift+C; ordinary Cmd/Ctrl+C remains
available to text fields. Workbench owns drawing/shortcut/dispatch ordering.

`WorkbenchPlugin` is a convenience group. These demo flags verify composition:

```sh
cargo run -p bed2 -- --ui-first
cargo run -p bed2 -- --compose-workbench
```

The second composes core, clipboard, native window and workbench UI plugins
explicitly, omitting native menus. Add either flag to the smoke command above.
Smoke uses a memory clipboard and leaves the user's system clipboard unchanged.
Quit/window-close intentions still reach application-owned close policy. The
existing camera/compositor setup remains application-owned.

For a gallery without workbench:

```sh
cargo run -p bedtime-ui2 --example gallery
```

See [migration step 3](../MIGRATION/step-3.md), [migration step 2](../MIGRATION/step-2.md) and the
[UI crate documentation](../bedtime2/crates/bedtime-ui2/README.md).
