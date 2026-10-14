# bedtime-api

Shared contracts for the desktop foundation and optional Bevy feature plugins.
This is a linked Rust API; each plugin is an ordinary Bevy `Plugin`.
The owned launcher re-exports this crate as `bedtime::api`.

| Module | Contracts |
| --- | --- |
| `workbench` | Panels, commands, focus, layout requests and `WorkbenchRegistry` |
| `documents` | Document identities, `DocumentFrame`, revision-bound snapshots, edits, replies, bindings and viewers |
| `features` | `FeatureRegistry`, captured `HostRequest`, synchronous `HostEffects`, operation barriers and callback-owned `FrontendSnapshot` |
| `files` | Live `FileContext`, provider-qualified `FileLocation` and shared `FileServices` |
| `storage` | Scoped plugin snapshots, contributions and application-supplied persistence |
| `presentation` | Texture bindings in `PresentationFrame` |
| `settings` | Registered owner/key definitions, effective snapshots, transactional requests/receipts and optional frontend sections |
| `launch` | Application identity/paths, process selectors and entrypoint registrations |
| `status` | Status items, measurements, drawing and commands |
| `gpu` | Shared renderer and texture contracts |

`WorkbenchRegistry` owns the canonical handlers and presentation metadata.
`FeatureRegistry` adds viewer, context and settings contributions referencing
those definitions. They serve distinct responsibilities and keep their existing
registration order.

The default build includes workbench, file context, storage, settings and status contracts without
adding document, language-server or GPU services. Enable `documents` for the
document-dependent contracts in `documents` and `features`. Feature registration and settings contracts do not require documents. `gpu` adds GPU
contracts independently. `language-services` enables `documents`, LSP contracts
and `bedtime-documents/ide-services`; editor panels and widgets have their own
`language-services` opt-in.

`HostAction` is an action payload. `HostRequest` captures the payload together
with session, source, target area and submission order when it is emitted as a
Bevy message. `HostEffects` collects existing frontend and command effects for
host admission. The host's controlled request processor owns consequential
transitions. An in-memory `SessionId` identifies a document session; it is
independent of a persisted bEd workspace.

Document, settings, file context and rendering observations have separate owners.
A plugin system reads the domain resources it needs. Frontends can call
`FrontendSnapshot::capture(world)` at the callback boundary before mutably
borrowing panel state, then borrow its `HostContext` for widgets. A missing
optional domain contributes its default observations.

Saved-workspace definitions and actions belong to `bedtime-plugin-workspaces`.
Ordinary panels use file context or documents and do not need workspace metadata.

Public IDs identify their owning semantic domain. Document commands use
`bedtime.documents.*`, split and workbench commands use `bedtime.workbench.*`, and code editor
features use `bedtime.editor.*`. Stored formats are versioned; old bEd settings,
keybindings, workspaces and history are not imported.
