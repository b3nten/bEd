# bedtime-workbench2

Composable workspace entities with optional native and egui presentation.
Add WorkbenchPlugin and UI2's UiPlugin after the application's normal Bevy setup,
in either order. Applications compose concrete workspace and panel entities;
an empty workbench is valid. Workbench never installs
DefaultPlugins or creates a window or GPU device. Adding
`WorkbenchSurface::default()` to a window creates the default shell and wires an
application-supplied camera marked `WorkbenchCamera`.

| Plugin | Responsibility |
| --- | --- |
| WorkbenchCorePlugin | Workspace entities, activation markers, commands, documents and shutdown |
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

// No panel-type registration is needed.
~~~

On macOS with `ui`, the native window adapter also supplies live file-drag
coordinates. Winit 0.30.13's `HoveredFile`/`DroppedFile` events carry paths but no
pointer movement, so ordinary egui mouse position can be stale during Finder
drags. The adapter samples the AppKit content-view position on hover frames and
the final drop frame, converts to Bevy logical coordinates (including scale-factor
overrides), and emits ordinary cursor messages before egui input processing. It
requests redraws while hovering and retires hover state on cancellation/drop or
window closure. Drop-only automation injection retains its supplied cursor; it is
not a Finder-drag test.

When using native window integration, configure Bevy WindowPlugin with
close_when_requested: false. CloseRequested leaves close/exit decisions with
the application.

## Component composition

Workbench uses the application's World and renderer. Local defaults are required
components: `Workspace` supplies work queues, lifecycle state and (with UI enabled)
`WorkspaceUi`; `Layout` supplies its area collection and revision state;
`Area`/`PanelInstance` supply presentation components. Concrete panel components
require `PanelInstance::of::<Self>()` to supply metadata and drawing immediately.
Application-specific state can be composed in the same spawn. No facade calls are required:

```rust,ignore
fn setup(mut commands: Commands) {
    let workspace = commands.spawn((
        Workspace { name: "Project".into(), ..default() },
        ActiveWorkspace,
        Session::default(),
    )).id();
    let layout = commands.spawn((
        Layout { name: "Code".into(), ..default() },
        InWorkspace(workspace),
        ActiveLayout,
    )).id();
    let area = commands.spawn((Area::default(), InLayout(layout))).id();
    commands.spawn((
        Notes { text: "Initial draft".into() },
        InArea(area),
    ));
}
```

`ActiveWorkspace` is authoritative, with at most one globally. `ActiveLayout`
is authoritative within each workspace, with at most one per workspace. There
can be zero of either. A bare workspace creates no layout; a bare layout creates
no area. `Area::default()` covers the complete layout. Several areas must form a
complete nonoverlapping tiling in integer `0..LAYOUT_EXTENT` coordinates. Areas
can contain zero panels. Attaching a new concrete panel selects its tab in the
destination area without changing the layout's focused area. Missing focus/selection
defaults to the first area/tab. Removing or moving the selected panel chooses a
remaining tab near its former index; removing the focused area chooses a remaining
area. `Workbench::select_panel` selects a tab, `focus_area` changes
area focus, and `focus_panel` is an explicit convenience that does both. Conflicting
live references or duplicate active selections fail clearly at reconciliation.

Lifecycle hooks derive area/panel `InWorkspace` from their parents and maintain
`InActiveWorkspace` as selection and membership operations are applied. This
includes spawning, reparenting, removing relationships and despawning. Direct
World calls finish with membership current; deferred `Commands` do so when
applied. Chained Update systems can change selection in one system and query
the new membership in the next without waiting for a workbench schedule stage.
Keep deferred writers and readers ordered normally; commands that have not yet
been applied are not visible to queries.

Lifecycle hooks also maintain selection/focus fallbacks as relationships change.
Scheduled reconciliation validates the graph and invalidates stale layout gestures
before drawing, after queued workbench/workspace operations, and in PostUpdate.
It is not an initialization pass: required components supply defaults, metadata
and drawing as soon as composition applies. `reconcile_workbench(world)` is
available for immediate validation. Keep structural edits valid at that boundary.

`InWorkspace`/`WorkspaceMembers`, `InLayout`/`LayoutAreas` and
`InArea`/`AreaPanels` are Bevy relationships with linked despawning. Workspace
members retire with their workspace; areas/panels also retire with their layout.
Moving a layout, area or panel updates its descendants' workspace ownership as
the relationship change is applied, before subsequent despawns can act on the
old workspace's ownership list. Use `Workbench::relocate_panel` for checked moves
between layouts/workspaces. Cross-workspace relocation rejects drafts and
panel-owned work and detaches `DocumentView`; documents do not migrate implicitly.
Detach other application-owned scoped links before calling. Raw cross-workspace
reparenting of a bound document viewer is unsupported and rejected by validation.
Application editor entities can use `InWorkspace` directly. Ordinary game
entities need no workbench components. Workspace-specific state belongs on the
workspace entity; resources remain application-wide.

```rust,ignore
fn active_sessions(mut sessions: Query<&mut Session, With<ActiveWorkspace>>) {
    for mut session in &mut sessions { session.ticks += 1; }
}
fn active_notes(mut notes: Query<&mut Notes, With<InActiveWorkspace>>) {
    for mut note in &mut notes { /* ... */ }
}
// The displayed layout, rather than each workspace's remembered selection:
fn current_layout(layout: Single<Entity, (With<ActiveLayout>, With<InActiveWorkspace>)>) {}
```

`WorkspaceRunning`/`InRunningWorkspace` include the active workspace and inactive
workspaces with `Workspace.background_updates` enabled. These derived markers
scope queries; they do not suspend arbitrary systems. Managed worker I/O continues
regardless of these markers. Applications should not write derived markers.
Selection and relationship changes maintain these markers through lifecycle
hooks. In-place changes to mutable `Workspace.background_updates` are reflected
by the next reconciliation pass; `Workspaces::set_background_updates` resolves
them immediately.

No selected workspace/layout is a normal state: the shell draws its available
content and skips missing panel layout content. Operations needing a destination
return an error. The existing `Workbench`/`Workspaces` convenience APIs remain;
their strict `new`/`active`/`active_layout` accessors still require a selection.
`Workspaces::selected()` and `active_layout(world, workspace_entity)` return
`Option`. `Workspaces::open` remains a convenience that creates a default layout
and area, while direct component composition creates only what was spawned.
Closing the final workspace or deleting its final layout is allowed. Use queued
requests for activation/closure during drawing so that drawing can finish first.

## Application-owned sessions and close decisions

Workbench provides live composition and layout transitions, not a generic session
format. Core workspace/layout snapshots, capture/restore/replace APIs, layout
templates and snapshot-derived `WorkspaceChanged` notifications have been removed.
Applications own validation, persistence formats, storage, defaults, creation
choices, restore descriptors and reconstruction of their concrete panels and
documents. Observe relevant ECS state or application transitions explicitly when
implementing persistence; there is no replacement generic serialization registry.
See [application composition](../../../MIGRATION/application-composition.md) for
the accepted design and unfinished application work.

Destructive UI layout intentions emit `PanelCloseRequested` rather than granting
permission to destroy views. The application may reject a request, retain it while
prompting/saving, or approve it with `Workbench::commit_close(&request)`. The commit
revalidates the captured layout/activation stamp and targets, so delayed approval
cannot silently close a changed layout. `close_targets(&request)` is a read-only
policy preflight that uses the same geometry planner as commit, including the exact
panels removed by joins/docks. Activation hooks invalidate stamps immediately on
marker changes, including leave-and-return before reconciliation. This includes
panel/area closure and destructive joins/docks. Workbench owns structural and document preflight checks;
the application owns the user-facing decision and recovery policy.

Raw entity despawn still performs ownership/worker cleanup, with no veto and no
save prompt. It is not a safe user-facing close-decision API. The controlled
workspace/document APIs below retain their explicit preflights.

## Documents

[`bedtime-editor2`](../bedtime-editor2/README.md) optionally adapts its persistent
`TextBuffer` model to these same sessions/history and supplies embeddable text
views. Applications retain file/source policy. `document_changes_since` exposes
a bounded, independently readable journal of all accepted content transitions,
including undo/redo and whole-model invalidations; readers refresh on a gap.

Workbench owns workspace document sessions independently of their panel viewers.
The application creates and loads models, supplies source observations and save
I/O, and owns source identity, storage, merge and recovery policy. Documents use
the core crate only: no renderer, window backend, native library, filesystem
watcher or storage provider is involved. The full contract, invariants and
deferred requirements are in [the workbench documents
record](../../../MIGRATION/workbench-documents.md).

`open_document` creates a session in a workspace. `String` is a built-in model
with UTF-8 byte-range edits. `DocumentOptions::loaded` marks the supplied model
as the acknowledged source checkpoint; `DocumentOptions::unsaved` supplies no
checkpoint, so even untouched content is dirty. `DocumentOptions::default()` is
`unsaved("Untitled")`.

```rust,ignore
let text = open_document(world, workspace, "Initial".to_owned(),
    DocumentOptions::loaded("notes.txt"))?;
let scratch = open_document(world, workspace, String::new(),
    DocumentOptions::unsaved("Untitled"))?;

#[derive(Clone, Default, PartialEq)]
struct Settings { name: String }
#[derive(Clone)]
enum SettingsChange { Name(String) }
impl bedtime_workbench2::DocumentModel for Settings {
    type Change = SettingsChange;
    fn apply(&mut self, change: &SettingsChange) -> Result<SettingsChange, String> {
        // Run on an isolated clone; return the inverse; do no I/O.
        let SettingsChange::Name(name) = change;
        Ok(SettingsChange::Name(std::mem::replace(&mut self.name, name.clone())))
    }
}
let settings = open_document(world, workspace, Settings::default(),
    DocumentOptions::loaded("Settings"))?;
```

`DocumentModel` requires `Clone + PartialEq + Send + Sync + 'static`, and its
`Change` type is `Clone + Send + Sync + 'static`. A clone is the editable
candidate, so `Clone` must isolate mutable state (not share it with the
canonical model) and `apply` must not perform I/O or mutate anything outside its
own model. `PartialEq` defines clean content: the document is clean exactly
when the model equals its acknowledged source baseline. `source_key` deduplicates
reopening only within one workspace; it returns the existing session rather than
replacing contents, and a model-type mismatch fails.

Bind a panel with the typed handle. The panel must be a `PanelInstance` in the
same workspace; other layouts in that workspace can bind the same session, and
another workspace cannot:

```rust,ignore
bind_document_view(world, panel, text)?; // Inserts DocumentView, updates PanelInstance.dirty.
```

The session, not the viewer, owns content, revision and history. Closing a viewer
never closes the document; `document_ids(world, workspace)` lists sessions and
`cast_document::<M>` recovers a typed handle.

Edits take the expected revision and return the new one. Revisions start at 0,
advance once per accepted content change and reject stale expectations. Revision
is monotonic but is not a saved-state id: compare `DocumentInfo.dirty` against
the acknowledged baseline instead. `TextChange` ranges are UTF-8 bytes, and both
ends must be character boundaries.

```rust,ignore
let info = inspect_document(world, text.id())?;
let revision = edit_document(world, text, info.revision,
    TextChange { range: 3..3, replacement: "!".into() },
    EditOptions::new("Typing"))?;
edit_document_transaction(world, text, revision,
    [first_change, second_change], EditOptions::new("Paste"))?;
```

A transaction applies all changes to successive states of one isolated clone and
commits one revision and undo entry; a failing change changes nothing. No-op
transactions add no revision or history. `history_limit` defaults to 100 and 0
disables history, not editing. `undo_document`/`redo_document` return the new
revision and reject drafts.

Group consecutive edits into one undo entry with an explicit group. Save start,
external observation, undo/redo, rebinding or an ungrouped edit closes the group,
and a closed group cannot be reopened:

```rust,ignore
let group = begin_document_undo_group(world, text.id(), panel)?;
edit_document(world, text, revision, change,
    EditOptions::grouped("Typing", group))?;
end_document_undo_group(world, text.id(), group)?;
undo_document(world, text.id())?;
redo_document(world, text.id())?;
```

`DocumentDraft` on a viewer declares uncommitted input. Insert it while the input
is uncommitted and remove it only after committing or abandoning that input:

```rust,ignore
world.entity_mut(panel).insert(DocumentDraft);
world.entity_mut(panel).remove::<DocumentDraft>();
```

While any viewer has a draft, save, undo, redo, document close/discard and
workspace or panel/layout closure reject with `Drafts`. `DocumentInfo` exposes
`has_drafts` and disables `can_undo`/`can_redo`; reconciliation derives
`PanelInstance.dirty` from document dirtiness or the viewer's draft.

Saves are asynchronous and typed. Bind one callback per document, then start a
save. The callback receives an immutable snapshot of the content captured at
save start, its revision and source generation, and the acknowledged source
version at that time. It runs in workspace-managed work, so it may block on
application I/O.

```rust,ignore
bind_document_save(world, text, DocumentSaveBinding::new(
    Some("project/notes.txt".into()), // Serializes equal destinations across the World.
    |snapshot| {
        // Application code: conditionally write snapshot.model, checked against
        // snapshot.expected_source_version. Return the new token (or None), and
        // return DocumentSaveError::Conflict when the backend refuses the write.
        Ok(None)
    },
))?;
let work = save_document(world, text.id())?;
```

Destination keys serialize saves across every document and workspace in the
World; an absent key serializes only within that document. Conditional writes
remain the backend's responsibility: an absent or unchecked token provides no
overwrite protection. One save runs per document, and a save, rebind, close or
discard while it is in flight is rejected.

The installed polling system runs before `WorkspaceSystems::Prepare` and emits
`DocumentSaveFinished` messages; `poll_document_saves` returns completions
directly without blocking. Success acknowledges only the captured model (later
edits stay dirty) and stores the returned token. Failures preserve content and
history. `Conflict`, `Panicked` and `Cancelled` require a fresh source observation
before retry; `SourceChanged` means the write may have succeeded but newer source
knowledge cannot be acknowledged. These uncertain receipts advance the source
generation so an older read cannot masquerade as the required refresh. Retained
conflicts cannot be resolved until that refresh is supplied.

External source changes are offered with the expected source generation.
`SourceVersion` is an opaque application token, not a timestamp. The component
that opens and observes a source supplies events in source order; Workbench gates
them by generation but does not reorder, coalesce or debounce them.

```rust,ignore
let generation = inspect_document(world, text.id())?.source_generation;
let update = offer_document_external_update(world, text, generation,
    loaded, Some(version))?; // Applied, Acknowledged, Duplicate or Conflict.
```

A stale generation rejects atomically with `StaleSourceGeneration`, so an async
read started before a save or refresh cannot overwrite newer knowledge. A clean
document is replaced with an undoable external update. A dirty document retains
local content and records a conflict; later observations do not replace local
content automatically, though they may acknowledge it when it already matches
the incoming content. Otherwise, inspect and resolve the conflict explicitly
with the current revision and source generation:

```rust,ignore
if let Some(conflict) = document_conflict(world, text)? {
    resolve_document_conflict(world, text,
        conflict.revision, conflict.source_generation,
        DocumentConflictResolution::Merged(merged))?;
}
```

`AcceptIncoming`, `KeepLocal` and `Merged` all acknowledge the incoming token;
`Merged` is an application-supplied result, not a built-in merge algorithm.
Resolution does not write to the source, and a stale revision or generation is
rejected. If `requires_source_refresh` is true, fetch and offer a new observation
first; accepting a cached conflict is not a substitute for that refresh.

A watcher may echo a write before its save receipt arrives. Matching captured
contents are retained without replacing local edits or undo state, then
acknowledged only when the successful receipt's source token also matches.

Controlled lifecycle calls close every bound viewer after validation:

```rust,ignore
check_document_close(world, text.id(), false)?; // Inspect only.
close_document(world, text.id())?;              // Clean, conflict-free, not saving/drafted.
// Alternatively, after an explicit discard decision:
// discard_document(world, text.id())?;
preflight_workspace_close(world, workspace, WorkspaceCloseMode::Saved)?;
let owner = Workbench::for_entity(world, workspace)?.workspace_id();
Workspaces::new(world).close(owner, replacement)?;
```

`WorkspaceCloseMode::Saved` requires clean, conflict-free documents;
`Discard` allows dirty content and conflicts. Both always reject in-flight saves
and drafts. `Workspaces::close` and `discard_and_close` run that preflight and
return an error without changing selection or retiring anything. Applications
can call the read-only preflight before showing their own save/discard/cancel
UI, then again after asynchronous saves. Direct raw entity despawn bypasses these
checks; the controlled APIs enforce them.

Document handlers and shortcuts are explicit application choices, not installed
by the core plugin (which supplies their command definitions). The supplied world handlers act on the focused panel's
`DocumentView` and fail without rerouting when no document viewer is admitted:

```rust,ignore
register_document_handlers(world)?; // SaveDocument, UndoDocument, RedoDocument.
register_document_shortcuts(world); // Primary+S, Primary+Z, Primary+Shift+Z, Primary+Y.
```

Text viewers must route undo/redo before their `egui::TextEdit` handles those
chords and call `undo_document`/`redo_document`. The general shortcut collector
deliberately leaves text-editing chords to widgets, so a widget-private history
would diverge from the document; use the same document undo instead.

Session persistence is application-owned. Workbench has no workspace/layout
snapshot or restore API. A host restoring a session must validate its own data,
compose layouts and concrete panels, reopen documents and bind viewers explicitly.
Application restore descriptors may contain title, source key, model contents/token
and target tab; `source_key` deduplicates documents within a workspace.

Replacing a save binding is not a complete Save As workflow: source identity,
any chooser and source-key changes remain application-owned. Workbench provides
no filesystem watcher, storage chooser/provider, merge algorithm, history
persistence or automatic crash recovery.

## Error dialogs (`ui` feature)

Request user-facing errors independently of the panel or workspace that noticed
an operation fail:

```rust,ignore
request_error_dialog(world, ErrorDialog {
    title: "Could not open the file".into(),
    message: "The file is no longer available. Refresh Files and try again.".into(),
    details: Some(error.to_string()),
});
```

Callers own domain-specific titles, understandable messages and any recovery
advice. Raw I/O/debug diagnostics belong in optional `details`, not the main
message. Workbench presents a nonblocking egui modal with an OK button;
Enter/Escape also dismiss it. Technical details start collapsed and use
selectable monospace text. There is no native alert, new window, logging service
or general notification framework.

`request_error_dialog` lazily creates the `ErrorDialogs` resource without any
plugin, workspace or window requirement. `len`, `is_empty` and `front` inspect
retained dialogs, including the active one. Requests survive a headless/no-UI
period and outlive their source panel/workspace. The workbench root presents
one dialog before drawing shell/panels, even with hidden panels or no selected
workspace. It retains the first available workbench window until dismissal,
rehoming the dialog if that window/surface closes. The shell is disabled during
the first, active and closing frames; dismissal Enter/Escape events are removed
before underlying panel gestures run. Bevy input remains application-owned.

The queue holds at most 16 entries including the active dialog. Identical
active/pending requests are deduplicated. If full, its last pending error and
new requests become one additional-error count, without retaining their
identities or diagnostic details. Further requests coalesce into that summary
until it is dismissed; its count describes omitted requests, not distinct
errors. Earlier retained duplicates do not increase the count. A dismissed
error can be requested again, so callers must edge-detect persistent failures
rather than resubmitting them on every draw/poll. Errors requested during drawing
appear on the next root pass. The module/API are absent with `ui` disabled; the
standalone core remains renderer-independent.

## Layouts, panels and UI

Tag the window and explicitly supply the workbench camera. Workbench supplies a
dark theme, UI surface and shell:

~~~rust,ignore
world.entity_mut(window).insert(WorkbenchSurface::default());
world.spawn((Camera2d, WorkbenchCamera));
~~~

Configure panel appearance and interaction directly on `WorkbenchSurface`.
Supply `UsesTheme` or `WorkbenchShell` on the same window to configure them
explicitly. Mark an existing application camera instead of
spawning a new one if appropriate:

~~~rust,ignore
world.entity_mut(camera).insert(WorkbenchCamera);
~~~

Exactly one camera must be marked `WorkbenchCamera`, including cameras that
already have `UiCamera(window)`. `WorkbenchCamera` requires `UiCamera` (and its
`EguiContext`) immediately. Setup resolves its initially unbound window link;
it never creates or selects an unmarked camera. The application owns the camera and its
render order. Add `InView(window)` if it should retire with the window. Supplied
themes remain application-owned; a generated default theme retires with the
window. Missing or duplicate marked cameras, duplicate workbench surfaces and
conflicting render targets fail clearly. UI plugins still require the
application's Bevy renderer/window plugins.

Setup runs automatically in `PreUpdate`, after application `Startup` and before
`bedtime_ui2::UiSystems::Configure`. Inserting components is sufficient; no setup
helper or binding resource is needed. Systems that need the completed UI link,
theme or view can run after `UiSystems::Configure`, or in `Update`. Query
`With<WorkbenchSurface>` for the window and `With<WorkbenchCamera>` for its camera.
`WorkbenchShell`, `UiSurface` and a transparent `UiBackground` are required
defaults available immediately when the surface component is inserted. An
explicit `UiBackground` overrides that default.

Removing `WorkbenchSurface` immediately retires its generated root renderer and
default theme, restores its camera target and Quit shortcuts, and detaches a UI
camera binding that workbench supplied. UI2 restores camera clear/output,
compositing and context settings and ends that egui session when the binding is
removed. The application
window and camera remain alive. Supplied `UiCamera` bindings, themes and unrelated
views are preserved. Required/default components on the window remain in place;
reinserting `WorkbenchSurface` recreates its wiring in the next configuration
pass. Despawning the window performs the same cleanup for its surviving camera.

Every workbench content instance is a panel. `Panel` is the rendering/type
contract; `PanelInstance` is the ECS metadata attached to each instance:

~~~rust,ignore
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
struct Notes { text: String }

impl bedtime_workbench2::Panel for Notes {
    const KEY: &'static str = "example.notes";
    const LABEL: &'static str = "Notes";
    fn draw(ui: &mut egui::Ui, world: &mut World,
            context: &bedtime_ui2::UiContext<'_>, panel: Entity) {
        let mut notes = world.get_mut::<Notes>(panel).unwrap();
        context.frame(ui).text_edit(egui::TextEdit::singleline(&mut notes.text));
    }
}
commands.spawn((Notes { text: "Draft".into() }, InArea(area)));
// Required components preserve explicitly supplied component overrides.
~~~

Panel draw methods are ordinary Rust calls with exclusive world access, not
registered Bevy systems. Panels compose `bedtime_ui2::Component` renderers with
arbitrary associated props. The shell, panels, menus, titlebars and status renderers all draw in the
application World during the same egui pass. Scheduled input,
commands and updates remain Bevy systems. `Panel` has no initializer. Bevy
required components supply only missing state, preserving explicit overrides in
the spawn. `PanelInstance::of::<P>()` carries `P`'s label, descriptive key and draw
function on that same entity. `Panel::KEY` is retained as descriptive application
identity (an empty key is usable), not a construction registry or validated type ID.
Workspace and entity identities keep widget state distinct. Moving tabs retains
panel and widget state.

`WorkbenchSurface` owns panel rounding, suggested padding, gaps and corner hit
targets, along with single-area edge-to-edge presentation, tab visibility, corner
dragging and user permissions. Configure it directly or change its fields from
ordinary systems:

~~~rust,ignore
world.entity_mut(window).insert(WorkbenchSurface {
    drag_corners: false,
    tab_bar: TabBarVisibility::Hidden,
    ..default()
});
~~~

Disabling `drag_corners` removes corner hit targets and cancels any ongoing
corner gesture. Divider resizing and other layout operations remain available.
Hidden tab bars reserve no space; each area's selected panel continues drawing
and can be selected through application operations. Hiding the bars also ends
any ongoing tab drag. These settings apply across workspace and layout switches.

By default one tiled area has no border, rounding or gaps (even with several
tabs). For a fixed panel presentation, `WorkbenchSurface::single_panel()` also hides
the tab bar and disables user layout editing, creation and closure:

~~~rust,ignore
world.entity_mut(window).insert(WorkbenchSurface::single_panel());
~~~

The same panel identity, focus and input path remains active. Application APIs
can still create/select/rearrange panels. `TabBarVisibility` supports `Always`,
`MultipleTabs` and `Hidden`. User gestures submit `WorkbenchRequest::UserEdit`;
`Workbench::user_edit` applies the same permission policy to application-provided
user commands. Ordinary `Edit` requests and direct application operations remain
available. Custom command handlers should use `user_edit` for user layout actions.
Content padding and scrolling remain opt-in. A panel can query
`world.get::<WorkbenchSurface>(context.surface)` for the surface's suggested
padding and rounding, then pass them explicitly to UI2's `padded` and `scroll`
helpers. Workbench configuration is not part of `UiFrame` or `UiContext`.

UiFrame carries borrowed theme, icon, texture and input services. Its surface,
view and panel entities belong to the application World. Panels can inspect game
entities and shared UI components directly. `UiViewport` displays a normal Bevy
image; no viewport proxies or scene duplication are required:

```rust,ignore
let image = world.resource_mut::<Assets<Image>>()
    .add(bedtime_ui2::viewport_image(UVec2::new(640, 400)));
world.entity_mut(game_camera).insert(RenderTarget::Image(image.clone().into()));
world.entity_mut(panel).insert(bedtime_ui2::UiViewport::new(image));
```

UI2 resizes visible image targets before camera updates. Application camera
routing belongs in `UiSystems::SyncViewports`. Applications decide which cameras
are active and how gameplay pauses or steps. Additional preview scenes can use
render layers with the same renderer. Workbench does not create, tick or own
child Apps/SubApps. A plugin that needs isolated simulation/rendering owns that
integration; UI2's independent `EmbeddedRenderPlugin` remains available. See
[UI2 viewports](../bedtime-ui2/README.md#icons-and-viewports).

`WorkbenchShell` on the window configures bounds, minimum area size, panel
visibility and titlebar ownership. It remains stable across
workspace switches. `WorkbenchPresentation` reports titlebar/menu/panel/status
allocations, including usable `titlebar_content`. Titlebar, menu and status
renderers belong to each workspace. Select a local menu with
`Workbench::set_menu`. The status bar is absent by default and reserves no space
until the application registers its drawing function:

~~~rust,ignore
let status = Workbench::new(world).register_status_bar(draw_status);
world.entity_mut(status).insert(ApplicationStatus::default());

fn draw_status(ui: &mut egui::Ui, world: &mut World, context: &bedtime_ui2::UiContext<'_>) {
    let status = world.get::<ApplicationStatus>(context.view).unwrap();
    ui.label(&status.message).on_hover_text("Application status");
}
~~~

The renderer owns all row contents, layout and tooltips; it can compose ordinary
egui widgets and UI2 components. The shell supplies a padded row at the bottom
of its allocation and draws it once per frame. The row height accounts for themed
text and interaction height, with two-point top/bottom insets and compact
button padding capped at one point vertically. Widgets share the row's full
vertical bounds and center their contents. Panel button padding does not
inflate the strip. `register_status_bar` replaces
the previous renderer and its owned state. `status_bar()` returns its optional
entity; `remove_status_bar()` removes it and restores the full panel height.
The renderer and its state retire with their workspace.

Add `Titlebar` to the same window as `WorkbenchSurface` to provide titlebar
content. The component holds the requested height and drawing function directly:

~~~rust,ignore
world.entity_mut(window).insert((
    Titlebar {
        height: 20.0,
        draw: draw_titlebar,
    },
    ApplicationChrome::default(),
));

fn draw_titlebar(ui: &mut egui::Ui, _: &mut World, context: &bedtime_ui2::UiContext<'_>) {
    let mut frame = context.frame(ui);
    frame.ui.label("My application");
    if frame.icon_button(bedtime_ui2::BuiltinIcon::Plus).clicked() {
        // Submit the application's operation.
    }
}
~~~

`Titlebar` requires `WorkbenchSurface`, supplying its defaults if it is absent.
The shell reads the component each frame; no registration call or renderer
entity is needed. Drawing receives the window as both `UiContext.view` and
`UiContext.surface`, so custom state can be ordinary components on that window.
The renderer runs once per visible frame in the application World and owns the
complete layout inside a clipped child Ui. Egui identities follow the surface,
keeping titlebar widget state stable across workspace switches.
The host reserves extra space beside native controls, six points at
the horizontal window edges and four points above the content before calling it.
Popups retain their ordinary overlay bounds. Change `Titlebar.height` or
`Titlebar.draw` through normal component mutation; the height must be finite and
positive. Removing `Titlebar` removes the custom content and requested height,
retaining native chrome and the workbench surface. The titlebar belongs to the
window and continues drawing with no active workspace. Its callback can open or
select a workspace before the shell draws that workspace's body.
Application setup must create any state its drawing callbacks require.

`WorkbenchShell.titlebar_ownership` explicitly chooses who manages the titlebar:

- `TitlebarOwnership::Workbench` (the default) renders the optional slot, paints
  its background and handles background window dragging and native double-clicks.
- `TitlebarOwnership::Application` leaves titlebar painting, native appearance and
  gestures to the application. The `Titlebar` component remains in place while
  drawing is suspended; switching back resumes it. No slot height or padding is
  allocated in this mode. The measured native band is still reserved to keep the
  menu and panels below native controls and accessories.

An application installing its own native widgets can select ownership when it
inserts the surface components, before native/UI updates:

~~~rust,ignore
world.entity_mut(window).insert((
    WorkbenchSurface::default(),
    WorkbenchShell {
        titlebar_ownership: TitlebarOwnership::Application,
        ..default()
    },
));
// Install platform widgets on the application's native window.
~~~

This option does not create native widgets or change Bevy's window configuration.
The application owns their layout, lifetime, event handling and any bridge for
dragging native items into panels. Window backing color/blur and close-request
forwarding remain available through the native adapter in either mode.

With workbench ownership, on macOS the slot uses the full-size content view beside
the native traffic lights. Other platforms, ordinary content views and embedded
host bounds use the same renderer in a header inside the content area. Without a renderer, only
the intersecting native titlebar band is reserved. The host lays out titlebar,
menu, panels and status in that order; applications need no manual bounds edits.
Presses on unused titlebar background start an OS window drag. Widget presses,
including typed `panel_drag_source` payloads, remain owned by their widgets.
The macOS adapter disables AppKit's automatic dragging through the transparent
content view, so widget gestures cannot move the window independently of egui.
It applies a per-instance, storage-compatible view subclass overriding only
`mouseDownCanMoveWindow`; Winit's view identity, event handlers and Metal layer
are retained. Switching to application ownership restores the original view class
and native appearance; switching back reinstates workbench behavior. With
workbench ownership, background dragging remains active even without a registered
renderer. The native adapter also restores mouse-up after native drags
and respects the system double-click preference for minimize, zoom or no action.

Areas tile a normalized 0–10,000 integer plane. Area owns its rectangle and selected
panel; AreaPanels stores tab order. Workbench methods alone commit topology and
membership changes. Switching layouts retains state; closing retires instances.
Panel and layout closure reject bound viewers carrying `DocumentDraft`; saving
or discarding the underlying document remains application policy, and the
controlled document APIs enforce their own preflight. Direct raw despawn
bypasses these checks.

Panel content is clipped to its content rectangle and the area's rounded outline,
including viewport images and custom painting. The host enforces this boundary;
plugins do not need to match their own corner radii. Popups and tooltips retain
their separate overlay layers.
Panel content has no implicit padding or scroll area. Plugins opt into UI2's
padded and scroll helpers. PanelPresentation and AreaPresentation report local
visibility, focus and geometry. WorkbenchSurface controls gaps, padding, rounding
and corner targets independently of the theme.

### Per-item tab text

`PanelTabStyle` is an optional component on a **panel instance**, not its type
registration or area. It is plain core data and does not require the `ui` feature:

```rust,ignore
world.entity_mut(panel).insert(PanelTabStyle {
    italic: true,
    text_opacity: 0.9,
});
// Return to automatic normal/preview styling:
world.entity_mut(panel).remove::<PanelTabStyle>();
```

Without an override, normal labels inherit the theme unchanged; preview labels
are italic at 90% of their themed text opacity, with no underline. An explicit
component replaces both defaults: `PanelTabStyle::default()` means upright text
at full themed opacity, even on a preview. Opacity multiplies the current themed
selection/hover text color, including its alpha; it does not change icons, dirty
markers, close controls or hover backgrounds. Finite opacity values are clamped
to `0.0..=1.0` for rendering; nonfinite values use `1.0`. Font metrics remain
theme-owned. Tab width and painting use the same shaped label, including italics.

`PanelInstance.preview` remains semantic state. Styling never sets or clears it;
keeping or moving a preview still clears that flag. Explicit styles stay on the
same item when it moves or becomes permanent. Style insertion, edits and removal
are observed on the next draw without changing layout revisions. This is
**runtime-only appearance configuration**. Applications own any serialization
and reapply overrides when composing their panels.

The frontend retains Blender-style interactions:

- Drag tabs to reorder, group or split. Moving a final tab fills its vacated area;
  closing a final tab leaves an empty area.
- Drag area corners inward to split, across a border to join, or farther into
  another area to relocate. Partial joins preserve target tails.
- Drag shared borders to resize connected spans, including T-junctions.
- Close with x, middle click or the tab menu. Double click keeps a preview tab.

Gestures cancel on Escape, right click, focus loss, activation or bounds changes.
Resize previews commit on release. Interactive cross-layout dragging and floating
native panels remain outside this implementation; checked `relocate_panel` is an
explicit programmatic transition, not a cross-layout drag implementation.

Workbench has no `PanelType`, panel registry/catalog or built-in panel picker.
The application owns creation discovery (for example, a View menu) and directly
spawns its concrete components. For creation drags, pass an application-owned
`PanelFactory::new(label, create)` to `panel_drag_source`, where
`create: impl Fn(&mut World, Entity) -> Entity + Send + Sync + 'static` creates
exactly one panel in the supplied area and returns it. Constructors can capture
source data, such as a file path. `PanelFactoryPayload::with_content` retains
typed widget data alongside the constructor so content targets can consume the
drag before the shell offers area/tab/split targets. Eligible content targets call
`PanelFactoryPayload::reserve_for_content` each draw pass to retain hover-feedback
priority until release. Validate application-specific inputs before offering this
infallible factory; Workbench does not resolve descriptive keys to constructors.

Drawing queues WorkbenchRequests on the originating workspace entity. WorkbenchSystems::Apply
commits a batch after contextual command dispatch and publishes WorkbenchFinished.
Gesture requests include layout revision and activation stamps; stale edits fail.

## Execution and external work

`WorkspaceSystems::Prepare` validates composed graphs before drawing; it does
not initialize panels. Selection and membership hooks update query markers as commands
are applied. UI focus observations precede shortcuts, contextual
commands and layout edits. `WorkspaceSystems::Dispatch` then handles queued
workspace activation/closure requests. `WorkspaceSystems::Sync` validates those edits
before the application's normal Update schedule. There is one ordinary Bevy
plugin lifecycle, clock, fixed-update loop and renderer.

Workspace layout/command queues are components, accessible through
`Workbench::requests()` and `Workbench::commands()`, or ordinary component queries.
Queued operations retain their originating workspace when activation changes.
`ContextCommandInvocation.workspace` identifies that origin inside a handler.
Global resources are shared; workspace-specific handler data uses that entity.
`GlobalCommandRequests` is a distinct resource for application-wide command
requests, including commands when no workspace exists.

```rust,ignore
Workbench::with_workspace(world, project)?.commands().invoke::<Save>();
world.resource_mut::<WorkspaceRequests>().0.push_back(WorkspaceRequest::Activate(other));
```

No host/world forwarding is needed for shared services: an editor panel can
query application entities, change a theme, or inspect game assets directly.
Keep consequential lifecycle changes behind the Workspaces API or queued
WorkspaceRequests. In particular, queue activation while drawing so the current
workspace's presentation finishes before the selection changes.

Application exit policy can handle `CloseRequested` in PostUpdate. Managed work
is polled in Last, including in inactive and closing workspaces. Applications
own any gameplay input gating and held-key cleanup when switching workspaces.
Worker I/O remains independent of active/running query markers.

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

`Workspaces::close(id, replacement)` runs the `WorkspaceCloseMode::Saved` document
preflight itself and returns an error without changing selection or retiring
anything when it fails; `discard_and_close` uses `Discard`. Applications can use
the same read-only `preflight_workspace_close` before showing their own
save/discard/cancel UI, then close after asynchronous saves. On success it
switches to the supplied replacement (or clears selection), rejects access/spawns in the closing
workspace, requests cancellation and polls cleanup without blocking the UI.
Its workspace entity and linked members retire after managed work finishes. Unix children have
their own process group: shutdown sends TERM, then KILL after 250 ms, and reaps
the child. Other platforms terminate the direct child; descendant process-tree
supervision is currently Unix-specific. Cooperative tasks must return after
cancellation. Drop force-stops owned processes as a fallback.

For application exit, call `Workspaces::shutdown_work()?`; it preflights every
workspace with `Discard` and refuses in-flight document saves or uncommitted
viewer drafts before cancelling managed work. Keep updating until
`work_finished()`, then emit AppExit. Storage I/O, settings, save/discard policy
and remote services remain application-owned.

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
register_context_command::<Save>(world)?; // Metadata, independent of handlers.
install_global_handler::<Save, _>(world, save_application)?;
install_workspace_handler::<Save, _>(world, workspace, save_workspace)?;
install_panel_handler::<Save, _>(world, panel, save_panel)?;
world.spawn(Binding::new::<Save>(KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS)));
world.spawn((MenuItem::new("Save"), InMenu(file_menu), Invokes::of::<Save>()));
```

Installing a handler does not register command metadata. Definition registration
and handler installation work without a selected workspace. One handler per command
and scope is permitted. `Workbench::register_handler` is explicitly workspace-scoped;
the free `install_global_handler` installs a world handler. As a direct function
alternative to Bevy system handlers, attach `CommandHandler::<Save>::new(call)`
to its panel/workspace (or an unscoped entity for world scope). This installs no
command definition and runs no application code during composition. A panel's
`InPanel` ownership retires separately owned handlers, bindings and menu roots
when the panel closes. Menu descendants inherit their root's lifetime.

Bindings form a nearest-scope overlay by chord: panel, workspace, then world.
Handler lookup independently follows panel → workspace → world by command ID.
A world binding can use a panel handler; a panel binding can use a world handler.
Disabled local handlers block fallback. Handler errors and stale admitted targets
never reroute to another handler/binding. Admission captures handler generation and
scope identity, so replaced or retargeted handlers reject an old request rather
than executing the new capability under stale admission.

Modifiers match exactly; PRIMARY resolves to Cmd on macOS and Ctrl elsewhere.
Chords retain physical Bevy KeyCode identity. Repeats default to disabled. A
selected binding reserves its input even when unavailable. Same-scope conflicts
reserve the chord, execute nothing and publish `BindingConflict`.

The workspace component `ContextCommandRequests` and the distinct application
resource `GlobalCommandRequests` accept typed `invoke::<Save>()`, `invoke_id(id)` or
explicit requests with `InvocationContext::{Focused, Global, Panel(entity)}`.
Dispatch admits one batch before execution, captures workspace/panel/handler targets,
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
border matching the status bar and horizontal padding from `WorkbenchSurface.panel_gap`
(the default gap when used without a workbench surface).
Attach a `bedtime_ui2::UiView` to the `MenuBar` entity to draw application controls
after its abstract menus, including icon-only buttons. Its renderer receives the
menu root as `UiContext::view`, the surface's normal UI services and the main World.
These controls are part of the egui bar and are suppressed with it during native
menu presentation.
`draw_popup_menu(&mut frame, world, &response, root, context)` associates a menu
with an egui response. Adding `NativeMenuBar` selects native presentation; removing it returns to egui
when the application calls `draw_menu_bar`. The egui bar is suppressed while
that root is successfully natively installed (`native_menu_installed`), rather
than merely supported by the platform. Global application menu roots also work
without a selected workspace. Native menu clicks and supported physical keyboard
accelerators enqueue the same revalidated command requests; applications own their
actions and modal/shutdown gates. Unsupported native key mappings retain a textual
hint and the ordinary keyboard route.

## Shortcuts and direct panel input

The UI adapter reads only the explicit WorkbenchCamera's egui context, after
drawing and applying panel focus observations. It uses `consume_shortcut` for
registered bindings, retaining exact modifier matching, event order and repeat
policy. Widgets can consume a shortcut first. Text editing reserves typing,
navigation and clipboard keys, including no-op edits and Enter ending focus;
unrelated chords such as Save remain available. Popups/modals and an unfocused
workbench window suppress workbench keyboard shortcuts.
Workbench owns Quit shortcut dispatch and disables egui's built-in Quit shortcut
on its camera, preserving the application's command and shutdown policy.

The PreUpdate order is UI/shell drawing, focus observations, shortcut collection
(`ContextCommandSystems::CollectInput`), binding resolution, ContextCommand
dispatch and queued workbench/workspace edits. The headless core accepts
normalized `BindingInput` directly and has no egui dependency.

Panels can read Bevy input directly in their own systems or use egui input in
their drawing functions. They own gating using window focus, panel visibility,
`Workbench::has_focus`, viewport hover/egui responses and the surface's
`UiCapture` summary. Workbench does not filter Bevy events or provide a second
held-key state. Consuming an egui shortcut leaves Bevy input unchanged.

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
Set `titlebar_show_title: false` when the application supplies its own titlebar
content; keep `titlebar_shown` and `titlebar_show_buttons` enabled to retain native
controls. `WindowTitlebarGeometry` reports AppKit's reserved height and the union
of visible native button bounds before UI drawing, in logical content-view
coordinates with a top-left origin. Measurements are independent of tint and
blur. Workbench converts them to egui points using the current zoom and reserves
only the portion intersecting its bounds. Native controls remain owned by the OS;
application renderers do not measure or coordinate them. Windows custom-frame
integration and Linux client-side decorations are not implemented.

Panel gaps and outer margins default to 5 logical points in multi-area layouts.
Single-area layouts are edge-to-edge by default. `WorkbenchSurface.panel_gap` sets the full spacing between panels
and between panels and the shell's window, menu, titlebar and status edges.
Tabs have transparent fills with a 12% text-color hover
tint, square lower corners and meet their border. Tab bars have no add button;
empty areas have no built-in type picker; creation UI belongs to the application.
Corner handles use a crosshair cursor and take egui input priority over panel contents. A tab's faint
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
immediately. Fade timing starts at first display and uses elapsed egui time:
preceding idle time and repeated layout passes cannot consume the animation.
Pure area relocation snaps its geometry while fading its contents.

`WorkbenchSurface` also configures panel rounding, suggested opt-in content
padding, panel gaps, corner hit size and debug visibility, independently of `Theme`.
UI2 drawing helpers take explicit geometry and do not depend on this component.
Defaults are 7-point rounding, 14×12-point padding, 5-point gaps, 13-point hit targets and debug off:

```rust,ignore
world.entity_mut(window).insert(WorkbenchSurface {
    rounding: 7.0,
    padding: [14.0, 12.0],
    panel_gap: 5.0,
    hit_size: 24.0,
    debug: true,
    ..default()
});
```

Hit size is measured inward from each visible panel corner after gaps are applied,
so changing panel spacing moves the handles with the panels. Zero disables corner
handles, as does `WorkbenchSurface.drag_corners = false`. Targets are capped at
half the visible panel's shortest side to prevent overlapping corners. Debug
paints the rectangles registered with egui; egui's normal interaction radius also
allows nearby presses in the gutter. Hover cursors and drag startup use the same
egui response, with priority over panel content and below popups/modals.

The native corner regression scenario exercises all four corners, including a
four-area junction, at 0-, 5-, 16- and 32-point gaps. It splits and rejoins through
real pointer input, including presses just into the gutter:

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_corners.py
```
