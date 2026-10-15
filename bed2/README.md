# bEd 2

Application host for the second Bedtime foundation:

```sh
cargo run -p bed2
```

The application configures Bevy's window and renderer and installs
`DesktopPlugin`, `UiPlugin` and `WorkbenchPlugin`. Its workbench starts with the gallery and cube in a split layout. The gallery demonstrates
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

Copy Greeting and Toggle Copy Availability buttons reference the same typed
command entities as the macOS native menu. Availability changes affect both
presentations. Quit requests and window-close requests reach application-owned
close handling. Escape requests quit when UI capture allows it; dialogs and text
fields retain their input.
The demonstration Copy Greeting shortcut uses Cmd/Ctrl+Shift+C so ordinary
Cmd/Ctrl+C remains available to text fields.

Workbench's UI integration orders drawing before command ingress and dispatch in
`PostUpdate`. Commands therefore execute after the borrowed ImGui frame ends.

For a gallery without workbench:

```sh
cargo run -p bedtime-ui2 --example gallery
```

See [migration step 3](../MIGRATIONS/step-3.md), [migration step 2](../MIGRATION/step-2.md) and the
[UI crate documentation](../bedtime2/crates/bedtime-ui2/README.md).
