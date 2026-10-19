# Bedtime 2 migration overview

This document records the architecture discussion and the direction for the
second Bedtime foundation. Implementation proceeds alongside the existing
application in `bedtime2/` and `bed2/`. The first implementation is recorded in
[step-1.md](step-1.md).

The current UI/input implementation is [egui and contextual input](step-4-egui.md).
V2 uses upstream `bevy_egui`, explicit panel input ownership and independently
resolved ContextCommand bindings and handlers. UI2 remains usable without the
workbench command model. Stages 1–3 record the earlier implementation history.

## Purpose

Bedtime is a composable desktop application platform built on Bevy. Its central
capabilities are its UI, panels and layout, and shared documents with undo/redo.
bEd is one application built on that platform; other applications should be able
to use the foundation without adopting bEd's filesystem, IDE services or product
policy.

The migration aims to make the program easier to reason about through clear
state ownership, explicit transitions and fewer interactions between subsystems.
Reducing the crate count alone will not achieve that. The new foundation should
give related behavior one coherent model and one owner.

The current code has useful behavior to preserve, including revision-aware
saves, shared document views, close preflight, ordered requests and protection
against stale operations. Its public surface also reflects several overlapping
models: generic workbench and document-aware host requests, text/byte sessions
and typed model documents, and separate settings and workspace persistence.
The migration is an opportunity to reconsider those distinctions.

## Three foundational crates

The current split, revised in [the stage 3 follow-up](step-3-split.md), is:

| Crate | Responsibility |
| --- | --- |
| `bedtime-clipboard2` | Independent text/file clipboard, optional OS access and in-process fallback |
| `bedtime-ui2` | Upstream egui/Bevy integration, input ownership, themes, icons, viewports and motion |
| `bedtime-workbench2` | Headless ContextCommands, bindings/handlers/menus, panel layout, workspaces and typed/text document sessions; settings remain planned; optional native and UI adapters |

UI2 depends on clipboard, with no workbench dependency. Workbench uses clipboard
and optionally UI2. The separate desktop crate has been retired; native window
and menu integration now belongs to workbench plugins. A release game can use
clipboard or UI2 without workbench.

WorkbenchCorePlugin runs headlessly. WorkbenchPlugin is a convenience plugin group
composing core, clipboard, native and UI services according to enabled Cargo
features. Applications can compose the same individual plugins themselves.

### Bevy composition

Applications install and configure Bevy's plugins, then add the Bedtime plugins.
The foundation should attach to the application's existing window, renderer and
event loop. It should not silently install another `DefaultPlugins` group or
create another GPU device.

A convenience launcher or plugin group may eventually assemble this setup, but
it should use the same public composition path as an ordinary application.
Re-entering an application's executable to dispatch private helper modes is not
part of the intended foundation.

### Desktop and UI ownership

Applications own their Bevy windows, cameras, renderer and event loop.
`UiCamera(surface)` associates an application camera with its UI surface;
automatic primary egui-context creation is disabled. UI2 schedules one drawing
pass after upstream input processing. Upstream owns GPU extraction/rendering,
texture integration, platform input, cursor and IME. Bed adds theme preparation,
shared clipboard, presentation motion and explicit input claims.

Workbench core owns abstract menus and contextual command identities. Native
and egui presenters submit the same requests. Native shortcut hints are label
text, keeping keyboard execution in the UI → panel → command route. The optional
workbench UI adapter projects theme backgrounds to native window appearance.

## Workbench's semantic model

Three concepts define the editing environment:

| Concept | Ownership |
| --- | --- |
| Workspace | An application-defined context with an identity, scoped settings and plugin data, and an editing arrangement. |
| Document | Canonical shared content, edit history, a saved checkpoint and a persistence binding. |
| Panel | Presentation state and references to documents or other application state. |

A document can exist without a panel, and several panels can view the same
document. Panels such as terminals need not become documents merely to join
the layout system.

### Workspaces, settings and plugin data

Workspaces are first-class workbench concepts. They do not inherently represent
directories, projects or remote hosts. Applications choose how to identify,
select, load and persist them.

Plugins should be able to register settings and persist namespaced data in a
global or workspace scope without choosing storage paths or knowing the storage
backend. Applications provide that backend. For example:

- bEd can store global state in `~/.config/bed` and local workspace state in project folders.
- Another application can use one permanent workspace.
- An application can persist workspace state in a database or cloud service.

Global state retains its own lifetime when the active workspace changes.
Settings and ordinary plugin data can share persistence infrastructure while
settings retain definitions, defaults, validation and explicit scope resolution.
The exact settings precedence and storage API remain to be designed.

Asynchronous operations must retain the scope in which they started. A delayed
write for workspace A must not be redirected to workspace B because the active
workspace changed. Workbench should enforce this centrally.

### Documents and undo/redo

The v2 implementation now provides workspace-owned typed/text sessions, shared
history, asynchronous save bindings and explicit external-update reconciliation.
See the [implemented document contract](workbench-documents.md) for the current
API, invariants and remaining application-owned/deferred work.

The document system manages document instances and history independently of
where their contents originate. A document's creator supplies its persistence
behavior, including loading and saving where applicable. Creating a document
from an already-loaded or generated value should also be possible.

Examples include a text file, a database row and an untitled application model.
Viewers edit the shared content without needing to know its storage location.
Compatible viewers share the same document and history while retaining their
own selection, scroll and other presentation state.

Persistence operations receive snapshots and return results. Workbench owns
committing those results and acknowledging saved checkpoints. If a save starts
at revision 10 and editing produces revision 11, completion of that save must
leave the newer changes dirty. Failed saves must retain live documents and must
not authorize their retirement.

Storage independence does not make every viewer compatible with every content
model. Viewers must understand the shared model or use an explicit edit adapter.
Maintaining separate serialized editing buffers would reintroduce synchronization
and history problems.

Undo/redo normally operates on document contents. Reversing an already-committed
database transaction or another external side effect would require additional
application semantics.

The existing typed `DocumentModel`/`DocumentWriter` support and Bedside Table are
useful evidence for this direction. They do not settle the new API or require
all content models to use the same internal representation.

## ContextCommands, bindings, handlers and menus

A `ContextCommandId` is a serializable namespaced string. Metadata, bindings,
registered Bevy system handlers and menu items reference it independently.
Bindings belong to a panel or the workbench. Lookup chooses the focused panel's
binding for a chord before falling back to the workbench. Handler lookup then
independently chooses the panel's handler for that command before workbench
fallback. Disabled local handlers block fallback; failures never reroute.

UI/shell input claims run first, then panel input systems, binding lookup,
command dispatch, queued layout edits and remaining application input. Text
helpers reserve editing gestures and IME, including no-op edits and focus-ending
Enter. Panels can consume eligible events or capture keyboard/pointer input.
A game viewport can own Cmd+S before the workbench sees it. Routed held state
follows admitted presses and releases/cancellation, preventing stuck keys and
fresh presses leaking into another owner.

Menus use the same effective binding hints and handler availability as keyboard
commands. Requests capture panel context. Dispatch admits one batch, resolves
handlers, rechecks availability and publishes results; follow-up requests wait
for the next pass. Consequential layout edits retain their existing single owner.

## UI foundation

[Stage 4](step-4-egui.md) replaces stage 2's ImGui implementation with egui
widgets on upstream `bevy_egui`. The old bindings, input adapter and renderer are
removed from v2. Drawing systems receive borrowed `UiFrame` values with an egui
Ui, theme and small helpers for text ownership, icons, viewports and motion.
One pass per Bevy frame prevents layout retries from replaying application
mutations. Surface/view ownership and independent theme references remain ECS
relationships.

Everforest Medium light/dark palettes, PaperMono and semantic metrics remain
ordinary theme data. Applications own theme selection and persistence. The
custom panel shell retains its layout geometry, gaps, borders, tabs and gestures;
ordinary widgets inherit the theme without copying v1 controls individually.

Motion remains independent of theme. The default duration is 160 ms, with
continuous scalar retargeting and reduced motion. Native popups now grow from
their source with bed1's 96% → 100% cubic ease-out and matching fade at that
duration. Only popup backgrounds grow; contents fade at their final size and
pixel positions, keeping glyph sampling fixed. Layout and hit bounds keep
their final coordinates. New panels snap into
place; existing panels animate their resize during tab drops
and area moves. Corner splits show both halves immediately. Tabs follow their
geometry directly. The egui crate is now vendored with shared widget color
transitions; upstream `bevy_egui` remains the renderer/integration. UI2's ComboBox
is a re-export, retaining the dropdown gap and hidden scrollbar in the fork.
See [vendor patch notes](../vendor/egui/BED_PATCHES.md) for provenance and scope.

The demo's application controls live in a dockable Controls panel. View, Layout
and Workspace menus exercise contextual commands, and each Notes panel contributes
a menu only while focused. Shared menu snapshots enforce panel ownership for
native and egui presentation. Primary buttons retain egui interaction states;
tabs use bed1's transparent fills and subtle hover tint, without an add button.

Panel hosts supply the full content area with no padding or automatic scrolling;
each implementation opts in. The demos use bed1-style padding, while the 3D scene
reaches its panel edges beneath padded text. Shell gaps/margins are 7.2 points.
Tab hover backgrounds slide upward without moving text. UI2 also owns a reusable
status bar: plugins register icon/inline/tooltip systems on either side, with
priority-based collapse to icons. Bed2 reserves the bar below the panel layout.

Icons use Bevy's asset lifecycle: SVG `Icon` assets, partial `IconSet` packs and a
`UiIcons` handle component selecting a surface's requested pack. `UiIconStatus`
reports the active pack and loading/errors; failed or pending replacements retain
the current artwork. Built-in semantic slots use a fixed enum, while custom art
can be referenced directly by handle or through an `IconSource` component.
Individual displayed icons need no entities. DPI-aware rasterization and managed
texture lifetime remain private to UI; color-theme changes only alter draw tint.

Scene viewports use standard Bevy image targets and a `UiViewport` component.
Games/model viewers retain their normal camera schedules and post-processing;
UI embeds their completed output in a panel. The compositor separates drawing
the UI layer from final alpha composition and can overlay an earlier window
camera. `bed2-demo-game` validates this with one Bevy-only scene shared by the
editor viewport and a standalone target. Optional editor effects remain separate
from game effects; applying them to the completed editor would also affect embedded
viewport pixels unless the application opts out or adds explicit layer handling.

## Optional service boundaries

An optional filesystem crate, discussed as `bedtime_fs` and provisionally named
`bedtime-fs2` for the migration, will give consumers filesystem operations
independently of the core document system. These include reads, writes, reloads,
watches, search, metadata and samples, and create/rename/delete operations.

Applications can supply local, remote or virtual implementations. Paths must
retain provider identity. Operation completion and cancellation should support
slow providers without requiring consumers to understand networking.

A file-backed document is an integration of these two systems: load through the
filesystem provider, create the document, and bind saving to the provider.
Explorer and other filesystem consumers do not need to route every operation
through a document.

Process execution is another candidate boundary because terminals, language
servers, builds and debuggers have concrete needs for working directories,
environment, streams, cancellation and exit status. Its API and crate have not
been decided. Other shared modules should be introduced only when actual
consumers justify them.

The application executable should stop serving as a terminal-command client or
debugger launcher. Dedicated helpers are the proposed replacement. The existing
debugger trampoline addresses a real LLDB/macOS attachment constraint; moving it
out of the application does not itself eliminate that constraint.

## Remote: alternatives still under discussion

Remote implementation is deferred while the local ownership model is established.
The discussion identified two possible approaches:

1. A headless application/server with selected authoritative state replicated to
   clients using Bevy networking infrastructure.
2. Service implementations that relay filesystem, Git, debugger and process
   operations across SSH, with local implementations using the same consumer
   contracts.

The working recommendation is to begin with service relays for bEd's existing
remote-development requirements. In that model, the client owns interactive
document contents, history and presentation; the server owns filesystem access
and tool/process execution. A headless Bevy server can host those services
without replicating the workbench.

Replication remains an option, particularly for shared sessions or collaboration.
It still requires explicit decisions about document authority, edit ordering,
latency, reconnects and undo ownership. A networking library does not supply
those application semantics automatically. No replication dependency or remote
architecture has been selected.

Separate build targets can keep UI dependencies out of a server. The desktop
client should be able to select local or remote providers at runtime; consumer
plugins should not require separate implementations for each location.

## Migration sequence

Develop each foundation in isolation and review its behavior before building the
next layer. Keep the original application usable throughout the migration.
`bed` remains the default Cargo workspace member; `bed2` is selected explicitly.

| Phase | Intended result | Status |
| --- | --- | --- |
| Desktop scaffold | An ordinary Bevy host plus a native-only desktop plugin. | Initial implementation recorded in [step 1](step-1.md); services now live in workbench and GUI validation is covered by stage 4. |
| Desktop stabilization | Validate native behavior and settle the platform/input boundary needed by UI. | Native window/menu integration and input ordering exercised through stage 4. |
| UI foundation | UI input/rendering, appearance and reusable controls in the existing Bevy app. | Initial ImGui implementation recorded in [step 2](step-2.md); superseded by stage 4. |
| Workbench foundation | Panels/layout plus a coherent document, history, workspace and persistence model. | Typed commands, workspace/layout/panel lifecycle, layout snapshots and [document sessions](workbench-documents.md) are implemented. Settings, document restoration policy and storage integrations remain application-owned or deferred. |
| egui migration | Preserve the Bed shell, themes and motion using egui widgets and upstream Bevy integration. | Implemented and native-smoke tested; see [stage 4](step-4-egui.md). |
| Contextual input and commands | UI/panel input ownership and independently resolved bindings and handlers. | Implemented, including game capture and effective menu hints; see [stage 4](step-4-egui.md). |
| Workspace runtimes | Scope plugin worlds and external work to application-defined boundaries. | Implemented with workspace SubApps, explicit host scheduling, active/explicit APIs and asynchronous task/process cleanup; see the [workspace API](../bedtime2/crates/bedtime-workbench2/README.md). |
| Services and application features | Add filesystem and justified shared services, then migrate bEd features onto them. | Workspace-scoped tasks and pipe processes implemented; filesystem and feature migration follow. |
| Remote integration | Exercise the same service contracts across a remote boundary and evaluate replication against concrete requirements. | Deferred. |

Before committing to the workbench API, exercise a text file, database row and
generated model through the same document lifecycle: two viewers, editing,
undo/redo, saving, closing and workspace switching. Use these cases to discover
necessary semantics rather than build a general framework in advance.

The migration is successful when important state has clear owners, consequential
transitions are inspectable, and plugins can use the foundation without learning
bEd-specific policy or duplicating lifecycle rules.
