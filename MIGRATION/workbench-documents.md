# Workbench documents: implemented contract and deferred work

This records the document/session API implemented in
`bedtime2/crates/bedtime-workbench2/src/documents.rs` and
`document_commands.rs`, plus its lifecycle integration in `workspaces.rs` and
`layout.rs`. The crate [README](../bedtime2/crates/bedtime-workbench2/README.md)
shows practical usage; this document records the contract, ownership and limits
that should survive future changes.

It distinguishes implemented behavior from requirements that remain
application-owned or deferred. Acceptance coverage lives in
`src/documents/tests.rs`; final test-run evidence for a change belongs to the
implementing task, not to this record.

## Scope

Implementing crates: `bedtime-workbench2` core. The module adds no renderer,
window backend, native-library, filesystem-watcher or storage-provider
dependency, and compiles with default features disabled. It uses Bevy, `std` and
the crate's existing workspace/work queues. Applications still own models,
source I/O, source identity, save policy, conflict policy, persistence and UI.

## Ownership model

- A `Document` is a workspace member entity with an `InWorkspace` relationship.
  It owns the canonical model, revision, undo/redo history, acknowledged source
  checkpoint, save binding and save task privately (type-erased behind
  `Document`). Only `documents.rs` mutates those fields.
- `Document` exposes queryable metadata only: `workspace()`, `title()`,
  `source_key()` and `model_type_name()`.
- `DocumentHandle<M>` is a `Copy`, non-owning typed reference. Dropping handles
  or viewers never retires a session.
- `DocumentView` on a panel is a non-owning, typed reference to one document. A
  panel may be in any layout of the same workspace; other workspaces are
  rejected. Several viewers, including viewers in different layouts, share one
  session, history and source checkpoint.
- `DocumentDraft` is an application-declared marker on a bound viewer meaning
  "uncommitted input exists here". It is not inferred from model state.
- `PanelInstance.dirty` is derived, not authoritative: reconciliation sets it to
  the document's dirty state or the panel's draft state.
- Reconciliation validates every document/view before writing any dirty flag.
  Duplicate source keys within one workspace, a document outside its workspace,
  a view without `PanelInstance`, a view in the wrong workspace, a model-type
  mismatch or a `DocumentDraft` without `DocumentView` fail the pass.

## Model contract

```rust
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
}
```

- `apply` must return an inverse change that restores the previous model. Undo
  and redo replay inverses; there is no separate snapshot per history entry for
  ordinary edits.
- Changes are applied to an isolated clone of the canonical model. A returned
  error changes no content, revision or history, even if `apply` mutated its own
  clone before failing.
- Implementations must not perform I/O. Save callbacks, not models, do I/O.
- `PartialEq` defines "clean": the document is clean exactly when its content
  equals the acknowledged source baseline. Equality is not a byte comparison,
  not a revision check, and not a substitute for a source token.
- `String` implements `DocumentModel` with `TextChange`, a replacement over a
  UTF-8 byte range. Both range ends must be character boundaries and within
  `0..len`; invalid ranges are rejected without changing content.

## Opening, revisions and history

- `open_document(world, workspace, model, options)` validates the workspace and
  creates the session plus its `InWorkspace` link.
- `DocumentOptions::loaded(title)` marks the supplied model as the acknowledged
  source checkpoint, with no source token and a history limit of 100.
  `DocumentOptions::unsaved(title)` supplies no checkpoint, so even an untouched
  model is dirty and cannot be closed without an explicit discard.
  `DocumentOptions::default()` is `unsaved("Untitled")`.
- `source_key` is application identity. It is deduplicated only within one
  workspace: reopening an existing key returns the existing session (cast to the
  requested model type) and does not replace its contents or checkpoint. The
  same key in another workspace is a different session. Reopening with a
  different model type fails with `WrongModel`.
- `source_key` has no setter. Replacing a save binding is not a rename or
  "Save As"; the application owns source identity and must open a new session
  when identity changes.
- `revision` starts at 0 and advances by one for each accepted content change:
  edits, clean external replacement, conflict resolution that replaces content,
  undo and redo. No-op edit transactions do not advance it. Saving and source
  acknowledgement do not advance it.
- Revision is monotonic and is not a saved-state identifier. A saved document
  can have a higher revision than its baseline; comparing `revision` with a
  remembered save revision is wrong. Use `dirty` and `source_version`.
- `inspect_document` reports revision, `source_generation`, acknowledged
  `source_version`, dirty/saving/conflict/refresh state, binding and draft
  presence, undo/redo labels and the last save error.
- `edit_document` applies one change. `edit_document_transaction` applies a
  sequence to successive states of one isolated clone and commits one
  revision/history entry. A transaction is atomic: a failed change leaves the
  canonical content, revision and history unchanged.
- `history_limit` bounds retained undo transactions (0 disables history, not
  editing). `undo_document`/`redo_document` return the new revision and reject
  drafts.
- `begin_document_undo_group(world, id, viewer)` starts an explicit group;
  grouped edits merge only while consecutive and unbroken. Save start, external
  observation, undo, redo, rebinding and direct non-grouped edits close the
  group. `end_document_undo_group` reports `StaleUndoGroup` after any break. A
  group cannot be reopened.
- The acknowledged source checkpoint (`baseline`) is deliberately outside undo
  history. Undoing back to earlier text does not restore earlier source
  knowledge; it makes the document dirty against the newer checkpoint.

## Drafts

A viewer inserts `DocumentDraft` while it holds uncommitted input (for example
an in-progress text composition or dialog). Remove it only after explicitly
committing that input into the document or abandoning it.

While any viewer has a draft:

- `save_document`, `undo_document`, `redo_document`, `close_document`,
  `discard_document` and `check_workspace_documents` reject with
  `DocumentError::Drafts`; workspace preflight and panel/layout closure reject
  the operation with their own draft error.
- `DocumentInfo.has_drafts` is true, `can_undo`/`can_redo` are false, and the
  panel is dirty.
- Rebinding a drafted panel away from its document is rejected.

Workbench cannot infer the contents of uncommitted widget input, so a text
viewer must commit a draft (or abandon it) before document undo/redo, save or
retirement. Undo/redo reject drafts rather than mixing widget-private state with
document history.

## Save protocol

```rust
pub fn bind_document_save<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    binding: DocumentSaveBinding<M>,
) -> Result<(), DocumentError>;

pub fn save_document(world: &mut World, id: DocumentId) -> Result<WorkId, DocumentError>;
pub fn poll_document_saves(world: &mut World) -> Vec<DocumentSaveFinished>;
```

- One typed binding per document. Binding is rejected while a save is running;
  unbinding is also rejected while running.
- `DocumentSaveBinding::new(destination_key, callback)` takes
  `Fn(DocumentSaveSnapshot<M>) -> Result<Option<SourceVersion>, DocumentSaveError>
  + Send + Sync + 'static`.
- `DocumentSaveSnapshot` contains `document`, `model` (immutable-at-time copy),
  `revision`, `source_generation` and `expected_source_version` — the token of
  the acknowledged checkpoint at save start, not the latest editor revision.
  The callback may mutate its own copy but cannot mutate the session.
- Destination keys serialize saves across all documents and workspaces in one
  World. Equal keys are busy (`DestinationBusy`); absent keys serialize only
  within that document. Applications must supply globally meaningful keys such
  as canonical paths when two destinations can alias.
- `save_document` spawns one workspace task owned by the document, records the
  in-flight capture and returns `WorkId`. `DocumentInfo.saving` is then true and
  a second save, rebind, close or discard is rejected.
- The callback must enforce its backend's compare-and-swap/conditional-write
  contract if lost updates matter. Supplying `expected_source_version` is only
  useful when the backend checks it; an absent token or unconditional writer
  provides no overwrite protection.
- Completion is acknowledged only for the captured model. Edits made during the
  save remain dirty; undoing back to the captured contents can make the document
  clean again.
- `poll_document_saves` is non-blocking and returns each completion once. The
  installed polling system runs before `WorkspaceSystems::Prepare` and also
  emits `DocumentSaveFinished` messages.
- Success acknowledges the captured content and stores the returned token
  (possibly `None`), then advances `source_generation`. It does not change
  content revision.
- All errors preserve content/history and leave the binding installed. A normal
  backend `Failed` result allows retry. `Panicked` and `Cancelled` leave the source
  outcome uncertain and require refresh before retry or clean retirement.
- `DocumentSaveError::Conflict` (backend CAS failure) also sets
  `requires_source_refresh`. Uncertain receipts advance `source_generation`, so
  reads begun before that receipt become stale. A fresh source observation is
  required; a retained conflict cannot bypass the gate by being resolved.
- `DocumentSaveError::SourceChanged` means the write may have succeeded but
  source or binding knowledge advanced while it was in flight. Its completion
  cannot acknowledge an obsolete snapshot; a fresh observation is required.
- A late completion cannot overwrite a newer conflict, acknowledged checkpoint
  or binding. Saves never silently adopt the latest editor revision.
- A watcher may observe the captured write before its receipt. Matching captured
  contents are retained as pending incoming knowledge without replacing local
  edits or undo state. A successful receipt with the same source token acknowledges
  that echo without adding history; a different newer observation is preserved.

## External source observation

```rust
pub fn offer_document_external_update<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    expected_source_generation: u64,
    incoming: M,
    version: Option<SourceVersion>,
) -> Result<DocumentExternalUpdate, DocumentError>;
```

- `SourceVersion` is an opaque application token (string wrapper), not a
  filesystem timestamp, hash or comparable ordering. Workbench stores and
  returns it without interpretation.
- The component that opens/observes a source supplies observations in source
  order. Workbench does not order, coalesce, debounce or retry events from
  different reads; it only compares the supplied generation and suppresses
  events that carry no new source knowledge.
- `expected_source_generation` must equal `DocumentInfo.source_generation`.
  Mismatch returns `StaleSourceGeneration` and changes nothing: this is how an
  async read started before a save, refresh or conflict is prevented from
  overwriting newer knowledge.
- Results:
  - `Applied`: the document was clean and different; content is replaced with an
    undoable "External update" entry, the new baseline/token is acknowledged.
  - `Acknowledged`: incoming content equals local content; checkpoint/token are
    acknowledged without content revision or history.
  - `Duplicate`: no content or history change (same pending incoming, a
    self-write echo, or incoming equal to the acknowledged base). A differing
    token is still recorded and advances `source_generation`.
  - `Conflict`: incoming is retained without replacing local content. This covers
    divergent local edits and a possible self-write echo awaiting its save receipt.
- A retained conflict is not implicitly resolved by later observations. Newer
  observations may replace pending incoming knowledge or converge if local
  content matches, but content replacement requires explicit resolution.
- `document_conflict` reports `base` (last acknowledged model), `local`,
  `incoming`, `incoming_version`, revision and source generation, or `None`.
- `resolve_document_conflict` requires the current revision and source
  generation and rejects `requires_source_refresh` even if a comparison remains
  available. Refresh first: resolving an old comparison is not new source evidence.
  `AcceptIncoming` replaces content, `Merged(model)` installs an
  application-supplied result, and `KeepLocal` keeps content without a content
  revision change. All three acknowledge the incoming token as the new
  checkpoint and close any undo group. There is no merge algorithm; an
  application computes `Merged` if it wants one.
- Resolving a conflict does not write to the source. The document remains dirty
  unless the chosen content equals the incoming baseline.

## Lifecycle

- `check_document_close(world, id, discard)` rejects saving and drafts always,
  and rejects dirty/conflict when `discard == false`.
- `close_document(world, id)` requires a clean, conflict-free, non-saving,
  non-drafted document. `discard_document(world, id)` explicitly abandons dirty
  content and conflicts but still rejects in-flight writes and drafts.
- Both close every bound viewer through the workbench panel API, then despawn
  the session. Every viewer is validated before the first is retired, so a
  rejection leaves all viewers and content intact. Neither runs during UI
  drawing. Both reconcile the workbench graph first.
- Closing the last viewer does not close the document; the session (and any save
  binding) remains until `close_document`/`discard_document`, workspace
  retirement, or direct despawn.
- Workspace close preflight is `preflight_workspace_close(world, workspace,
  WorkspaceCloseMode)`. `Saved` requires every document to be clean and
  conflict-free; `Discard` allows dirty content and conflicts. Both always reject
  in-flight saves and drafts. Applications may call this before presenting their
  own save/discard/cancel UI, then again after asynchronous saves.
- `Workspaces::close` runs the `Saved` preflight and `discard_and_close` runs the
  `Discard` preflight. Either returns an error without switching selection or
  retiring anything when preflight fails; a failed close does not partially
  close the workspace.
- `Workspaces::shutdown_work` returns `Result<(), String>`. It first preflights
  every workspace with `Discard` (refusing in-flight document saves and drafts)
  and only then requests cancellation of managed work. A refused shutdown
  performs no cancellation. Shutdown is final for those work scopes;
  `WorkspaceWork::is_closing()` then prevents new document creation, edits or
  source observations while the application drains work before exit.
- Closing a panel or deleting a layout also rejects `DocumentDraft` viewers;
  the ordinary save/discard decision for the document is still application-owned.
- Direct `World::despawn` of a document, viewer, panel, layout or workspace
  bypasses these checks. Members still retire through relationship-linked
  despawning, but no save/draft preflight runs. Use the controlled APIs when
  preflight matters; raw despawn remains useful for application-owned teardown
  where the application has already decided.

## Commands and shortcuts

- `SaveDocument`, `UndoDocument` and `RedoDocument` are public `ContextCommand`
  types with keys `workbench.document.save`, `workbench.document.undo` and
  `workbench.document.redo`. The core plugin installs one global handler for
  each.
- Handlers resolve the admitted invocation's focused panel, require a
  `DocumentView` owned by that invocation's workspace, and call the public
  document API. A missing viewer or a viewer that moved/closed before execution
  returns an error without rerouting.
- `register_document_shortcuts(world)` is opt-in. It adds conventional global
  Save/Undo/Redo chords only when an equivalent global binding is absent:
  Primary+S, Primary+Z, Primary+Shift+Z and Primary+Y. It does not add menu
  items, panel bindings or a text-widget bridge.
- The general shortcut collector deliberately leaves text-editing chords to
  widgets. A text viewer must intercept undo/redo before its `egui::TextEdit`
  handles them and route them through `undo_document`/`redo_document`; otherwise
  the widget's private undo history diverges from the document.
- Command handlers obey each public operation's gates. Undo/redo can run during
  saving or a conflict, but not with viewer drafts; saves reject conflicts, and
  retirement rejects in-flight writes. Commands do not bypass these rules.

## Persistence, restore and application-owned descriptors

- `WorkspaceSnapshot`, `LayoutSnapshot` and `PanelSnapshot` contain layout
  metadata only: names, integer rectangles, tab order, panel key/title/preview,
  selection and focus. They contain no document contents, source key, source
  version, revision, dirty flag or save binding.
- An application that wants to restore documents persists its own descriptor
  per document/viewer — for example title, source key, model identity/version,
  chosen tab/panel identity and any source token. After `restore_workspace`,
  reopen the model with `open_document` and `bind_document_view` the matching
  restored panel. `RestoredWorkspace`/`RestoredLayout`/`RestoredArea` return
  fresh entities in snapshot order for this purpose.
- `source_key` deduplicates reopened sessions within the restored workspace.
  Loading the actual model remains application I/O.
- Rebinding a save binding or changing the destination key is not a complete
  Save As workflow: source identity, chooser and any `source_key` change remain
  application-owned. A real rename requires application policy and usually a
  new session.
- No built-in filesystem watcher, storage chooser/provider, merge algorithm,
  history persistence or automatic crash recovery exists. Applications decide
  when to observe, how to persist, whether to offer merge and how to recover.

## Invariants

- Canonical content, revision and history change only through `documents.rs`;
  models and save callbacks receive isolated copies.
- A failed edit transaction or stale revision changes nothing.
- Revision advances only on accepted content change and is never used as a
  saved-state marker.
- Clean means `content == acknowledged baseline`; saving acknowledges only the
  captured model.
- Source generation gates stale async reads and resolutions atomically.
- One in-flight save per document; equal destination keys are serialized across
  the World.
- A retained conflict requires explicit resolution and is never silently merged.
- Drafts block save, undo/redo, close, discard and workspace/layout retirement.
- Controlled close/quit APIs run preflight; raw despawn does not.
- Closing a viewer never retires its document session.
- No renderer/native feature is required for documents.

## Deferred and application-owned requirements

- File watching, polling and event ordering/coalescing.
- Storage providers, chooser UI, canonical path policy and backend CAS
  implementation.
- Save As/rename/source-identity migration.
- Three-way merge and conflict UI; `DocumentConflictResolution::Merged` only
  accepts an application result.
- Durable undo history, crash recovery, autosave and debounce.
- A text-edit bridge that converts `egui::TextEdit` output into `TextChange`
  transactions and routes its undo/redo. No panel type or document viewer is
  shipped by this module.
- Multi-window or cross-workspace presentation of one canonical session;
  `DocumentView` currently requires the document's workspace.

## Verification — 2026-10-18

The implementation was checked with:

```sh
cargo test --offline --locked -p bedtime-workbench2 -p bedtime-ui2 -p bed2 -p bedtime-automation2
cargo test --offline --locked -p bedtime-workbench2 --no-default-features
cargo check --offline --locked -p bedtime-workbench2 --no-default-features --features ui
cargo check --offline --locked -p bedtime-workbench2 --no-default-features --features native
cargo build --offline --locked -p bed2 --features automation
```

The first suite passed 166 Workbench, 23 UI2, 10 application and 11 automation
unit tests, plus the public document API doctest. The headless Workbench suite
passed 131 unit tests and that doctest. Both optional-feature checks and the
automation-enabled application build passed. Coverage includes source-generation
refresh barriers, cancelled/late writes, watcher echoes before save receipts,
shared typed/text history, and atomic draft/close preflight across layouts.

The new `bedtime2/tools/smoke_documents.py` drives the real application rather
than test-only mutation endpoints. Native exploration verified creation, duplicate
bindings and a live text edit. The complete scenario has **not** passed: its first
run exposed an automation multiline-input issue (the script now emits Enter
separately), and subsequent launches stalled at macOS window creation before the
protocol greeting. End-to-end native UI verification remains outstanding; no
passing smoke summary is claimed. Quit policy is covered by application unit
tests, not the automation transport's force-exit cleanup.
