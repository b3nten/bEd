# Bedtime 2 migration overview

This document records the architecture discussion and the direction for the
second Bedtime foundation. Implementation proceeds alongside the existing
application in `bedtime2/` and `bed2/`. The first implementation is recorded in
[step-1.md](step-1.md).

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
| `bedtime-ui2` | ImGui lifecycle, Bevy rendering, input adaptation, themes, controls, icons and effects |
| `bedtime-workbench2` | Headless application/action/menu model, panel layout, workspaces and planned documents/settings; optional native and UI adapters |

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

The existing `bedtime-desktop` combines native integration with the complete
ImGui/Bevy rendering runtime. In the new design, the ImGui integration belongs
in `bedtime-ui2`, including input translation, frame snapshots, rendering,
composition and recovery of UI-owned GPU resources.

Workbench core owns abstract menus and action types. Native and ImGui adapters
present the same model and submit requests directly to the command dispatcher.
The workbench UI adapter projects theme background colors to native window
appearance; UI2 itself has no native workbench dependency. Native platform input
still enters through Bevy/Winit, while ImGui-specific cursor and IME adaptation
belongs to UI2.

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

## ECS menus and typed commands

The initial desktop scaffold introduced owning ECS menu relationships and
entity-bound handlers. [The stage 3 split](step-3-split.md) supersedes that model:
menus are portable workbench data and reference action types through `Invokes`.
Action definitions retain readable namespaced keys for persistence. Scoped
handlers remain registered Bevy system entities, owned by their panel when local.

There is one request queue. Menus, keybindings and application systems submit
requests naming an action type and context. Workbench resolves panel/global
handlers, captures a batch before execution, checks availability and publishes
results. Handler follow-ups wait for the next dispatch pass. Native callbacks
never execute application handlers directly. Native and in-application menus
use the same resolved availability and shortcut bindings.

## UI foundation

[Step 2](step-2.md) introduces `bedtime-ui2`: ImGui lifecycle, native input
adaptation, rendering through Bevy's existing device, reusable controls, themes
and control animations. Drawing is immediate; shared application state and surface
configuration are ECS data. Private form state can use structs or Bevy `Local<T>`;
control interpolation belongs to the UI runtime. Registered drawing-system entities receive borrowed frame input and ordinary
Bevy queries/resources. Relationships express ordered surface/view ownership,
view-owned parts and independent theme references.

UI provides Everforest Medium light and dark as ordinary theme values. Consumers
own theme selection, identity and persistence; themes need no stable keys or
global registry. Motion policy is independent of the color theme. The shared
style is prepared before drawing, ordinary controls inherit it, and closed context
menus perform no style setup or contents construction. A 10,000-row clipped list
in the gallery exercises this boundary.

V1's tooltip/popover motion supplies the reference: source-anchored scale and
fade, with native layout and hit testing preserved. Built-in controls automatically
own motion in a private cache keyed by view and ImGui identity, without animation
entities or queries. An explicit final frame pass transforms vertices and clip rectangles,
including child windows/custom painting, before snapshot capture.

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
| Desktop scaffold | An ordinary Bevy host plus a native-only desktop plugin. | Implemented; see [step 1](step-1.md). Native GUI validation and further desktop work remain. |
| Desktop stabilization | Validate native behavior and settle the platform/input boundary needed by UI. | Pending. |
| UI foundation | ImGui lifecycle, input integration and rendering in the existing Bevy app, followed by appearance and reusable controls. | Implemented for one active surface, with a widget gallery; see [step 2](step-2.md). Additional UI capabilities remain scoped there. |
| Workbench foundation | Panels/layout plus a coherent document, history, workspace and persistence model. | Typed commands and the in-memory workspace/layout/panel lifecycle are implemented; see [stage 3](step-3.md). Documents, settings and persistence follow. |
| Services and application features | Add filesystem and justified shared services, then migrate bEd features onto them. | Planned. |
| Remote integration | Exercise the same service contracts across a remote boundary and evaluate replication against concrete requirements. | Deferred. |

Before committing to the workbench API, exercise a text file, database row and
generated model through the same document lifecycle: two viewers, editing,
undo/redo, saving, closing and workspace switching. Use these cases to discover
necessary semantics rather than build a general framework in advance.

The migration is successful when important state has clear owners, consequential
transitions are inspectable, and plugins can use the foundation without learning
bEd-specific policy or duplicating lifecycle rules.
