# bedtime-workbench2

Composable workspace runtimes with optional native and egui presentation.
Add WorkbenchPlugin and UI2's UiPlugin after the application's normal Bevy setup,
in either order. Workbench creates an active in-memory Untitled workspace;
it never installs DefaultPlugins or creates a window, camera or GPU device.

| Plugin | Responsibility |
| --- | --- |
| WorkbenchCorePlugin | Host workspace registry, activation, scheduling and shutdown |
| WorkspacePlugin | Local commands, keybindings, menus, layouts and panels; installed by each runtime |
| ClipboardPlugin | Shared text/file clipboard, usable independently |
| WorkbenchNativeWindowPlugin | Native appearance and application-owned close requests |
| WorkbenchNativeMenuPlugin | Native presentation of the active workspace's menu |
| WorkbenchNativePlugin | Group of native window/menu plugins |
| WorkbenchUiPlugin | Host adapter for workspace egui presentation |
| WorkbenchPlugin | Group of enabled core, clipboard, native and UI capabilities |

Default features are ui, native and clipboard. With defaults disabled, the core
compiles without a renderer, window backend, native menu library or OS clipboard.
Features control compiled dependencies; plugins control installed systems.

~~~rust,ignore
app.add_plugins(WorkbenchPlugin).add_plugins(UiPlugin);
// Or compose:
app.add_plugins((WorkbenchCorePlugin, ClipboardPlugin, WorkbenchUiPlugin, UiPlugin));
~~~

When using native window integration, configure Bevy WindowPlugin with
close_when_requested: false. CloseRequested leaves close/exit decisions with
the application.

## Workspace ownership

Each workspace owns a Bevy SubApp with its own world, resources, schedules,
registered systems, layouts, panels and command queues. The host owns the window,
renderer, clipboard and runtime registry. WorkspaceId identifies a host entity;
panel and layout Entity values are meaningful only inside their workspace world.

There is always exactly one active workspace. Closing it requires an explicit
live replacement. Closing the final workspace is rejected; the application can
replace it with an untitled workspace or exit. Workspaces controls lifecycle;
Workbench selects a world once for the lifetime of its borrow.

~~~rust,ignore
let mut runtime = WorkspaceRuntime::new("Project");
// The application chooses the boundary, plugins, services and persistence.
runtime.add_plugins(MyProjectPlugin);
let project = Workspaces::new(host_world).open(runtime);
Workspaces::new(host_world).activate(project)?;

let mut workbench = Workbench::new(host_world); // Active workspace
let reading = workbench.create_layout("Reading")?;
workbench.activate_layout(reading)?;

// An explicit selection never changes the active workspace.
let mut background = Workbench::with_workspace(host_world, project)?;
background.world_mut().insert_resource(MyProjectState::default());
~~~

Inside a workspace system, Workbench::new(world) operates on that system's own
world, including when it is inactive. It never looks up the host's active
workspace. Queues live in their originating world, so switching cannot retarget
commands or late task results. During UI drawing, queue lifecycle changes in
WorkspaceRequests; activation runs after drawing so the input router can release
old ownership. Opening a runtime does not activate it. WorkspaceIdentity supplies the host WorkspaceId
after opening; identity is available to Startup systems.

Workspace is a local resource containing the name and selected layout.
Workbench does not interpret a project directory, database path, settings store,
or persistence format. Each workspace has at least one layout. Layouts own areas,
which own panels through InLayout and InArea. Removing the workspace world
releases all of these entities and its plugin resources.

## Layouts, panels and UI

Attach the window in the host and register contents in the selected world:

~~~rust,ignore
let host = attach_surface(host_world, surface);
let mut workbench = Workbench::new(host_world);
let layout = workbench.active_layout();
let world = workbench.world_mut();
let panel = register_panel(world, layout, "Notes", draw_notes)?;
world.entity_mut(panel).insert(Notes::default());
let menu = world.spawn(MenuBar).id();
Workbench::new(world).set_menu(Some(menu))?;
let status_bar = Workbench::new(world).status_bar();

fn draw_notes(mut frame: bedtime_ui2::UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    frame.text_edit(egui::TextEdit::singleline(&mut notes.text));
}
~~~

The host lends its egui frame to the active workspace's shell and panel systems.
Draw systems execute in their workspace world and use ordinary local Queries and
Resources. Workspaces namespace egui identities, so equal local entity IDs do
not share widget state. Moving tabs retains panel and widget state.

UiFrame carries borrowed theme, icon, texture and input services. Its surface is
a host entity and must not be queried in the workspace world. Clipboard handles
share the host service. WorkspaceViewport components project local UiViewport
observations into host entities for texture registration and resizing.
Applications explicitly project simulation into Bevy render entities; ordinary
renderable entities in workspace worlds are not extracted automatically.
See bed2/src/viewport_demo.rs for an example.

WorkbenchHost configures the host's bounds, minimum area size and panel visibility.
WorkbenchPresentation reports the resulting titlebar/menu/panel/status allocations.
Menu and status contributions belong to each workspace: select a local menu with
Workbench::set_menu and register UI2 contributions on Workbench::status_bar().
The shell reserves native titlebar space, the optional egui menu and status bar.

Areas tile a normalized 0–10,000 integer plane. Area owns its rectangle and selected
panel; AreaPanels stores tab order. Workbench methods alone commit topology and
membership changes. Switching layouts retains state; closing retires instances.
These operations do not perform application save/discard preflight.

Panel content is clipped to its content rectangle and the area's rounded outline,
including viewport images and custom painting. The host enforces this boundary;
plugins do not need to match their own corner radii. Popups and tooltips retain
their separate overlay layers.
Panel content has no implicit padding or scroll area. Plugins opt into UI2's
padded and scroll helpers. PanelPresentation and AreaPresentation report local
visibility, focus and geometry. PanelSettings controls gaps, padding, rounding
and corner targets independently of the theme.

The frontend retains Blender-style interactions:

- Drag tabs to reorder, group or split. Moving a final tab fills its vacated area;
  closing a final tab leaves an empty area.
- Drag area corners inward to split, across a border to join, or farther into
  another area to relocate. Partial joins preserve target tails.
- Drag shared borders to resize connected spans, including T-junctions.
- Close with x, middle click or the tab menu. Double click keeps a preview tab.

Gestures cancel on Escape, right click, focus loss, activation or bounds changes.
Resize previews commit on release. Cross-layout moves, floating native panels
and persistent restoration remain outside this implementation.

Register panel factories through register_panel_type. PanelDefinition supplies
KEY and LABEL; its Bevy factory takes In<Entity> for the destination area and
returns Result<Entity, String>. It creates the panel using register_panel_in_area
or create_panel_in_area, inserts local state and returns its entity.
PanelType and ActionEnabled live on the registered system entity. Empty areas
present the available factories. panel_drag_source provides typed drag payloads.

Drawing queues WorkbenchRequests in the originating world. WorkbenchSystems::Apply
commits a batch after contextual command dispatch and publishes WorkbenchFinished.
Gesture requests include layout revision and activation stamps; stale edits fail.

## Execution and external work

WorkspaceSystems::Prepare handles plugin readiness/finish/cleanup, Startup once,
and schedules preceding PreUpdate before drawing. WorkspaceSystems::Dispatch
runs the active world's PreUpdate after drawing, before UI2 publishes input.
WorkspaceSystems::Update runs the remaining MainScheduleOrder schedules.
Background workspaces use the same order, each once per host frame.
Workspace clocks use host frame durations and support fixed schedules.
Install window, renderer and native plugins in the host; plugins that create
additional SubApps are not supported inside a workspace runtime.

Workspaces::set_background_updates(id, false) suspends the inactive world's ECS
schedules. Owned workers keep servicing process I/O and retain bounded output.
Switching releases old input capture and held keys before equal local entity
IDs appear in another workspace.

~~~rust,ignore
let mut w = Workbench::new(host_world);
let task = w.work().spawn_task(Some(panel), "Search", |cancel| {
    if cancel.wait(Duration::from_secs(2)) { return None; }
    Some(compute_result())
})?;
let process = w.work().spawn_process(
    Some(session), "Tool", ProcessSpec::new("my-tool"),
)?;
~~~

WorkspaceWork owns workers independently of result/I/O handles. An optional local
owner entity gives work a shorter lifetime: removing the owner cancels its work
on the next host update, even in a suspended workspace. Typed WorkspaceTask handles
receive results and distinguish Finished, Cancelled and Failed outcomes. In-process
cancellation is cooperative.

WorkspaceProcess supplies bounded stdin, drained stdout/stderr, exit status and
cancellation; close_stdin delivers EOF after queued input. Output retains the newest 256 KiB. Each input write is limited to
8 KiB, with up to 16 queued writes. These are pipe processes; terminal emulation
and PTY setup belong to a terminal backend. Custom backends can live in scoped
tasks and shut down when Cancellation is requested.

After application save/discard preflight, call Workspaces::close(id, replacement).
It switches to the replacement atomically, rejects access/spawns in the closing
workspace, requests cancellation and polls cleanup without blocking the UI.
Its world and host handle retire after managed work finishes. Unix children have
their own process group: shutdown sends TERM, then KILL after 250 ms, and reaps
the child. Other platforms terminate the direct child; descendant process-tree
supervision is currently Unix-specific. Cooperative tasks must return after
cancellation. Drop force-stops owned processes as a fallback.

For application exit, call Workspaces::shutdown_work(), keep updating until
work_finished(), then emit AppExit. Persistence, settings, save/discard policy
and remote services remain application-owned or future work.

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
Areas give up reassigned space immediately and animate only into newly available
space. A tab drop that splits an area's width while expanding its height snaps
the width and animates the height independently. Corner splits show both halves
immediately; direct border dragging follows the pointer.
While dragging a corner within its own area, the split line follows the pointer
without easing or minimum-size clamping. Releasing clamps the actual split to
the minimum panel size.
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
