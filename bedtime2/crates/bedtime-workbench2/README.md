# bedtime-workbench2

Entity-backed workspaces, retained layouts, panels and typed commands.
`WorkbenchPlugin` always establishes an active temporary workspace and one layout.
The default build runs headlessly without a renderer. Enable `ui` to attach its
ImGui frontend to an existing `UiSurface`; applications install the UI/desktop
plugins and Bevy's renderer themselves.

Documents/history, persistence, settings and shortcut routing are later slices of
[stage 3](../../../MIGRATIONS/step-3.md). There is no file, process or remote service
in this crate.

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

## Register a command

```rust,ignore
struct SaveDocument;
impl CommandDefinition for SaveDocument {
    const KEY: &'static str = "example.save_document";
    const LABEL: &'static str = "Save Document";
}

let mut workbench = Workbench::new(app.world_mut());
let save = workbench.register_command::<SaveDocument, _>(save_document)?;
```

The trailing `_` infers Bevy's system-parameter marker. Handlers are ordinary
Bevy systems, including exclusive systems, with `In<CommandInvocation>` input and
`CommandResult` (`Result<(), String>`) output. Their resources and queries are
injected normally by Bevy.

Registration decorates Bevy's registered-system entity with `CommandInfo` and a
required `ActionEnabled(true)` component. That entity is the returned runtime
handle. Metadata and lookup live in ECS; there is no parallel command registry.

`CommandDefinition::KEY` is a stable, namespaced string. Keys require at least
two dot-separated segments containing ASCII letters, digits, `_` or `-`.
Registration rejects duplicate keys or duplicate definition types before installing
a handler. `LABEL` provides initial human-readable command metadata.

Use `workbench.command::<SaveDocument>()` for typed lookup. `CommandInfo::key()`
returns a `CommandKey`, which serializes as a plain string. Deserialize that key
and use `command_by_key(key.as_str())` to resolve a current entity after restart.
Unknown keys return `None`; saved shortcut storage and missing-plugin retention
policy remain future work. Runtime `Entity` and `TypeId` values are never persisted.

## Present and invoke a command

A desktop `MenuItem` references the returned entity through `Activates(save)`.
Menu labels and shortcuts belong to their placements. `ActionEnabled` belongs to
the shared action and affects all its menu presentations and dispatcher admission.
Removing a menu does not remove the command; despawning the command detaches its
presentation references.

Native and UI activations targeting registered commands are collected in
`PostUpdate`, in `CommandSystems::Ingress`, before dispatch. With the `ui` feature,
workbench orders UI drawing before ingress automatically, so UI actions execute
in the same frame after drawing ends. Other action entities remain available to
application consumers. Activations retain their source and target. Application systems can also
submit through the durable `CommandRequests` resource:

```rust,ignore
requests.invoke(save);
requests.submit(CommandInvocation { command: save, source: Some(panel) });
```

The sole dispatcher runs in `PostUpdate`, in `CommandSystems::Dispatch`. It
processes the current FIFO batch, checks that each command still exists and is
enabled, executes the handler, and emits `CommandFinished` with its result.
Handler errors do not prevent later requests from executing. Command execution
is never a relationship hook or an immediate native callback.

Requests enqueued by a handler remain queued until the next dispatch pass. This
prevents recursive execution and leaves the state transition boundary explicit.
Invocations carry source observations; handlers must capture any additional
operation context they need rather than resolving it from later UI focus.

Results are ordinary Bevy messages, intended for consumers to read promptly.
The initial system has no asynchronous command barriers, durable result receipts,
argument serialization or shortcut editor.

```sh
cargo test --locked -p bedtime-workbench2 --lib
cargo run --locked -p bed2
```
