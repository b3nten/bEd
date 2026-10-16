# bEd 2

Application host for the second Bedtime foundation:

```sh
cargo run -p bed2
```

The application configures Bevy's window and renderer and installs
`WorkbenchPlugin` and `UiPlugin`. Its workbench starts with the gallery and cube
above a dockable Controls panel. The gallery demonstrates
Everforest Medium light/dark themes, egui widgets, motion and clipped lists.
Use **Browse icons** to explore the SVG catalog and switch replacement packs at runtime.
Theme entities and shared view state live in ECS; controls own UI-local animations.

The Controls panel selects workspaces and layouts, creates/deletes them, and reopens
panels. **Playground / Split** contains the gallery and cube; **Reading** has an
independent gallery; **Scratch** has editable notes. Switching retains panel
instances and state. Each demo layout starts with Controls; View → Open Controls
restores it after closing. Its rows wrap and scroll when docked into a small area.
Everything is in memory in this first workbench slice.

Open **View → Open Theme Editor** (or **Theme editor** in Controls) to edit the
active theme's colors, alpha and dimensions live. Each value has a reset button;
**Reset theme** restores the built-in light or dark defaults. Edits are not saved.

Click tabs to select; close with ×, middle click or the context menu. Drag along
the strip to reorder, into a panel's center to group, or toward an edge to split.
Drag panel corners inward to split, across an adjacent border to join, or farther
into another panel to relocate. Shared borders resize their connected span.
Right-click an empty area to create panels; **Drag notes** in Controls demonstrates
dropping a new panel from a registered factory. Tab bars have no add button;
tabs use transparent fills and a subtle hover tint, as in bed1.
The hover tint slides up from the tab's bottom edge without moving its text.
Escape or right click cancels a gesture.
Controls provides **Panel rounding**, **Panel padding**, **Panel gap**, **Show corner targets**
and **Corner size** for the optional surface `PanelSettings` component. These are
independent of the theme and its editor. Debug starts off. Hit size is measured
inward from the outside corner, including the gutter; zero disables corner handles.
The **3D viewport** embeds the same Bevy game camera as before.
**Full-window scene** renders beneath the UI; **Return to workbench** restores
panel rendering. Hidden tabs/layouts stop the camera from rendering.
The shell provides content without padding; gallery, Notes and Controls opt into
bed1-style 14×12-point padding. The viewport pads its text while the scene image
reaches the panel's left, right and bottom content edges. Panel gaps and outer
margins default to 7.2 logical points and change live through **Panel gap**.

The UI2 status bar shows workspace/layout and focused-panel contributions on the
left, with rendering statistics and an interactive Motion setting on the right.
Icons precede each item. Narrowing the window collapses lower-priority items to
icons; their tooltips remain available, and widening restores their content.
The workbench owns its status bar and reserves its height automatically. The
application supplies the window and menu root, then registers panel and status
contributions. The same root lays out native titlebar space, the optional egui
menu, tiled panels and status, and paints their shared background once.
See [UI2 status bars](../bedtime2/crates/bedtime-ui2/README.md#status-bars) for plugin registration.

The scene is a separate Bevy-only crate. Run it without Bedtime:

```sh
cargo run -p bed2-demo-game --example standalone
```

For native renderer verification:

```sh
cargo run -p bed2 -- --workbench-smoke-dir target/workbench2-smoke
```

The in-application menu bar and macOS native menu present the same abstract menu tree.
Copy Greeting and Toggle Copy Availability name ContextCommands; the dispatcher
resolves their current handlers. Availability affects both presentations. The
gallery's Copy/Paste controls use the shared clipboard independently of workbench.
Click the scene to capture keyboard input before workbench shortcut lookup.
The Copy Greeting shortcut is Cmd/Ctrl+Shift+C; ordinary Cmd/Ctrl+C remains
available to text fields. Workbench owns drawing/shortcut/dispatch ordering.

View opens Controls, the widget gallery, Notes, the Theme Editor and the 3D
viewport. Layout and Workspace menus create/delete their respective containers, with unavailable
delete actions disabled. Cmd/Ctrl+N opens Notes; Cmd/Ctrl+Shift+N creates a layout.
Each Notes panel contributes a **Notes** menu only while focused, with copy,
insert-sample and clear actions handled by that panel. Its Cmd/Ctrl+Shift+C
binding overrides Copy Greeting in that context. Closing the panel retires its
menu, bindings and handlers.

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

The egui migration and contextual input implementation are recorded in [stage 4](../MIGRATION/step-4-egui.md).

The Controls panel's **Native menu** checkbox switches the same menu tree between macOS
and egui presentation. **Blur** and **Opacity** control macOS material and theme
background alpha independently. To start with translucent panels and blur:

```sh
cargo run -p bed2 -- --blur
```

Use `--egui-menu` to start with the in-application menu. These flags also work
with the native smoke command. Blur smoke checks the requested tint alpha in
panels, gaps and the titlebar, with and without blur. The macOS window uses
Bevy's full-size content view. The workbench reserves the observed native
titlebar inset above application controls, including when the UI is zoomed.
