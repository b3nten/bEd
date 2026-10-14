# bedtime-core

Service implementation beneath the owned [`bedtime`](../bedtime/README.md)
launcher: workbench composition, document/model editing, ordered requests,
settings and configuration projection. Applications use the launcher to install
the desktop foundation; `DocumentHostPlugin` supports ordinary headless Bevy
tests of shared editing services.

Feature plugins register panels and commands through `api::workbench::register`.
The workbench owns handlers and presentation metadata; core resolves document
requests and close/save barriers. Applications contribute menu layout, status
identity, settings pages and overlays while core retains one ordered request
owner and the shared desktop frontend. Explicit custom `DesktopUi` and
`WorkbenchViewport` inputs support alternate presentation without disabling
document lifecycle, command routing or persistence services.

The launcher also installs `DesktopWorkbenchPlugin`: native file drags and
clipboard access, pinch gestures, and panel geometry reach the same shared
workbench contracts with either the standard UI or an application's custom UI.
Native edit and drop actions enter the shared request resolver with their original
source, session, layout and ordering. Applications contribute their menu arrangement;
registered panel handlers own clipboard interpretation, including terminal file
and image pastes.

## Documents and editing

Core installs a `FileContext` for the selected directory, local `FileServices`,
and an empty `DocumentSession` for canonical text and byte documents,
revision-bound edits, multiple views and undo/redo. It polls document services,
publishes document snapshots and resolves requests after drawing. Add
`bedtime_plugin_editor::EditorPlugin` for the existing text and hex widgets. Its default
build does not install language servers or highlighting. Core uses the shared
file-provider backend for local atomic saves; bEd explicitly enables
`language-services`. Core's `remote` feature provides SSH document access without
language-server or highlighting services.

With language services enabled, the code editor plugin selects the application's
`config_dir/lsp.json` and `<workspace_directory_name>/lsp.json` project overrides.
Explicit `SessionOptions` configuration takes precedence. A low-level layered
session with `lsp_project_config: None` reads no implicit project file. Remote
project overrides use the existing authenticated file provider; the user layer
stays on the client.

Core is the writer of `DocumentFrame`. By default its active document follows
the focused panel. An application can supply `DocumentFrameSelection` before
`CoreSystems::Frame` to remember a document while a tool has focus; an explicit
empty selection clears it. Manual publication uses the same `publish_documents`
operation and selection input.

Applications prepare a session with
`DocumentSession::with_file_services(options, &context, &services)`. SSH providers
retain the authenticated transport and deployed agent path. File locations keep
provider identity, so a remote path cannot fall back to a coincident local file.
The selected directory is independent of process cwd and saved-workspace policy.

`open_file` selects a registered viewer and binds its document or local file
before running the panel factory. `open_document_viewer` opens a compatible viewer
in its own tab for the same document. Viewer tabs share canonical data, revisions,
saves and undo history, while each keeps its own UI state. Repeating a viewer
request focuses its matching tab in the requested tab group; changing viewer type
keeps the originating tab open. `open_document_panel` explicitly creates another
view of an existing document. Saved file-backed panel records include the path and
content kind so layout restore can recreate their bindings.
Untitled panels reuse their validated document identity within the same live
session. Persistence across restarts requires the application's document
serializer; a missing identity produces an unavailable panel with its saved
record retained rather than replacing the document with an empty buffer.

For asynchronous producers, capture a `HostRequest` at emission and write it
as a Bevy message. Admission retains it while a layout barrier is pending. For
callers needing an operation receipt, use `submit_host_request` and
`take_host_result`. Requests retain their session, source and target area;
stale layout actions and effects from failed panels are rejected. Save receipts
identify the exact committed revision. The internal admitted-request queue and
receipt table remain durable rather than depending on message retention.
An operation barrier holds later intentions until earlier writes finish. The
requesting integration then prepares and validates the document transition,
retires its presentations, and commits the new context. Failed preflight leaves
the existing session intact.
The message envelope lives in `bedtime-api`, so feature plugins can emit it
without depending on the desktop composition crate. `HostEffects` is the synchronous effect outbox for registered commands,
panel actions and draft commits.

Custom typed contents implement `DocumentModel`, whose change returns its
inverse and whose encoding supplies bytes for saving. `create_model_document`
registers the contents; panels use `DocumentBinding` and read through `model`.
`edit_model` validates the expected revision and applies the entire change to a
clone before committing contents and history. `undo_document`, `redo_document`,
`save_document`, `save_document_as`, `document_revision` and `document_dirty`
work for both default and typed documents. Saves return a `DocumentSaveId`;
consume `poll_document_save` before reporting success. Its receipt names the exact
persisted revision. Edits made while an older revision is
saving remain dirty. `begin_explicit_document_save` reads the registered editor
formatting preference when language services are enabled; lifecycle saves and
autosave bypass formatting. All saves use the same receipts.

Pathless typed models can save to application-owned storage. Implement
`DocumentWriter::write(key, bytes)` and call
`bind_model_document_writer(world, document, opaque_key, Arc::new(writer))`
after creating the model. The binding is established once and supplies that
pathless document's normal save destination. Core captures an immutable encoded snapshot
and its revision/checkpoint, calls the writer off the UI thread, and acknowledges
that checkpoint only after the writer reports durable success. The writer never
changes document state. Save, dirty-close decisions, quit, and transition barriers
therefore use the same operation and receipt as file-backed documents. A failed
write retains the document; later edits remain dirty. File Save As is unavailable
for these bindings; an application can provide a separate export command.

Applications load their models before restoring panels and keep stable storage
keys in panel state. Runtime `DocumentId`s belong to the live session.
[`bedside-table`](../../../bedside-table/README.md) uses this contract to save SQL
query models into its own metadata database, without installing the code editor
or a workspace plugin. A plugin can also keep its own model without joining
this document lifecycle.
Model mutation and persistence stop when the shared session shuts down.

The default `DocumentClosePolicy` rejects closing the last view of a dirty
document or quitting with unsaved changes. Applications can select an automatic
Save or Discard policy, or opt into `DocumentCloseDecisionPolicy::Ask` for the
shared Save/Discard/Cancel prompt. Decisions carry a prompt token; saves and
retirement remain core-owned. A failed save retains the panel and document.
`LifecyclePreCommit` lets applications persist their own live state after
admission and before a context change or quit; a failed commit prevents retirement.
Core does not choose an application's storage schema or workspace policy.
Close approval never retires a frontend. `closing_panels(world)` exposes the
complete set during an aggregate layout/restore preflight, so other closing
views cannot be mistaken for surviving views. Retirement hooks run only at the
workbench's actual close commit.

## Global settings

Plugins register `SettingDefinition` values under an owner namespace before
launch finalization. Definitions supply keys, labels, defaults and optional
validators. `SettingsSnapshot::get(owner, key)` and `for_owner(owner)` expose
effective values. Frontends send `SettingsRequest::new`, `reset` or `batch` as
Bevy messages; `.tracked()` requests return an exact revision/error receipt via
`SettingsResults`.

The settings owner applies ordered batches atomically: validate, lock the stable
sidecar, reload current disk values, merge known changes, atomically replace the
file, then publish. Unknown or absent plugins' overrides survive. Invalid or
failed batches publish no partial values. Malformed startup/reload data retains
the previous valid snapshot, records the error and is never overwritten.
The launcher selects `~/.config/<app_id>/settings.json`; headless tests can use
`initialize_settings(world, None)` for memory-only preferences. The on-disk
format is `{ "version": 1, "overrides": { "owner": { "key": value } } }`.
Old formats are not imported.

Shared definitions belong to `bedtime.ui` (font, size, opacity and motion),
`bedtime.documents` (autosave and viewer selection), and `bedtime.workbench`
(keybindings and tab controls). Optional plugins register their own namespace.
Applications can override registered defaults through `set_application_defaults`
before `finalize_settings` seals registration. Keybindings are the ordinary
`bedtime.workbench.keybindings` setting. Core projects preferences before rendering;
workbench resolution supplies one immutable keymap. Terminal input receives it
before interception, and native menus receive it during preparation before UI.

[`bedtime-plugin-settings`](../bedtime-plugin-settings/README.md) is an optional generic
frontend. Applications can replace it without replacing the settings service.

## Layout and feature persistence

Core supplies one active workbench layout. Optional
[`LayoutsPlugin`](../bedtime-plugin-layouts/README.md) retains numbered or named
layouts and returns snapshots; applications choose templates and persistence.
[`bedtime-storage`](../bedtime-storage/README.md), exposed as `api::storage`, owns
plugin-state contributions and snapshot collection/restoration. Core initializes
memory-only storage. Applications choose `StateStore` and `StateScope` when
persistence is wanted; collecting snapshots preserves absent plugins and writes
nothing by itself. A saved workspace is optional.

## Icons, themes and assets

Plugins register exact namespaced icon contracts and embedded defaults through
[`bedtime-ui`](../bedtime-ui/README.md). `DesktopOptions::icon_directories` selects per-owner
artwork directories before feature plugins build. Core fills its embedded `bedtime.core.*`
artwork without discovering bEd configuration or resources.

The launcher accepts one `UiTheme`. The desktop prepares a complete
`UiFrameContext` snapshot for shared controls and custom frontends. The desktop
also registers the shared memory asset source before Bevy creates `AssetServer`;
optional model/preview consumers can load namespaced bytes after installation.
Native and GPU realization remain desktop-owned.

## Terminal and debugger entrypoints

Register `bedtime_plugin_terminal::launchers()` and/or `bedtime_plugin_debugger::launchers()`
before dispatch. Their early process handlers run before GUI startup. Install
the normal plugins explicitly in the desktop configuration callback.
Terminal owns command forwarding and acknowledgements; the document,
workspace and mascot command owners supply `+open`, `+workspace` and `+ducky`
behavior through registered workbench commands.

## Native workbench integration

`DesktopWorkbenchPlugin` bridges desktop input to captured host requests for
panel actions, clipboard operations and external file drags. It supplies native
pinch events, the file clipboard adapter and panel geometry for rendering even
when an application replaces the default drawing callback. Feature plugins own
paste interpretation and drop acceptance; applications retain their menu and
fallback opening policy. A direct-file viewer does not reserve its file against
editing, renaming or deletion merely because its tab is open.

`FoundationPlugin` reserves window close and native Quit requests for the shared
document owner, including when an application overrides the desktop window.
Dirty-document decisions and pending saves complete before the application exits.

The default core frontend draws the registered panels, status contributions and
ordered `WorkbenchOverlay` layers. `StatusBarIdentity` supplies optional permanent
application text. Applications arrange their menus with `api::menus::MenuLayout`;
command owners supply current labels, availability and remapped shortcuts for both
native and ImGui menus. Repeated menu references to a command must use the same
argument payload; malformed layouts return an explicit error. Native dispatch and toolbar drops use the same captured
command queue as shortcuts. Custom drawing remains available through
`draw_workbench`, without moving native or canvas lifetime ownership into the app.

Enable the optional `layouts` feature for `layouts::DocumentLayouts`, the document
integration for `bedtime-plugin-layouts`. It shares retained-layout switching,
barrier admission, definition capture and document-preserving rearrangement.
Applications own layout identities, definitions and persistence adapters. A core
without this feature does not depend on the retained-layout crate.
