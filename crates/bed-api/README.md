# bed-api

Shared contracts for the desktop foundation and optional Bevy feature plugins.
This is a linked Rust API; each plugin is an ordinary Bevy `Plugin`.
`bed-core` re-exports this crate as `bed_core::api`.

| Module | Contracts |
| --- | --- |
| `workbench` | Panels, commands, focus, layout requests and `WorkbenchRegistry` |
| `documents` | Document identities, snapshots, edits, replies, bindings and viewers |
| `features` | Host context, `HostFrame`, `HostAction`, captured `HostRequest`, `HostEffects`, `FeatureRegistry` and persistence contributions |
| `application` | Workspace specifications and bEd workspace actions |
| `settings` | Contributed settings sections |
| `status` | Status items, measurements, drawing and commands |
| `gpu` | Shared renderer and texture contracts |

`WorkbenchRegistry` owns the canonical handlers and presentation metadata.
`FeatureRegistry` adds viewer, context and settings contributions referencing
those definitions. They serve distinct responsibilities and keep their existing
registration order.

The default build includes workbench, settings and status contracts without
adding document, language-server or GPU services. Enable `documents` for the
document-dependent contracts in `documents` and `features`. `gpu` adds GPU
contracts independently. `language-services` enables `documents`, LSP contracts
and `bed-documents/ide-services`; editor panels and widgets have their own
`language-services` opt-in.

`HostAction` is an action payload. `HostRequest` captures the payload together
with session, source, target area and submission order when it is emitted as a
Bevy message. `HostEffects` collects existing frontend and command effects for
host admission. The host's controlled request processor owns consequential
transitions. An in-memory `SessionId` identifies a document session; it is
independent of a persisted bEd workspace.

Public Rust names and module paths do not determine stored identities. Command,
panel and viewer IDs, icon namespaces, settings keys and persistence formats
remain stable when packages and APIs are renamed.
