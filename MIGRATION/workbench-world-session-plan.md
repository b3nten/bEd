# One world, one session; reflected workbench persistence

Decision date: 2026-10-24.
Status: **direction accepted; implementation plan drafted, not implemented**.
The reflection experiment exists; production Workbench still has workspaces and
bEd2 still uses its application-owned session recipes.

## Decisions and rationale

### 1. Remove workspaces from Workbench first

Workbench manages **one editing session in the application's Bevy World**.
It does not manage several retained sessions with one active workspace. The host
owns opening another session, session replacement policy and application shutdown.

Remove workspace IDs/entities, activation, transitive membership, running/inactive
markers, background-workspace policy and cross-workspace relocation. Do not replace
these with a mandatory `Session` entity that reproduces the same ownership graph.
Session-wide stores/queues/defaults should normally be resources. Layouts, areas,
panels and documents remain entities where their identities/relationships matter.
An empty workbench and no selected layout remain valid.

The benefit is removing an entire scope from document ownership, command routing,
worker lifecycle, UI queries, persistence and admission checks, rather than merely
renaming APIs. Multiple layouts and multiple viewers of the same document remain
supported within the single session. Multiple surfaces, if supported, show that
session; independent sessions in the same world are not a requirement.

For initial bEd2 behavior, opening another folder launches a separate application
instance/window. Retain the current instance and its live work. A project folder is
application data, not a reusable Workbench execution/ownership scope. Applications
may still call project-local settings/state "workspace" data.

**Implementation evidence versus premise:** inspection on 2026-10-24 found that
`bed2/src/menus.rs::poll_folder_dialog` calls `opening::open_folder`, and
`opening.rs::poll_folder_opens` reuses/creates and activates a workspace in the
current world. The new-window policy is therefore planned work, not a claim about
that inspected route. Audit other opening routes before changing them.

Tearing down/restarting a native Bevy App in-process is **not** a prerequisite or
an established capability. Native event-loop recreation/platform restrictions and
renderer lifetime need separate investigation if in-place replacement becomes a
requirement. Do not build a host/session-world framework speculatively.

### 2. Add opt-in reflected world persistence afterward

A type explicitly opts in as Workbench-persisted reflection data. Reuse Bevy
0.20's `bevy_world_serialization::{DynamicWorld, DynamicWorldBuilder, WorldFilter}`
and reflection serialization/deserialization instead of inventing a parallel
component codec system. Registration supplies data/type knowledge, not a panel
catalog: ordinary concrete panel spawning/drawing must remain registry-independent.
Exact public API names and feature placement remain implementation decisions.

Capture a **persistent projection** of the world, never all reflected state by
default. Ordinary durable components/resources should need only the marker and
normal reflection registration. Split mixed durable/runtime components where that
clarifies ownership; use source/restore descriptors for runtime-heavy state.
Never persist drawing pointers, processes, managed jobs, caches, presentation
rectangles, derived membership, document save-in-flight state or GPU handles.

Workbench owns reusable structural capture/validation, remapped intra-snapshot
references and controlled graph restoration. The application owns storage format
selection/envelope, destinations, scheduling, I/O, migrations for its data, admission
to destructive replacement, external loading and recovery policy. Library-owned
persistent structural data still needs an explicit version/evolution policy.

**World scope does not mean global storage.** bEd2 can use separate
`GlobalSettings` and `WorkspaceSettings` types, even if both become resources.
Only the permitted setting subset inherits global values; local-only values need
not have global counterparts. Persist explicit values/overrides, not resolved
settings. Configuration inheritance and persistence routing are separate concerns.

After workspace removal there are no workspace ownership partitions to discover.
The application selects a destination for opted-in data: e.g. global preferences
versus project/session state. Prefer separate durable configuration types when
policies differ. Runtime bindings can be derived from distinct persisted default,
global-user and project override declarations; they need not themselves encode
those storage layers as Workbench runtime scopes. Panel-local overrides remain
possible. Preserve the difference between inherit, override and explicit unbind.
Command execution resolves panel → world after workspace scope is removed.

Each independently restorable partition must be reference-closed. Reject entity
references leaving it initially; don't synthesize dangling entities. Entity-valued
persistent fields must participate in Bevy entity mapping. Cross-partition stable
identities need a concrete requirement/design before support is added. Keep
canonical documents separate from viewer state: several views must not restore
several copies of one document.

### Prototype evidence and limits

See [bEd2's experiment](../bed2/README.md#reflected-persistence-prototype),
[`persist_world.rs`](../bed2/examples/persist_world.rs) and its
[persistence layer](../bed2/examples/persist_world/persistence.rs).

On 2026-10-24, five prototype tests, the renderer-free executable round trip and
native macOS save/mutate/load/render-capture smoke passed. These are prototype
results, not verification of the planned migration. The prototype still uses the
old workspace graph and must be adapted after task 1.

The experiment established marked-type extraction, exclusion of reflected but
unmarked data, reference remapping, independently restored workspace payloads,
required runtime component/drawing reconstruction and selection/focus restoration.
It also established that Bevy's writer uses `RelationshipHookMode::Skip`; structural
restore therefore needs explicit control. Temporary reflected bridge components
currently represent unreflected core structure and are removed after restoration.

Prototype limitations: Rust type paths as schema; no migrations; synchronous,
non-atomic file writes; value-only components; no asset-handle loading; no
production document/process admission or rollback. Staging constructs required
components too, so it is not universally side-effect-free. Active workspace choice
is application policy in the experiment. Do not promote these limitations into
production behavior by copying the example wholesale.

## Execution plan

Work in the following order. All checkboxes below are pending. Each phase should
leave the affected targets coherent and buildable; coordinate internal breaking API
changes with their consumers rather than adding a long-lived compatibility facade.
Do not modify v1 workspaces or unrelated uncommitted work.

## Task 1 — remove Workbench workspaces

### 1A. Inventory and settle the replacement data model

- [ ] Map production workspace dependencies across Workbench, bEd2, automation,
  Editor2, LSP2, Terminal2, Filetree2, Model2, Mascots2 and tests/examples. Read the
  current implementation rather than relying on older migration documents.
- [ ] Record the owner for every affected store/queue/link: documents and document
  IDs/indexes; managed jobs/processes; menu/status state; command requests/default
  bindings/handlers; project root; settings; layout slots; persistence admission.
- [ ] Specify one selected layout per world, panel → world command resolution,
  resource-backed session-wide stores and the remaining layout/area/panel/document
  relationships. Keep panel-owned work and document-view links explicit.
- [ ] Identify which stale-target stamps are still necessary. Remove workspace
  selection generations, but retain checks for changed layout/targets, handler
  replacement and stale async document/source/save results.
- [ ] Choose a minimal folder-launch implementation and root-normalization/error
  contract for bEd2. Explicitly decide home/no-folder and same-folder behavior;
  don't accidentally retain a hidden in-world session switcher.

### 1B. Remove core workspace ownership and routing

- [ ] Move session-wide document/work/request stores into world resources and
  update APIs to use the world plus relevant document/layout/panel identities.
  Migrate affected core tests/callers alongside each owner change.
- [ ] Remove `Workspace`, `WorkspaceId`, `InWorkspace`/`WorkspaceMembers`,
  `ActiveWorkspace`, active/running membership propagation, workspace background
  updates, activation/close requests and workspace-only reconciliation paths.
- [ ] Keep `Layout`, `Area`, `PanelInstance`, `InLayout`, `InArea`, tab/area order,
  layout selection and focus/selection fallback semantics. Keep required components
  supplying defaults/drawing immediately; no delayed panel initialization pass.
- [ ] Collapse workspace-level commands/handlers/requests into world scope without
  merging definition registration, handler installation, binding resolution and
  menu placement into one operation. Preserve nearer disabled-handler blocking,
  conflict handling and stale-admission rejection; do not reroute on failure.
- [ ] Remove cross-workspace relocation checks/APIs. Keep checked panel movement
  within/between layouts and its real remaining document/worker invariants.
- [ ] Replace workspace-close worker/document preflights with the appropriate
  session/app shutdown APIs. Preserve pending-save/draft gates, cancellation,
  process termination and joining, admitted-write completion, and stale-result
  rejection. Dropping the world alone is not an orderly shutdown protocol.

### 1C. Migrate the foundation integrations and bEd2

- [ ] Update optional document/editor, terminal, language-server and panel adapters
  to use world-owned session services. Preserve standalone crate feature boundaries.
- [ ] Replace active-workspace queries and scope parameters in UI, menus,
  automation and panel creation. Keep no-selected-layout handling valid.
- [ ] Move bEd2 project root/state/settings to application-owned resources or
  components with a clear owner, not a replacement Workbench workspace entity.
  Define the distinct global/local settings schemas and inheritance explicitly.
- [ ] Make folder opening validate and launch a new application instance/window;
  route native menus, dialogs and other folder-open entry points through that
  policy. Launch failure must leave the current session and unsaved work intact.
  No native App teardown/restart machinery in this task.
- [ ] Preserve existing per-root configuration paths, persistence readiness,
  debouncing, retries, shutdown flushing and late-load protection while adapting
  `session.rs`/`storage.rs` to one live session. Keep the old on-disk recipes as an
  application DTO if useful until task 2; do not combine both migrations at once.
- [ ] Adapt the reflection prototype to one world/session and application-selected
  global/local payloads. Keep it isolated from production persistence initially.
- [ ] Define how separate instances observe global preference changes and avoid
  clobbering one another's writes. Audit same-project concurrent writers too;
  distinguish existing limitations from requirements introduced by folder launch.

### 1D. Verification and completion gate

- [ ] Core behavior: empty world/no selected layout; layouts/splits/tab ordering;
  selection/focus fallback; stale close target rejection; shared documents across
  viewers/layouts; panel cleanup; controlled document/worker shutdown.
- [ ] Commands: panel/world binding and handler resolution, conflict/disabled
  behavior, stale admission and operation without a selected layout.
- [ ] Application: startup/home/folder argument; new-folder launch and failure;
  existing session unchanged; settings inheritance; session restore; saves,
  autosave, terminal/LSP cleanup and quit flush errors. Exercise native opening and
  closing, not only mocked launch success.
- [ ] Run targeted package tests, bEd2 checks and affected optional-feature checks.
  Audit remaining workspace identifiers in v2 by meaning, not a blind rename:
  application settings/file-format vocabulary can remain.
- [ ] Update crate READMEs and decision records to describe actual behavior. Remove
  obsolete runtime workspace code/tests, not the invariants those tests protected.

**Task 1 done:** production v2 Workbench supports one world/session without a
workspace execution/ownership scope, consumers compile, folder launch is explicit,
and saves/document lifecycle/worker shutdown retain their protections. The current
application recipe format may still be in use. Begin production persistence work
only after this gate; do not assert completion without verification results.

## Task 2 — reusable marked-type world persistence

### 2A. Settle the snapshot and restore boundary

- [ ] Adapt/re-run the prototype against task 1's actual data model. Establish the
  smallest capture, application-routing and restore API with real callers.
- [ ] Choose direct reflection versus structural projections for each core type;
  keep a single live source of truth. Never serialize reverse relationship targets
  merely to compensate for skipped relationship hooks.
- [ ] Specify resource/unscoped-entity capture, marked panel-local components,
  structural inclusion, unsupported/unmarked concrete panels, reference closure,
  duplicate/unknown types, selection/order and library schema versions. Unsupported
  durable data must not silently disappear when overwriting a good session.
- [ ] Specify restore into an empty/admitted session initially, not arbitrary merge
  or replacement of a running world. Application approval precedes destructive
  work. Define failure effects, side-effect-free validation limits, and rollback
  or staged commit requirements before enabling production replacement.
- [ ] Keep persistence optional and headless-capable without a renderer/window or
  asset-backed scene spawning. Choose the smallest feature/dependency boundary
  consistent with Bevy's actual world-serialization dependencies.

### 2B. Implement the reusable layer

- [ ] Add the persisted-type reflection marker, deny-by-default extraction and
  load-time allowlist enforcement. Registration must not become a panel factory
  catalog or a requirement for ordinary panel composition.
- [ ] Capture marked values plus required structural metadata; support the host's
  global/local routing without reintroducing workspace membership as storage policy.
- [ ] Validate the envelope/schema, byte/entity/component limits, types, references,
  ownership, geometry and selections before live mutation. Do not trigger external
  loading or managed workers while inspecting untrusted stored state.
- [ ] Allocate/remap entity identities, restore durable concrete components and
  rebuild structure through controlled parent-first relationship insertion. Preserve
  required-component defaults, order and selection/focus despite attachment hooks.
- [ ] Recreate runtime state through explicit application/plugin activation only
  after the graph is ready. Regenerate presentation, reverse links and runtime
  bookkeeping; don't deserialize them as authority.
- [ ] Add tests for an independently defined marked component/resource without
  changes to a central panel enum, remapped references, runtime exclusion, shared
  viewer/document identity where supported, round trips, and rejected-load effects.

### 2C. Integrate production bEd2 storage

- [ ] Use distinct global/local settings and binding declarations to route durable
  data to the existing application-chosen destinations. Persist inheritance/unbind
  intent, not effective settings or runtime command handlers.
- [ ] Move panel-specific persistence declarations near their types/plugins;
  remove central `PanelKind`/field matching where reflected durable data replaces
  it. Keep file opening, canonical document creation and external runtime loading
  through their checked application paths.
- [ ] Keep global layout defaults/templates distinct from full session restoration:
  saving a tool/geometry default must not capture project paths or live documents.
- [ ] Preserve storage admission, canonical project identity, bounded reads,
  atomic writes, debounce/retry, malformed/newer-file protection and shutdown flush.
  Implement the selected multi-instance write policy; reflection doesn't solve it.
- [ ] Choose and document compatibility with existing `.bed` files before switching
  writers: deliberate migration or explicit rejection with preserved original files.
  Never treat a rejected old file as an empty session and overwrite it silently.
- [ ] Keep unsaved-content/history recovery a separately specified milestone unless
  explicitly included. Reopening a file descriptor is not unsaved-content recovery.

### 2D. Verification and completion gate

- [ ] Test fresh-world round trips of global/local settings, layout geometry/order,
  active layout, focus/tab selection, panel data and scoped binding declarations.
- [ ] Test invalid/unknown/newer data and cross-partition references; rejected loads
  leave live state and saved originals intact. Test write/flush failure and retry.
- [ ] Exercise native edit/layout changes → save → restart → restore, shared-file
  viewers, tool defaults versus sessions and clean runtime worker recreation.
- [ ] Run affected package/feature checks and native smoke routes. Update README
  contracts, context and older decision records; retire obsolete manual capture/
  restore paths and prototype scaffolding once equivalent coverage exists.

**Task 2 done:** marked durable types can round-trip without central application
panel recipes; Workbench owns graph correctness while bEd2 still owns destinations,
external I/O and recovery policy. Production storage/lifecycle protections are
verified, not replaced by the prototype's shortcuts.

## Verification commands (planned, not executed by this document change)

Start with the changed packages and expand to integrations affected by each phase:

```sh
cargo test --offline --locked -p bedtime-workbench2
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bed2 --features automation
cargo check --offline --locked -p bed2
cargo test --offline --locked -p bed2 --example persist_world
cargo run --offline --locked -p bed2 --example persist_world -- --roundtrip
cargo run --offline --locked -p bed2 --example persist_world -- --smoke
```

Add the affected editor/LSP/terminal/panel standalone and Workbench feature checks
from their maintained READMEs. The current prototype commands may change after
its task-1 adaptation; update this section rather than keeping stale commands.
Native smoke coverage must explicitly distinguish automated lifecycle/render checks
from pointer-driven editing/dragging and platform behavior that was not exercised.

## Relationship to earlier records

[Application composition](application-composition.md) remains the implemented
entity-first contract until migration lands. This decision supersedes its future
requirement for workspace execution scopes and its blanket exclusion of reusable
persistence. It does **not** reverse concrete component composition, no panel
factory registry, explicit handler installation, checked close decisions, or
application ownership of external I/O and recovery.

[Workbench documents](workbench-documents.md) retains its content/history/save/
source consistency requirements; workspace ownership there must be migrated, not
used as a reason to discard those requirements. Older composition/overview files
are historical where they disagree. Keep `CONTEXT.md` as the entry point and
record implementation/verification progress here when work actually occurs.
