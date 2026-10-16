# bedtime-workbench2

Composable workbench services with optional native and ImGui presentation.
The former desktop crate has been folded into this crate; clipboard is an
[independent crate](../bedtime-clipboard2).

After the application's normal Bevy setup, add `WorkbenchPlugin` and UI2's
`UiPlugin` in either order. Workbench creates an active temporary workspace and
layout, but never installs DefaultPlugins, creates a window, camera or GPU device.

| Plugin | Responsibility |
| --- | --- |
| `WorkbenchCorePlugin` | Headless actions, keybindings, abstract menus and workspace/layout/panel ownership |
| `ClipboardPlugin` | Shared text/file clipboard, usable without workbench |
| `WorkbenchNativeWindowPlugin` | Native window appearance and application-owned close requests |
| `WorkbenchNativeMenuPlugin` | Native presentation of the selected abstract menu root |
| `WorkbenchNativePlugin` | Group of the two native plugins |
| `WorkbenchUiPlugin` | ImGui workbench presentation and input integration |
| `WorkbenchPlugin` | Group of core, clipboard, native and UI capabilities enabled at compile time |

Default features are `ui`, `native`, `clipboard`. `native` also enables OS access
in the clipboard crate. With defaults disabled, core compiles without a renderer,
window backend, native menu library or OS clipboard dependency. Plugins control
installed systems; Cargo features control compiled dependencies.

```rust,ignore
// Usual desktop setup, after DefaultPlugins:
app.add_plugins(WorkbenchPlugin).add_plugins(UiPlugin);

// Or compose exactly the capabilities needed:
app.add_plugins((WorkbenchCorePlugin, ClipboardPlugin, WorkbenchUiPlugin, UiPlugin));

// Or disable one member of the group:
app.add_plugins(WorkbenchPlugin.build().disable::<WorkbenchNativeMenuPlugin>());
```

When using native window integration, configure Bevy's WindowPlugin with
`close_when_requested: false`. Read workbench `CloseRequested` and decide whether
to exit. Without this integration the application retains its usual Bevy close policy.

Documents/history, persistence, settings and document-aware close decisions are
later slices of [stage 3](../../../MIGRATION/step-3.md). There is no file, process
or remote service in this crate. The architectural follow-up is documented in
[the split plan](../../../MIGRATION/step-3-split.md).

## Workspaces, layouts and panels

Workspace, layout, area and panel instances are entities. Owning relationships
form workspace → layouts → areas → panels (`InWorkspace`, `InLayout`, `InArea`).
Every workspace has a selected layout; the final layout cannot be deleted.
Activate a different workspace before deleting the current one.

Areas tile a normalized 0–10,000 integer plane. `Area` owns its rectangle and
selected panel; `AreaPanels` gives tab order. ECS stores the live arrangement.
Temporary pure geometry plans handle splits, connected borders and gap filling;
`Workbench` methods own the commits. Switching layouts/workspaces retains panel
instances and state. Closing or deleting retires them. These in-memory operations
do not yet perform document save/discard preflight.

```rust,ignore
let mut workbench = Workbench::new(app.world_mut());
let workspace = workbench.active_workspace();
let layout = workbench.active_layout();
let another = workbench.create_layout(workspace, "Reading")?;
workbench.activate_layout(another)?;
workbench.activate_layout(layout)?;
```

With the `ui` feature:

```rust,ignore
let host = attach_surface(app.world_mut(), surface);
let layout = Workbench::new(app.world_mut()).active_layout();
let panel = register_panel(app.world_mut(), layout, "Notes", draw_notes)?;
app.world_mut().entity_mut(panel).insert(Notes::default());

fn draw_notes(frame: bedtime_ui2::UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    frame.controls().input("Notes", &mut notes.text);
}
```

Each rendered panel is its registered Bevy system entity. It is called only when
its tab/layout/workspace is visible and receives a content window, so it draws
controls without constructing another root window. State can live in ordinary
components on that entity. `PanelPresentation` reports content/tab geometry,
visibility and focus; `AreaPresentation` reports the area, inset surface, tab bar
and content bounds. Inactive state remains alive; it is not drawn. Content windows
use the panel entity for their ImGui identity, so moving a tab retains its native
scroll and widget state.

The host fills its area with a square background without changing window theme
styles. Floating windows and dialogs keep their theme. `WorkbenchHost` can reserve
space above the layout for application chrome, or hide the workbench for a game
rendered over the entire window. Stage 2 still supports one native UI surface.

The frontend uses v1's Blender-style interaction model:

- Drag tabs along a strip to reorder, into a panel's center to group, or toward
  any edge to split. Moving a last tab fills its vacated footprint; closing the
  last tab leaves an empty area.
- Drag an area's corner inward to split, across an adjacent border to join, or
  farther into another area to relocate it. Partial joins preserve target tails.
- Drag a shared border to resize its connected span, including T-junctions.
- Close with ×, middle click or the tab context menu. Double click keeps a
  preview tab. Tab icons use UI2's replaceable `IconSource`.

Gestures, scrolling, preview interpolation and content fades stay UI-local.
Resize previews commit on release. Escape, right click, focus loss, activation or
bounds changes cancel the current gesture. Drag previews and tab drops use the
same final geometry calculation and surface insets. Cross-layout moves, OS
floating panels and persistent restoration remain outside this slice.

Register a reusable panel type with `register_panel_type::<NotesPanel, _>(factory)`.
`PanelDefinition` supplies `KEY` and `LABEL`; the factory is a Bevy system taking
`In<Entity>` (the destination area) and returning `Result<Entity, String>`.
It creates the panel with `register_panel_in_area` (or `create_panel_in_area`
headlessly), inserts its state and returns it. The registered factory entity owns
`PanelType` metadata and `ActionEnabled`, with no parallel registry. The area +
picker and `panel_drag_source(ui, kind, label)` use those entities. Content widgets
can consume `PANEL_TYPE_PAYLOAD` before the workbench's fallback target. Other
payload formats, such as file drops, are not integrated yet.

Drawing collects `WorkbenchRequests`. `WorkbenchSystems::Apply` processes one FIFO
batch after drawing and command dispatch, and publishes `WorkbenchFinished`.
Requests capture target entities. Gesture edits also capture the layout's
structural revision and activation; structural changes or switching away and
back reject stale edits. Application
creation/deletion should run between drawing passes, using the same `Workbench`
APIs. Do not directly despawn owned instances or rewrite their relationships.

`Workspace::settings_enabled` records whether workspace overrides will be allowed;
settings resolution and storage are not implemented yet. The temporary workspace
sets this to false.

## Actions and scoped handlers

```rust,ignore
struct SaveDocument;
impl CommandDefinition for SaveDocument {
    const KEY: &'static str = "example.save_document";
    const LABEL: &'static str = "Save Document";
}
let mut workbench = Workbench::new(app.world_mut());
let action = workbench.register_command::<SaveDocument, _>(save_document)?;
let handler = workbench.register_panel_command::<SaveDocument, _>(panel, save_panel_document)?;
```

Actions are identified by a Rust type. `CommandType` erases that type for mixed
menus and requests; it is not a handler entity. `CommandKey` serializes the stable
namespaced KEY for settings. Entity IDs and TypeId values are never persisted.
Definitions have metadata entities; handlers are registered Bevy system entities
with `CommandHandler`, `ActionEnabled`, and optional owning `InPanel` relationships.
Duplicate keys or duplicate handlers for the same action/scope are rejected.

Handlers take `In<CommandInvocation>` and return `CommandResult`. Queries,
resources and exclusive systems work normally. The invocation captures action,
source and panel. A focused panel's handler takes precedence over a global
handler; a disabled local handler blocks global fallback. Contexts can also name
an explicit panel or request only a global handler. Closing a panel retires its
handlers and scoped bindings, while menu items keep their action type.

```rust,ignore
requests.invoke::<SaveDocument>();
requests.submit(CommandRequest {
    command: CommandType::of::<SaveDocument>(),
    source: Some(button),
    context: CommandContext::Panel(panel),
});
```

There is one durable queue and one dispatcher. UI integration orders drawing,
shortcut resolution, dispatch and layout changes. The dispatcher captures all
batch targets before running any handler. Removing an admitted handler fails
that request without rerouting; disabled handlers fail execution. Errors do not
stop later requests. Follow-up requests enqueued by handlers wait for the next
pass. `CommandFinished` reports results as ordinary Bevy messages.

## Menus and shortcuts

```rust,ignore
let bar = world.spawn(MenuBar).id();
let file = world.spawn((Menu { label: "File".into() }, InMenu(bar))).id();
world.spawn((MenuItem::new("Save"), InMenu(file), Invokes::of::<SaveDocument>()));
world.spawn(KeyBinding::new::<SaveDocument>(
    KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS),
));
```

Menus are headless ECS data. `InMenu`/`MenuEntries` own ordered descendants.
`menu_snapshot` validates a selected root and resolves action availability,
context and shortcut labels. Panels can supply explicit menu context with a
`CommandContext` component or the popup drawing call.

With `native`, add `NativeMenuBar` to one `MenuBar` root to present it natively.
macOS realizes the menu; other platforms can use the ImGui renderer. With `ui`,
call `draw_menu_bar` inside an ImGui window with `MENU_BAR`, or `draw_popup_menu`
after a control. Both presenters use the same snapshot and queue action requests
directly. Native bindings are revalidated against current ownership, action type
and availability. Removing a menu never owns/removes the action's handlers.

Keybindings map portable Bevy chords to action types, optionally scoped by
`InPanel`. Panel bindings override global bindings; same-scope conflicts invoke
nothing. Native accelerators and keyboard routing share the resolved keymap,
with native-owned chords excluded from the keyboard path. UI capture suppresses
ordinary keyboard bindings; `allow_in_text` opts an individual binding in.
Persistent shortcut editing and multi-stroke bindings remain later work. ImGui
text controls currently handle their own Copy/Cut/Paste using the shared clipboard;
full widget-level command routing is not implemented yet.
`allow_in_text` applies to the Bevy keyboard path; native menu accelerators still
follow OS routing until focused-widget interception is integrated there.

```sh
cargo test --offline -p bedtime-workbench2 --no-default-features --lib
cargo test --offline -p bedtime-workbench2 --lib
cargo run --offline -p bed2
```
