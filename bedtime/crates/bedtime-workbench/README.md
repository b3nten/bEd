# bedtime-workbench

One editing session per Bevy World, with optional native and egui presentation.
Add WorkbenchPlugin and UI's UiPlugin after the application's normal Bevy setup,
in either order. Applications compose concrete layout and panel entities;
an empty workbench is valid. Workbench never installs
DefaultPlugins or creates a window or GPU device. Adding
`WorkbenchSurface::default()` to a window creates the default shell and wires an
application-supplied camera marked `WorkbenchCamera`.

| Plugin | Responsibility |
| --- | --- |
| WorkbenchCorePlugin | Layouts, panels, world services, commands, documents and orderly shutdown |
| ClipboardPlugin | Shared text/file clipboard, usable independently |
| WorkbenchNativeWindowPlugin | Native appearance and application-owned close requests |
| WorkbenchNativeMenuPlugin | Native presentation of the world menu |
| WorkbenchNativePlugin | Group of native window/menu plugins |
| WorkbenchUiPlugin | Host adapter for workbench egui presentation |
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
close_when_requested: false. Workbench handles `CloseRequested` itself; see
[Closing and quitting](#closing-and-quitting).

## Component composition

Workbench uses the application's World and renderer. The World is the editing
session: there is no Workspace component, mandatory Session entity, activation
scope or scope membership. `ManagedWork`, `ContextCommandRequests` and (with UI)
`WorkbenchUi` are resources. Applications own
project roots, settings and opening/closing another session.

`Layout` supplies its area collection. `Area`/`PanelInstance`
supply presentation components. A concrete panel requires
`PanelInstance::of::<Self>()` to supply metadata and drawing immediately; ordinary
composition requires no panel-type registration:

```rust,ignore
fn setup(mut commands: Commands) {
    let layout = commands.spawn((Layout { name: "Code".into(), ..default() }, ActiveLayout)).id();
    let area = commands.spawn((Area::default(), InLayout(layout))).id();
    commands.spawn((Notes { text: "Initial draft".into() }, InArea(area)));
}
```

At most one `ActiveLayout` exists in the World; zero is valid. `layouts(world)`
returns stable insertion order and `active_layout(world)` returns an Option.
A bare layout creates no area. `Area::default()` covers the full layout; multiple
areas must form a complete nonoverlapping integer `0..LAYOUT_EXTENT` tiling.
Areas may be empty. Attachment selects a panel's tab without changing area focus;
`select_panel`, `focus_area` and `focus_panel` make that distinction explicit.
Hooks maintain order and nearby selection/focus fallbacks immediately when
relationships apply. Reconciliation validates the graph and stale gestures; it is
not deferred panel initialization.

`InLayout`/`LayoutAreas`, `InArea`/`AreaPanels` and `InPanel`/`PanelCommandParts`
are linked Bevy relationships. Retiring a layout retires its areas and panels.
`Workbench::relocate_panel` moves between layouts while retaining document links
and managed work. Closing the last viewer retains the canonical document.
Game entities need no Workbench components. Application-wide session data belongs
in resources; optional panel/document owners give work a shorter lifetime.

No selected layout is normal: shell/titlebar/menu/status remain available and
panel content is skipped. `Workbench::new(world)` works in that state. Operations
that need a destination return an error. Deleting the last layout is allowed.
Keep deferred writers/readers ordered normally; unapplied Commands are not visible.

## Application-owned sessions

The host owns project/session admission, application storage and process services. Document sources own document I/O.
Workbench provides composition, checked structural transitions and optional
[reflected persistence](#optional-reflected-persistence). It has no panel factory
catalog or built-in project/window manager. See the
[world/session decision](../../../MIGRATION/workbench-world-session-plan.md).

## Closing and quitting

Workbench owns document lifetime, autosave and the unsaved-changes prompt, so
applications don't implement them.

- **Lifetime.** `DocumentReference` is a Bevy relationship from any entity to a
  document. On a panel it is the document the panel shows; on other entities (a
  child of a panel showing several documents, a plugin's holder) it just keeps
  the document open. Once a document has been referenced, losing its last
  reference releases it at the end of the frame (replacing a reference within
  one frame keeps it). Unsaved documents are only released once saved or
  discarded; otherwise quit asks about them. Documents never referenced are left
  to whoever opened them.
- **Closing panels** (closing a tab or area, or merging areas, via
  `Workbench::request_panel_close` or user edits) happens immediately unless it
  would release an unsaved document. Closing one of several views never asks.
- **Autosave.** Set the `DocumentAutosave` resource (default `Never`). With
  autosave on, a close that would release unsaved documents saves them and
  closes once the saves succeed.
- **Prompt.** With autosave off, or when a save fails, workbench asks: Save
  (Retry after a failure), Discard or Cancel. A failed save never closes
  anything. `close_prompt` and `resolve_close` drive this headlessly; the `ui`
  feature draws it.
- **Quit.** `CloseRequested` asks the same about every unsaved document, waits
  for saves, shuts down managed work, then sends `AppExit`. Hook `AppExit` to
  flush application state.

Raw despawn of panels bypasses the prompt; it is not a user-facing close.

## Documents

A document is an entity owning one source's content, undo history and what was
last saved. There is **one document per source**: opening the same source again
returns the same document. Models come from
[`bedtime-document`](../bedtime-document/README.md) (`DocumentModel`, with
`TextBuffer` for text); sources implement `DocumentSource<Model>`.

```rust,ignore
let document = open_document(world, MemorySource::new("Notes", TextBuffer::from("hello")))?;
// Loads on a worker; inspect_document(world, document.id())?.status says when it's Ready.
world.spawn((NotesViewer, DocumentReference::new(document.id()), InArea(area)));
```

- **Editing.** `edit_document` / `edit_document_transaction` take the revision
  the caller saw, and apply all of their changes or none. Undo groups merge
  consecutive edits from one holder. `document_changes_since` lists recent
  commits, so views can update positions instead of rebuilding.
- **Dirty** means the content differs from what was last loaded or saved, not
  that edits happened: undoing back to the saved content is clean.
- **One source task at a time.** Each document runs either a read or a save,
  never both, so saving and outside changes can't race. A save writes a
  snapshot and edits made meanwhile stay unsaved. A read never starts during a
  save, and a read still running when a save starts is dropped.
- **Outside changes** are picked up on `refresh_document`, or periodically for
  sources that report `check_interval` and a cheap `version`. A clean document
  takes the change as an undoable edit. A dirty one gets a conflict, which
  blocks saving until `resolve_document_conflict` (keep local, accept incoming,
  or a merged model).
- **Failures.** A failed save keeps everything, records `DocumentInfo::error`
  and schedules a read, so a save refused because the source changed
  (`SourceError::Changed`) becomes a conflict. A source that disappears marks the
  document `missing`; its content is kept and saving is refused. A failed first
  load gives `DocumentStatus::Failed`; `refresh_document` tries again.
- **Drafts.** `DocumentDraft` on a holder (e.g. an IME composition in progress)
  blocks saving, undo, outside changes and closing until removed.
- `close_document` / `discard_document` close a document and the panels showing
  it. Usually you don't call them: documents are released with their last
  reference (see [Closing and quitting](#closing-and-quitting)).

## Error dialogs (`ui` feature)

Request user-facing errors independently of the panel that noticed
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
plugin or window requirement. `len`, `is_empty` and `front` inspect
retained dialogs, including the active one. Requests survive a headless/no-UI
period and outlive their source panel. The workbench root presents
one dialog before drawing shell/panels, even with hidden panels or no selected
layout. It retains the first available workbench window until dismissal,
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
`bedtime_ui::UiSystems::Configure`. Inserting components is sufficient; no setup
helper or binding resource is needed. Systems that need the completed UI link,
theme or view can run after `UiSystems::Configure`, or in `Update`. Query
`With<WorkbenchSurface>` for the window and `With<WorkbenchCamera>` for its camera.
`WorkbenchShell`, `UiSurface` and a transparent `UiBackground` are required
defaults available immediately when the surface component is inserted. An
explicit `UiBackground` overrides that default.

Removing `WorkbenchSurface` immediately retires its generated root renderer and
default theme, restores its camera target and Quit shortcuts, and detaches a UI
camera binding that workbench supplied. UI restores camera clear/output,
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

impl bedtime_workbench::Panel for Notes {
    const LABEL: &'static str = "Notes";
    fn draw(ui: &mut egui::Ui, world: &mut World,
            context: &bedtime_ui::UiContext<'_>, panel: Entity) {
        let mut notes = world.get_mut::<Notes>(panel).unwrap();
        context.frame(ui).text_edit(egui::TextEdit::singleline(&mut notes.text));
    }
}
commands.spawn((Notes { text: "Draft".into() }, InArea(area)));
// Required components preserve explicitly supplied component overrides.
~~~

Panel draw methods are ordinary Rust calls with exclusive world access, not
registered Bevy systems. Panels compose `bedtime_ui::Component` renderers with
arbitrary associated props. The shell, panels, menus, titlebars and status renderers all draw in the
application World during the same egui pass. Scheduled input,
commands and updates remain Bevy systems. `Panel` has no initializer. Bevy
required components supply only missing state, preserving explicit overrides in
the spawn. `PanelInstance::of::<P>()` carries `P`'s label and draw function on
that same entity. Spawning `(MyPanel, InArea(area))` is the one way to create a
panel. Entity identities keep widget state distinct. Moving tabs retains
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
any ongoing tab drag. These settings apply across layout switches.

By default one tiled area has no border, rounding or gaps (even with several
tabs). For a fixed panel presentation, `WorkbenchSurface::single_panel()` also hides
the tab bar and disables user layout editing, creation and closure:

~~~rust,ignore
world.entity_mut(window).insert(WorkbenchSurface::single_panel());
~~~

The same panel identity, focus and input path remains active. Application APIs
can still create/select/rearrange panels. `TabBarVisibility` supports `Always`,
`MultipleTabs` and `Hidden`. `Workbench::user_edit` applies the surface's
permission policy to a user layout action; direct application operations bypass
it. Custom command handlers should use `user_edit` for user layout actions.
Content padding and scrolling remain opt-in. A panel can query
`world.get::<WorkbenchSurface>(context.surface)` for the surface's suggested
padding and rounding, then pass them explicitly to UI's `padded` and `scroll`
helpers. Workbench configuration is not part of `UiFrame` or `UiContext`.

UiFrame carries borrowed theme, icon, texture and input services. Its surface,
view and panel entities belong to the application World. Panels can inspect game
entities and shared UI components directly. `UiViewport` displays a normal Bevy
image; no viewport proxies or scene duplication are required:

```rust,ignore
let image = world.resource_mut::<Assets<Image>>()
    .add(bedtime_ui::viewport_image(UVec2::new(640, 400)));
world.entity_mut(game_camera).insert(RenderTarget::Image(image.clone().into()));
world.entity_mut(panel).insert(bedtime_ui::UiViewport::new(image));
```

UI resizes visible image targets before camera updates. Application camera
routing belongs in `UiSystems::SyncViewports`. Applications decide which cameras
are active and how gameplay pauses or steps. Additional preview scenes can use
render layers with the same renderer. Workbench does not create, tick or own
child Apps/SubApps. A plugin that needs isolated simulation/rendering owns that
integration; UI's independent `EmbeddedRenderPlugin` remains available. See
[UI viewports](../bedtime-ui/README.md#icons-and-viewports).

`WorkbenchShell` on the window configures bounds, minimum area size, panel
visibility and titlebar ownership. It remains stable across
layout switches. `WorkbenchPresentation` reports titlebar/menu/panel/status
allocations, including usable `titlebar_content`. Titlebar rendering belongs to its window. Menu/status selection is world-owned
in `WorkbenchUi`. Select the menu with
`Workbench::set_menu`. The status bar is absent by default and reserves no space
until the application registers its drawing function:

~~~rust,ignore
let status = Workbench::new(world).register_status_bar(draw_status);
world.entity_mut(status).insert(ApplicationStatus::default());

fn draw_status(ui: &mut egui::Ui, world: &mut World, context: &bedtime_ui::UiContext<'_>) {
    let status = world.get::<ApplicationStatus>(context.view).unwrap();
    ui.label(&status.message).on_hover_text("Application status");
}
~~~

The renderer owns all row contents, layout and tooltips; it can compose ordinary
egui widgets and UI components. The shell supplies a padded row at the bottom
of its allocation and draws it once per frame. The row height accounts for themed
text and interaction height, with two-point top/bottom insets and compact
button padding capped at one point vertically. Widgets share the row's full
vertical bounds and center their contents. Panel button padding does not
inflate the strip. `register_status_bar` replaces
the previous renderer and its owned state. `status_bar()` returns its optional
entity; `remove_status_bar()` removes it and restores the full panel height.
The renderer is world-owned; replacement/removal retires its owned state.

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

fn draw_titlebar(ui: &mut egui::Ui, _: &mut World, context: &bedtime_ui::UiContext<'_>) {
    let mut frame = context.frame(ui);
    frame.ui.label("My application");
    if frame.icon_button(bedtime_ui::BuiltinIcon::Plus).clicked() {
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
keeping titlebar widget state stable across layout switches.
The host reserves extra space beside native controls, six points at
the horizontal window edges and four points above the content before calling it.
Popups retain their ordinary overlay bounds. Change `Titlebar.height` or
`Titlebar.draw` through normal component mutation; the height must be finite and
positive. Removing `Titlebar` removes the custom content and requested height,
retaining native chrome and the workbench surface. The titlebar belongs to the
window and continues drawing with no selected layout. Its callback can compose
or select a layout before the shell draws the body.
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
Panel and layout closure reject bound viewers carrying `DocumentDraft`; unsaved
documents are handled as in [Closing and quitting](#closing-and-quitting).
Direct raw despawn bypasses these checks.

Panel content is clipped to its content rectangle and the area's rounded outline,
including viewport images and custom painting. The host enforces this boundary;
plugins do not need to match their own corner radii. Popups and tooltips retain
their separate overlay layers.
Panel content has no implicit padding or scroll area. Plugins opt into UI's
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
are observed on the next draw. This is
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

Gestures are deferred: drawing records each edit with the layout it was made in,
and `WorkbenchSystems::Apply` applies them after drawing, before keyboard routing.
An edit is dropped if that layout is no longer active, if anything it names has
left the layout, or (for a border resize) if that border no longer exists.

## Execution and external work

Relationship hooks keep the layout graph consistent as it changes; debug builds
also validate it once per frame (in `Last`), panicking on a broken graph.
`ManagedWork` polls in Last. There is one ordinary Bevy plugin lifecycle, clock,
fixed-update loop and renderer.

Command handlers can run without a selected layout. Application services are
accessible directly in the same World; there is no scope forwarding.

```rust,ignore
let task = world.resource_mut::<ManagedWork>().spawn_task(Some(panel), "Search", |cancel| {
    if cancel.wait(Duration::from_secs(2)) { return None; }
    Some(compute_result())
})?;
```

`ManagedWork` owns workers independently of result/I/O handles. An optional owner
entity gives work a shorter lifetime: its removal cancels work on the next update.
Hidden layouts do not suspend workers or polling. `spawn_task` is for disposable
calculations: cancellation can suppress its returned value. `ManagedTask` reports
Finished, Cancelled and Failed; cancellation is cooperative.

Quitting through `CloseRequested` performs orderly exit. Headless hosts and tests
can instead call `shutdown_work(world)` and keep updating until
`work_finished(world)`. New document/work admissions are rejected after shutdown.
Dropping the World alone is a fallback, not orderly shutdown. Tasks must return
after cancellation.

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
install_panel_handler::<Save, _>(world, panel, save_panel)?;
world.spawn(Binding::new::<Save>(KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS)));
world.spawn((MenuItem::new("Save"), InMenu(file_menu), Invokes::of::<Save>()));
```

Installing a handler does not register command metadata. Definition registration
and handler installation work without a selected layout. One handler per command
and scope is permitted. `Workbench::register_handler` installs a world handler;
the free `install_global_handler` installs a world handler. As a direct function
alternative to Bevy system handlers, attach `CommandHandler::<Save>::new(call)`
to its panel (or an unscoped entity for world scope). This installs no
command definition and runs no application code during composition. A panel's
`InPanel` ownership retires separately owned handlers, bindings and menu roots
when the panel closes. Menu descendants inherit their root's lifetime.

Bindings form a nearest-scope overlay by chord: panel, then world.
Handler lookup independently follows panel → world by command ID.
A world binding can use a panel handler; a panel binding can use a world handler.
Disabled local handlers block fallback. Handler errors and stale admitted targets
never reroute to another handler/binding. Admission captures handler generation and
scope identity, so replaced or retargeted handlers reject an old request rather
than executing the new capability under stale admission.

Modifiers match exactly; PRIMARY resolves to Cmd on macOS and Ctrl elsewhere.
Chords retain physical Bevy KeyCode identity. Repeats default to disabled. A
selected binding reserves its input even when unavailable. Same-scope conflicts
reserve the chord, execute nothing and publish `BindingConflict`.

The resource `ContextCommandRequests` accepts typed `invoke::<Save>()`,
`invoke_id(id)` or requests with `InvocationContext::{Focused, Global, Panel(entity)}`.
Dispatch admits one batch before execution, captures panel/handler targets and
generations, checks availability and publishes `ContextCommandFinished`.
Follow-ups wait for the next dispatch; requests submitted after PreUpdate dispatch
execute next frame.

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
Attach a `bedtime_ui::UiView` to the `MenuBar` entity to draw application controls
after its abstract menus, including icon-only buttons. Its renderer receives the
menu root as `UiContext::view`, the surface's normal UI services and the main World.
These controls are part of the egui bar and are suppressed with it during native
menu presentation.
`draw_popup_menu(&mut frame, world, &response, root, context)` associates a menu
with an egui response. Adding `NativeMenuBar` selects native presentation; removing it returns to egui
when the application calls `draw_menu_bar`. The egui bar is suppressed while
that root is successfully natively installed (`native_menu_installed`), rather
than merely supported by the platform. Global application menu roots also work
without a selected layout. Native menu clicks and supported physical keyboard
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
dispatch and queued layout edits. The headless core accepts
normalized `BindingInput` directly and has no egui dependency.

Panels can read Bevy input directly in their own systems or use egui input in
their drawing functions. They own gating using window focus, panel visibility,
`Workbench::has_focus`, viewport hover/egui responses and the surface's
`UiCapture` summary. Workbench does not filter Bevy events or provide a second
held-key state. Consuming an egui shortcut leaves Bevy input unchanged.

See [stage 4](../../../MIGRATION/application-composition.md) for migration and validation.

## Window transparency and macOS blur

`WindowBlur(true)` enables a passive AppKit material behind the Winit content
view. Removing/disabling it hides the material. Its lifetime follows the window;
resizing keeps it fitted without reparenting or replacing Winit's view. It does
not receive input. `WindowBackground` continues to select appearance and native
backing color; translucent GPU backgrounds use a clear native backing so alpha
is applied once. Window-wide opacity remains 1 so text stays opaque.

Applications configure Bevy window transparency/composite alpha mode and the
UI camera's scene-overlay routing. On macOS, enable `fullsize_content_view`
alongside `titlebar_transparent` to paint behind the native titlebar, as in the former application.
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
UI drawing helpers take explicit geometry and do not depend on this component.
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
cargo build --offline --locked -p bed --features automation
python3 bedtime_old/tools/smoke_corners.py  # drives bed_old; not maintained
```

## Optional reflected persistence

Enable `persistence`. `WorkbenchCorePlugin` registers the durable Workbench types;
`persistence::register(app)` is also available for hosts that only need the type
registry. Bevy 0.20 `bevy_world_serialization`/RON support is independent of
`ui`/`native`; no renderer, window, AssetServer or scene spawning is required.

Durable component/resource types opt in with reflection metadata:

```rust,ignore
use bedtime_workbench::{Workbench, persistence::{CapturedWorld, ReflectWorkbenchPersist}};
#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(PanelInstance = PanelInstance::of::<Self>(), RuntimeOnly)]
struct Notes { text: String }
app.register_type::<Notes>();

let mut captured = Workbench::capture_world::<ReflectWorkbenchPersist>(world)?;
// Optional edits affect detached data, never the live World.
captured.insert_component(panel, Notes { text: "Exported text".into() });
let snapshot = captured.serialize(world.resource::<AppTypeRegistry>())?;

let captured = CapturedWorld::<ReflectWorkbenchPersist>::deserialize(&snapshot, registry)?;
Workbench::restore_world(destination, captured)?;
// After all startup partitions have loaded, before normal runtime admission:
Workbench::finish_world_restore(destination);
```

`CapturedWorld<M>` owns a public Bevy `DynamicWorld` (`dynamic.entities` and
`dynamic.resources`). Applications can filter/edit it before encoding or restore;
`component/components/resource`, `insert_component` and `replace_resource` provide
typed access. Capture retains source entity identities for convenient edits.
Serialization creates a detached copy with canonical snapshot IDs. It encodes data;
it does not initialize entities or run ECS hooks. Graph traversal is iterative up to
the entity limit. Captures may be incomplete while
the host edits them; encoding, decoding and restore enforce the final invariants.
Filtering structural entities also requires updating the owning ordered collections,
selection/focus and layout-order resource. Dangling references are rejected, never
silently repaired. Capture validates existing Workbench composition first.

M selects reflection **type data**, not an entity marker. Applications can define
independent partitions without changing Workbench:

```rust,ignore
#[derive(Clone)]
struct ReflectGlobalPersist;
impl<T> bevy::reflect::CreateTypeData<T> for ReflectGlobalPersist {
    fn create_type_data(_: ()) -> Self { Self }
}
#[derive(Resource, Reflect, Default)]
#[reflect(Resource, GlobalPersist)]
struct GlobalSettings { large_text: bool }
app.register_type::<GlobalSettings>();
let globals = Workbench::capture_world::<ReflectGlobalPersist>(world)?;
```

The selected marker is enforced for both components and resources on encode/load.
Ordinary panel composition needs no reflection registration or factory catalog.
A durable panel component must require `PanelInstance` (directly or transitively)
to reconstruct its concrete capability. Unsupported panels fail encoding rather
than disappearing. Runtime handles, workers, presentation and document internals
remain unmarked; ignored fields must be safe defaults.

`Layout`, `Area`, durable `PanelInstance` fields, `ActiveLayout`, `LayoutAreas`,
`AreaPanels`, `InPanel` and the existing layout-order resource are directly marked.
There are no private structural DTO components or second live ordering model.
The relationship target lists preserve meaningful order as well as membership;
`InLayout`/`InArea` sources are rebuilt from those lists, not saved as duplicate
ownership authority. Missing/empty reverse lists have equivalent encodings.
Drawing runtime fields are excluded from `PanelInstance`; restore retains
the concrete panel's required drawing defaults and applies its saved title/preview.

Application-defined Bevy relationships can use the same ordered restoration path:

```rust,ignore
use bedtime_workbench::persistence::register_persisted_relationship;
#[derive(Component, Reflect, Clone)]
#[reflect(Component, WorkbenchPersist)]
#[relationship(relationship_target = GroupMembers)]
struct InGroup(#[entities] Entity);
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, WorkbenchPersist)]
#[relationship_target(relationship = InGroup)]
struct GroupMembers(#[entities] Vec<Entity>);
register_persisted_relationship::<InGroup>(&mut app);
```

Registration supplies typed relationship mechanics for both sides; it neither marks
those types nor captures related entities implicitly. It works with any capture
marker. Mark the source to save its component fields and reconstruct reverse
membership; also mark the target to save its collection order (where ordered) and
other fields. If only the target is marked, source values rebuild through
`Relationship::from` with default extra fields. If both are marked, their membership
must agree. Ordered target persistence requires pair registration; ordinary mapped
references and unregistered source components retain their normal insertion behavior.
Workbench registers `InLayout`, `InArea` and `InPanel` through this same API.

Before live mutation, generic validation checks source/target agreement, duplicate
members, reference closure, collection cardinality, self-link permission and cycles
in linked-spawn ownership across all registered pairs. Non-owning cycles and permitted
self-links are supported. Linked ownership must be acyclic. Missing entities are
rejected rather than synthesized; an empty one-to-one target's `Entity::PLACEHOLDER`
sentinel remains empty. New canonical IDs avoid that sentinel; earlier schema-2 IDs
still remap with declared snapshot identities taking precedence.

Restore inserts ordinary data/resources and empty target collections with saved
payloads before source hooks inspect their targets. It inserts marked sources,
attaches missing unmarked sources in linked ownership order, restores collection
order, drains attachment maintenance, and reapplies saved target fields while keeping
ignored runtime fields initialized by hooks. Trusted hooks must preserve restored
membership. Workbench separately checks geometry/roles/selections and reapplies its
saved focus/selection. No application-specific constructor/factory catalog is needed.

`Snapshot { version, data }` is library schema **2**; reflected Rust type paths/fields
are part of that schema. Previous schema-1 snapshots are explicitly rejected; hosts
must migrate or preserve them. Payload limits: 4 MiB, 4096 entities, 128 resources/
components per entity and bounded reflected traversal. Unknown/unmarked/duplicate
types, invalid references/roles/geometry/order/selections and asset handles are
rejected. Entity fields must participate in Bevy `#[entities]` mapping; resources
with entity fields also need `MapEntities`/reflected mapping registration. Each
partition must be reference-closed. Opaque/custom-Serde types must not conceal entity
references or runtime/asset handles.

Decode validates without inserting user components or constructing required defaults.
Custom Serde/FromReflect and metadata registration are trusted pure transformations.
Restore revalidates against the destination registry before mutation and requires
an empty admitted Workbench for graph data. It allocates/remaps identities, inserts
concrete data/defaults using ordinary reflected insertion, restores registered
relationships generically in linked ownership order, flushes attachment maintenance,
then reapplies target data and Workbench focus/selection. External work starts only through normal
application/plugin activation after the graph is ready. Trusted hooks must not panic
or start external I/O; arbitrary hook rollback/live-session merging is not provided.

### Startup world restoration and runtime layout replacement

World reconstruction establishes a session during startup. The host calls
`Workbench::finish_world_restore(world)` after its loading/admission barrier; later
`restore_world` calls reject both graph and resource restoration, even if all layouts
have subsequently closed. The barrier is explicit because storage may load across
many Bevy updates. Capture, encoding and decoding remain available during runtime.
This API never clears the application's World or infers how to retire session
services from persistence markers. Independent session replacement is unsupported.

A layout has its own bounded capture/replacement API:

```rust,ignore
use bedtime_workbench::persistence::{CapturedLayout, ReflectWorkbenchPersist};
let capture = Workbench::capture_layout::<ReflectWorkbenchPersist>(world, layout)?;
let snapshot = capture.serialize(world.resource::<AppTypeRegistry>())?;
let capture = CapturedLayout::<ReflectWorkbenchPersist>::deserialize(
    &snapshot, world.resource::<AppTypeRegistry>(),
)?;
let replacement = Workbench::replace_layout(world, layout, capture)?;
```

`CapturedLayout<M>` exposes an editable `dynamic: DynamicWorld` and typed component
access/edit helpers. `CapturedWorld::into_layout` selects a layout after application
projection. Both follow only persistently declared linked ownership, including
registered application relationship pairs; shared references never pull in extra
entities. Layout data must contain exactly one layout and its ownership tree, no
resources and no `ActiveLayout`. Encode/decode/replacement share world persistence's
allowlist, limits, pure validation, entity remapping and ordered reconstruction.
References leaving the tree reject the capture. `remove_panel` supports deliberate
application filtering while maintaining tab order and selection; other dangling
references remain errors rather than being repaired implicitly.

Replacement is destructive: the layout/areas/panels receive fresh identities. It
validates incoming data and the live destination, refuses viewer drafts/shutdown/UI
drawing, builds the new tree, then retires the old one. The returned root retains
its destination's layout order and active status. Hosts retain responsibility for
root metadata such as numbered slots and for application close admission. Canonical
documents and session services remain; closing viewers retains their documents and
admitted saves. Panel-owned managed work cancels on normal polling, with the same
cleanup/result rules as ordinary panel closure. Session resources are never replaced.
External activation follows complete reconstruction through ordinary schedules.
Trusted hooks must remain pure; rollback of arbitrary hook side effects is unsupported.

Workbench owns graph correctness; hosts own envelopes, marker/destination policy,
inheritance/unbind declarations, compatibility, bounded reads, atomic writes,
concurrent writers, debounce/retry, late-load policy, external loading and recovery.
bEd and `bed/examples/persist_world.rs` consume this same API.

```sh
cargo test --offline --locked -p bedtime-workbench --features persistence
cargo test --offline --locked -p bedtime-workbench --no-default-features --features persistence
cargo check --offline --locked -p bedtime-workbench --no-default-features
```
