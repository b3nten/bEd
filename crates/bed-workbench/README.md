# Desktop workbench

`bed-workbench` supplies content-independent panel entities, queued commands,
layout and persistence. It depends on Bevy, Dear ImGui and Bed's shared UI styles;
it does not depend on Bed's editor, documents, terminal, language servers or
content viewers.

A feature is an ordinary Bevy `Plugin`. In its `build` method, call
`bed_workbench::register(app.world_mut(), "feature", |registrar| { ... })` to
contribute panels and commands. Registration validates every namespaced ID and
menu reference before publishing definitions or allocating registered systems.
The `Registry` resource preserves contribution order. Each owner registers once
for the app's lifetime.

A panel factory receives `PanelCreation` containing its live `Entity` and saved
JSON state. Attach the feature's components to that entity and return a `Panel`
frontend object. Frontend objects stay in main-thread storage. `Panel::title`
and `Panel::save` read the world; `Panel::draw` can update widget state in the
world and returns `Action`s. Command handlers are registered Bevy
systems that receive an owned `CommandInvocation`. Invocations retain the source
entity and area that originated the command. Closing and restoring a panel
creates a new entity, so a stale command cannot act on a replacement that reused
its saved numeric ID.

Keep the factory entity distinct from the layout tab when using the editor host.
The generic workbench uses one entity for both. The editor passes factories a
frontend entity with `PanelOwner(tab)`, while the stable tab has
`PanelPresentation` and `PanelFrontend(active_frontend)`. Store feature state on
the factory entity. Use `api::panel_owner(world, factory_entity)` when reading
tab IDs or layout membership. Command sources can be either the originating
frontend or its tab; follow `PanelFrontend` when present before querying feature
components. Close, focus and split actions accept either identity. The
layout-app counter plugin demonstrates these lookups and needs no editor
dependency.

Install the optional `DocsPlugin` to provide `bed.docs.panel` and the
`bed.docs.show` command. Factories publish `api::PanelDocumentation` on their
frontend entity, with short titled usage sections. The Docs panel reads the
focused frontend and retains the most recent active work panel while Docs is
focused. It uses the live keymap for local and global shortcuts. Publish only
the shared focused actions the frontend actually supports in `shortcut_actions`.
Embedded widgets can publish individual action IDs there too, or a complete
widget scope through `shortcut_scope`. The panel's own scope is
included automatically. Notes can be updated by their owning feature as needed.

With `bed-desktop`, install `DesktopUi(draw)` and call
`bed_workbench::render_ui(world, ui)` from that callback. Configure
`WorkbenchSystems::Requests.in_set(DesktopSystems::AfterUi)` in `PreUpdate`. UI borrows
end before the FIFO request system runs. The dispatcher applies each command's
returned actions before processing the next command. Desktop input routing then
applies capture before Bevy's input systems. Gameplay in `Update` sees the
resulting world.

Plugins can attach `api::StatusItems` to a panel frontend or any ordinary entity.
Each item has a stable namespaced ID, numeric priority, required icon key, and
measurement, drawing and tooltip callbacks. A measurement of `None` withdraws
the item. Producers choose whether their content is focused, open-panel or
ambient; the renderer does not filter publishers by panel visibility.
Measurements describe content width; the renderer adds the leading icon and
separators. Callbacks read existing feature state, and drawing returns queued `Action`s through
`StatusContext::invoke` rather than performing commands during the UI borrow.
`api::StatusText` provides an optional text-and-commands presentation; custom
callbacks may draw interactive widgets directly.

Hosts reserve `status_bar_height(ui)` in `WorkbenchViewport::bottom_inset` and
call `render_status_bar` after `render_ui`, supplying their permanent right-hand
information with a leading icon and an icon resolver. Higher priorities keep
inline space longer; equal priorities sort by item ID and publisher identity.
Lower-priority items collapse to icons, or a shared overflow menu when space runs out.
`prefer_icon` explicitly requests compact presentation. Hover shows the item's
tooltip; clicking an icon opens its full widget. Overflow also renders full
widgets, so their controls remain available. Popup commands retain the producer
and originating area, and a withdrawn item closes its popup. Failed callbacks
are isolated until their descriptor is removed.

`DesktopPlugins` enables Dear ImGui docking before its first frame. Other hosts
must enable docking themselves. The compositor owns dock
windows and temporary pointer gestures; `WorkbenchLayout` remains the sole
owner of area geometry, tab membership, selection and focus. Corner edits,
border resizing and tab drops enqueue layout transitions through the same
request system. Closing the last tab leaves its area available for new content.

Incoming ImGui drag payloads use the same tab insertion, edge split and animated
placement previews as workspace tabs. The compositor reserves split destinations
through the layout request queue, then publishes `api::PanelDrop` before application
overlays draw. The message retains the destination area, optional panel, tab insertion
position, payload type and bytes. Smaller widget targets take precedence.
Features read these messages during the frontend and enqueue their effects through
the usual request queues.

`WorkbenchAppearance.animations` controls panel and popup opening motion as well
as tiling gestures. A context-local ImGui render hook fades completed panel and
child draw lists, including custom painters, while preserving input and layout
coordinates. The hook is removed when its workbench resource or context retires.

`save` and `restore` run between UI frames. Persistence keeps panel IDs, JSON
state and tiling membership separate from live entity identity. An unavailable
panel feature produces a visible placeholder and retains its complete opaque
saved record, allowing a later build with that feature to restore it.

Applications with document validation or asynchronous saves can install
`WorkbenchClosePolicy`. Returning false keeps the panel and layout intact;
the application calls `commit_close` once its close barrier completes. Restore
preflights every old panel before removing any or replacing the layout.

A custom compositor can use `panel_title`, `panel_state` and `render_panel` for
individual surfaces while keeping the same owned frontend store. The install
and take helpers transfer frontend ownership without replacing live entities.
A panicking presenter becomes an error surface: its partial intentions are
discarded, ImGui scopes are recovered, and its last valid saved state remains
available. Taking and reinstalling that surface preserves its quarantine.

`bed-editor-workbench` uses the same registry, frontend store, compositor and
`WorkbenchLayout`. Its document transactions replace the generic request
driver through `WorkbenchRequestMode::External`, retaining submission order
across generic and editor requests. Document validation and asynchronous saves
run before a panel or layout transition commits. Headless editor apps install
the same plugin: its normal service and request systems drive completion
without borrowing the workbench or advancing a separate app.

## Keyboard shortcuts

Plugins attach global defaults to commands with `registrar.global_shortcut(command_id, &["Primary+Shift+E"])`. Every command remains bindable without a default. Declare frontend actions with `registrar.panel_shortcut(action_id, label, panel_type, defaults)`; inside `Panel::draw`, call `shortcuts.take(ui, action_id)` after the focused widget has handled input. The supplied input is scoped to the panel type and shares consumption with embedded widgets and globals. Check focus and action availability before taking a shortcut.

Hosts publish user overrides through `ShortcutRuntime::overrides`. Editor integration reads `keybindings.json` automatically, using current action IDs directly. Overrides replace the action's full default list; an empty list unbinds it. Removing an override restores defaults. Each alternative records its assignment order. Latest user assignments win within a scope; default ties use stable action IDs. Settings warns about conflicting and overlapping combinations without blocking saves. Global shortcuts with modifiers take priority over local bindings. Shared `bed.edit.*` actions follow the focused widget or panel and are registered once by the workbench. Editor widgets share their keymap wherever embedded; Settings never inspects a panel's contents. Panel-opening shortcuts capture the focused tab area when invoked.

`Primary` resolves to Command on macOS and Ctrl elsewhere. Explicit Ctrl and Command stay distinct. Single combinations support exact modifiers and alternative bindings; sequential chords are not supported.

Settings groups global commands and focused actions under **Common** while their
dispatch remains distinct. The registry also provides `bed.navigation.move_*`
and `bed.navigation.select_*` for arrow movement and selection, and
`bed.view.zoom_in`, `bed.view.zoom_out`, and `bed.view.reset_zoom` for content zoom.
Frontends handle the actions they support with their existing scoped input and
publish those IDs in `PanelDocumentation::shortcut_actions`.

Hosts can retain inactive panels by adding `PanelSuspended` to their layout
panel entities. The workbench excludes suspended panels from drawing, focus,
reuse, and active layout validation, while feature systems and panel state remain
alive. Numbered slots and their persistence belong to the host; feature plugins
keep their ordinary panel factories, save methods, and close hooks. A host that
replaces the active layout calls `invalidate_layout_frontend` to clear native
dock placement and unfinished gestures without touching feature lifecycles.
