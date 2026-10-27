# Source-backed documents

Decision: 2026-10-26. This record supersedes the source ownership and
application-only I/O coordination statements in `workbench-documents.md`.
The single-World session and checked history/save/close contracts remain.

## Ownership

A provider interns canonical sources. One source has at most one typed document
per Workbench World. Same-type opens reuse the document, including pending/failed
loads; another type fails. An assigned source is never silently retyped after an
external change. Source identity is process-local and independent of its current
location; descriptors persist native locations, not runtime IDs.

The headless `bedtime-document2` core owns representation contracts, persistent
text, byte snapshots and transaction history. Workbench owns ECS documents,
source attachments, scheduled execution and acceptance. Filesystem2 owns native
I/O, canonical filesystem sources, checked publication, write admission and native
invalidation. Viewer crates own decoding/rendering and viewer-local work. The
application chooses representation/viewer associations, tabs, placement, settings
values, project admission and storage destinations.

`TextDocument` and read-only `BinaryDocument` are Workbench's standard models.
Structured viewers usually project shared text. Custom typed sources/models remain
possible without a registry, mandatory byte encoding or an implemented SQLite
backend. Standalone editors use `MemoryTextSession` and the same history engine.

## Execution and invariants

Typed opens reserve source/type before worker loading. Loading, ready and failed
states are explicit; viewers can bind pending handles. Reopening a source does not
replace edits or history. Periodic checks share a rotating 64-task budget. Sources
can opt out, as memory sources do. Native watching is one managed worker per World,
with bounded directory registrations and periodic metadata checks as fallback.

File opening classifies a new regular source as text/binary before reserving the
standard representation; classification and decoding run on workers and the prepared
input is accepted once. Existing source documents always take precedence over a
new viewer choice. Image failures may change presentation, never document type.

Source saves receive the captured model, acknowledged baseline and source token.
The source streams the model and exact baseline through staged replacement. Only
successful captures become acknowledged document state or LSP save notifications.
Later edits remain dirty. Failed/uncertain publication preserves history and requires
fresh source knowledge where effects are uncertain. Filesystem replacement retains
its documented external-writer limitations; it is not a locked content CAS.

Confirmed moves update source location without replacing its identity/history.
Symlink aliases share canonical targets; hard-link names remain distinct sources.
Retargeting an alias resolves a different source on the next open. Confirmed deletion
invalidates the old source; dirty documents retain work and lose their writer.
Native paths stay lossless. External changes pass the normal generation/conflict
gates; malformed text does not turn an assigned TextDocument into BinaryDocument.

Shared byte projections expose current unsaved text or version-checked binary
ranges using u64 offsets. Workbench owns 64 KiB pages and a 32-page cache budget.
Range requests have a 16 MiB allocation bound; larger consumers use an explicitly
bounded stream. Hex reads the canonical document, not a competing disk view.
Text retains the 256 MiB load limit and persistent buffer; paged text editing is
future work. Binary editing/undo/save UI is not introduced.

## Application boundary and compatibility

Filetree may resolve a source and call a host opening callback outside drawing;
its captured root/generation prevents stale delivery. Independent hosts can retain
buffered path-opening intentions. Terminal, restore, Git and Filetree all resolve
through the same provider. Standard application file panels contain document/view
state and no file workers, checkpoints or save-rebinding protocol.

Workbench owns autosave timing/deduplication and tracked Save All. The host supplies
policy and Save/Discard/Cancel choices. Source-change operations retain admitted
write access through reconciliation. Hosts flush their metadata after filesystem
outcomes are reconciled, then preflight documents and cancel/drain managed work.
Cancelling quit abandons the intention and leaves admitted writes running.

Existing application session schemas, viewer IDs and native path records remain.
Restoration reopens canonical sources and recreates viewers; incompatible descriptors
produce per-panel errors. Unsaved-content/history recovery, Save As, New/Untitled UI,
remote sources, SQLite and paged text editing remain separate work.

Verification evidence belongs in CONTEXT.md and the affected crate READMEs; this
record defines the ownership contract rather than a transcript of migration work.
