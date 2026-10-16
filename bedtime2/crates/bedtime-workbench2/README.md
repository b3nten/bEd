# bedtime-workbench2

Composable workbench services with optional native and egui presentation.
The former desktop crate has been folded into this crate; clipboard is an
[independent crate](../bedtime-clipboard2).

After the application's normal Bevy setup, add `WorkbenchPlugin` and UI2's
`UiPlugin` in either order. Workbench creates an active temporary workspace and
layout, but never installs DefaultPlugins, creates a window, camera or GPU device.

| Plugin | Responsibility |
| --- | --- |
| `WorkbenchCorePlugin` | Headless contextual commands, keybindings, abstract menus and workspace/layout/panel ownership |
| `ClipboardPlugin` | Shared text/file clipboard, usable without workbench |
| `WorkbenchNativeWindowPlugin` | Native window appearance and application-owned close requests |
| `WorkbenchNativeMenuPlugin` | Native presentation of the selected abstract menu root |
| `WorkbenchNativePlugin` | Group of the two native plugins |
| `WorkbenchUiPlugin` | egui workbench presentation and input integration |
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
app.world_mut().get_mut::<WorkbenchHost>(host).unwrap().menu = Some(menu_root);
let status_bar = app.world().get::<WorkbenchHost>(host).unwrap().status_bar();
// Pass status_bar to plugins registering UI2 status items.
let layout = Workbench::new(app.world_mut()).active_layout();
let panel = register_panel(app.world_mut(), layout, "Notes", draw_notes)?;
app.world_mut().entity_mut(panel).insert(Notes::default());

fn draw_notes(mut frame: bedtime_ui2::UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    frame.text_edit(egui::TextEdit::singleline(&mut notes.text));
}
```

Each rendered panel is its registered Bevy system entity. It is called only when
its tab/layout/workspace is visible and receives a clipped child egui Ui, so it draws
controls inside the allocation supplied by the shell. State can live in ordinary
components on that entity. `PanelPresentation` reports content/tab geometry,
visibility and focus; `AreaPresentation` reports the area, inset surface, tab bar
and content bounds. Inactive state remains alive; it is not drawn. Content UIs
use the panel entity for their egui identity, so moving a tab retains its native
scroll and widget state.

Panel content reaches the surface edges below the tab bar. The shell adds no
content padding and does not wrap renderers in a scroll area. Panels choose both
explicitly through UI2's `padded` and `scroll` helpers. Bed2's gallery, notes and
Controls use the theme's 14×12-point padding; the viewport pads its text and fills
the remaining content area with its scene image.

The host paints the theme background alpha once across its bounds, covering
panel interiors, gaps and outer margins. Panels keep rounded, inset borders,
including a single panel. Attaching the shell selects UI2's
`UiBackground::Transparent` automatically. The root paints the background once
across native titlebar space, menu, panels, gaps and status bar.
Focus borders and tab foreground/close colors fade with the surface motion
policy. Tab hover backgrounds slide upward without moving text or hit bounds.
Corner-split preview lines end inside the visible surface, respecting both
shared gaps and outer margins.
Floating windows and dialogs keep their theme.

The application attaches one workbench to an explicit window surface; the plugin
owns its shell layout. `WorkbenchHost.bounds` optionally selects an outer
rectangle in egui points; otherwise the root uses its parent's available space.
The root reserves the native titlebar, the selected menu's egui row when needed,
and a bottom status bar, then gives the remaining rectangle to the tiled panels.
The status bar belongs to the host; plugins register UI2 items on
`host.status_bar()`. No application-maintained top/bottom insets or ordering of
separate chrome, panel and status views is needed. `WorkbenchPresentation`
reports the actual bounds and titlebar/menu/panel/status allocations.

`panels_visible = false` retains menu and status while leaving the central region
transparent for a scene underneath. Application overlays can use the observed
panel allocation. One native UI surface is supported; the application still
creates its Bevy window, renderer and camera.

The frontend uses v1's Blender-style interaction model:

- Drag tabs along a strip to reorder, into a panel's center to group, or toward
  any edge to split. Moving a last tab fills its vacated footprint; closing the
  last tab leaves an empty area.
- Drag an area's corner inward to split, across an adjacent border to join, or
  farther into another area to relocate it. Partial joins preserve target tails.
- Drag a shared border to resize its connected span, including T-junctions.
- Close with ×, middle click or the tab context menu. Double click keeps a
  preview tab. Tab icons use UI2's replaceable `IconSource`.

Gestures, scrolling and preview interpolation stay UI-local.
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
picker and `panel_drag_source(&response, kind, label)` use those entities. Content widgets
can consume the typed `PanelTypePayload` before the workbench's fallback target. Other
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

## ContextCommands, handlers and bindings

`ContextCommand` declares a namespaced KEY and LABEL. `ContextCommandId` is the
serializable string identity; Rust TypeId and handler entity numbers are not
part of identity. Definitions, bindings, handlers and menu placements are
independent. Typed helpers are conveniences for constructing the same stable ID.

```rust,ignore
struct Save;
impl ContextCommand for Save {
    const KEY: &'static str = "example.save";
    const LABEL: &'static str = "Save";
}
Workbench::new(world).register_context_command::<Save>()?;
Workbench::new(world).register_handler::<Save, _>(save_workbench)?;
Workbench::new(world).register_panel_handler::<Save, _>(panel, save_panel)?;
world.spawn(Binding::new::<Save>(KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS)));
world.spawn((MenuItem::new("Save"), InMenu(file_menu), Invokes::of::<Save>()));
```

Registering a handler also registers missing metadata. One handler per command
and scope is permitted. A panel's `InPanel` ownership retires its handlers,
bindings and menu roots when the panel closes. Menu descendants inherit their
root's lifetime.

Binding lookup starts at the focused panel and falls back to workbench by chord.
Handler lookup independently starts at the invocation's panel and falls back to
workbench by command ID. A global binding can use a panel handler; a panel binding
can use a workbench handler. Disabled local handlers block fallback. Handler
errors and stale admitted targets never reroute to another handler/binding.

Modifiers match exactly; PRIMARY resolves to Cmd on macOS and Ctrl elsewhere.
Chords retain physical Bevy KeyCode identity. Repeats default to disabled. A
selected binding reserves its input even when unavailable. Same-scope conflicts
reserve the chord, execute nothing and publish `BindingConflict`.

`ContextCommandRequests` accepts typed `invoke::<Save>()`, `invoke_id(id)` or
explicit requests with `InvocationContext::{Focused, Global, Panel(entity)}`.
Dispatch admits one batch before execution, captures panel/handler targets,
checks availability and publishes `ContextCommandFinished`. Handler follow-ups
wait for the next dispatch pass. Requests submitted after PreUpdate dispatch,
including ordinary Update systems, execute the next frame.

## Menus

Native and egui presenters consume the same abstract menu snapshots. Availability
uses the selected handler. Shortcut hints use effective bindings in that same
context, excluding shadowed shortcuts. `InPanel` menu roots target their panel.
Within a menu tree, `InPanel` entries appear only when the snapshot's context
targets that panel; use it on a top-level menu to contribute a focus-only menu
to both native and egui bars. Other roots normally use the focused panel.
Explicit invocation contexts can also be attached to roots or descendants.
Snapshot requests reject removed,
disabled, detached or retargeted menu items.

`draw_menu_bar(&mut frame, world, root)` renders an egui menu bar with a bottom
border matching the status bar and horizontal padding from `PanelSettings.panel_gap`.
`draw_popup_menu(&mut frame, world, &response, root, context)` associates a menu
with an egui response. Adding `NativeMenuBar` selects native presentation; removing it returns to egui
when the application calls `draw_menu_bar`. The egui bar is suppressed while
that root is natively presented. Native menu clicks enqueue requests. Native shortcut hints
are label text; keyboard accelerators do not create a second execution path.

## Panel input ownership

`register_panel_input(world, panel, system)` registers one Bevy input system for
a panel. Its borrowed `PanelInput` supplies eligible events/state and operations
to consume events or capture/release keyboard and pointer input. Keyboard input
targets focused content; pointer input targets its content region or capture
owner. UI widgets, overlays and shell gestures retain priority.

```rust,ignore
fn game_input(mut frame: PanelInput, mut game: ResMut<GameInput>) {
    game.input = frame.input.clone();
    if frame.content_focused { frame.capture_keyboard(); }
}
```

A focused game viewport can reserve every keyboard gesture before command
lookup. Captures persist across frames and cancel on focus loss, hiding, closing
or layout changes. Releases follow the owner of the original press; held input
never appears as a fresh press in another owner. The `bed2` scene demonstrates
keyboard capture, while headless tests forward the same input to a separate Bevy
world.

The UI adapter establishes this PreUpdate order: UI/shell, focus observations,
panel input, binding lookup, ContextCommand dispatch, queued workbench edits,
then UI2 publishes remaining application input. Application input consumers use
`bedtime_ui2::input::RoutedInput` after publication. The headless core accepts
normalized `BindingInput` directly and has no egui dependency.

See [stage 4](../../../MIGRATION/step-4-egui.md) for migration and validation.

## Window transparency and macOS blur

`WindowBlur(true)` enables a passive AppKit material behind the Winit content
view. Removing/disabling it hides the material. Its lifetime follows the window;
resizing keeps it fitted without reparenting or replacing Winit's view. It does
not receive input. `WindowBackground` continues to select appearance and native
backing color; translucent GPU backgrounds use a clear native backing so alpha
is applied once. Window-wide opacity remains 1 so text stays opaque.

Applications configure Bevy window transparency/composite alpha mode and the
UI camera's scene-overlay routing. On macOS, enable `fullsize_content_view`
alongside `titlebar_transparent` to paint behind the native titlebar, as in bed1.
`WindowTitlebarInset` reports AppKit's reserved height in native logical window
coordinates, independently of background/blur styling and before UI drawing.
The shell converts it to egui points using the current zoom and reserves only
the portion intersecting its bounds. Controls start below that reservation;
plugins do not read or coordinate native titlebar geometry. Native titlebar controls remain owned by the OS.

Panel gaps and outer margins default to 7.2 logical points, including
single-area layouts. `PanelSettings.panel_gap` sets the full spacing between panels
and between panels and the shell's window, menu, titlebar and status edges.
Tabs have transparent fills with a 12% text-color hover
tint, square lower corners and meet their border. Tab bars have no add button;
empty areas retain a context menu for opening panels. Corner handles use a
crosshair cursor and take egui input priority over panel contents. A tab's faint
hover background slides up from its bottom edge at twice the surface's motion
speed; its geometry and text stay fixed. Reduced motion
settles the highlight immediately. New areas appear at their final size.
Tabs fill the bar height up to the inside of the top border. Both top corners use
the panel rounding. Their icons, labels and close buttons are
centered vertically in the full bar, independently of the tab background.
The tab bar reaches both panel edges; only the contents inside each tab are padded.
Corner splits show both halves immediately. Existing areas animate when tab
drops or area moves resize them; direct border dragging follows the pointer.
Tabs follow their area's geometry without separate interpolation. Panel contents
fade in when first shown or when their committed area or placement changes, using
the surface motion policy. Nested widgets, icons and viewport images inherit the
fade. Geometry interpolation does not restart it; reduced motion settles it
immediately. Pure area relocation snaps its geometry while fading its contents.

An optional `PanelSettings` component on the `UiSurface` configures panel rounding,
opt-in content padding, panel gaps, corner hit size and debug visibility. It is shared with
UI2, independent of `Theme`, and exposed through `UiFrame.panel_settings`.
Defaults are 8.5-point rounding, 14×12-point padding, 7.2-point gaps, 15-point hit targets and debug off:

```rust,ignore
world.entity_mut(window).insert(PanelSettings {
    rounding: 8.5,
    padding: [14.0, 12.0],
    panel_gap: 7.2,
    hit_size: 24.0,
    debug: true,
});
```

Hit size is measured inward from each layout area's outside corner, including its
gutter, so resizing a target changes its intrusion into the panel without moving
the outside anchor. Zero disables corner handles. Targets are capped at half the
area's shortest side to prevent overlapping corners. Debug paints the same
rectangles used for egui hit testing; it does not affect their input priority.
