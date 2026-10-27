# One world, one session; reflected workbench persistence

Decision date: 2026-10-24.
Status: **implemented** in production v2. Workbench now owns one World/session;
bEd2 enforces exclusive project admission and uses optional reflected graph
persistence with application-owned storage. Verification and limits are below.

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

Opening another folder attempts a separate application instance/window and requests
normal quit of the current instance, without waiting for child readiness. A project
folder is application data, not a reusable Workbench execution/ownership scope. Applications
may still call project-local settings/state "workspace" data.

**Multiple independent windows editing the same project are explicitly unsupported.**
bEd2 permits one live editing session per canonical project folder across application
instances. Reopening the current project must reuse the current session; a folder
already owned by another instance must be rejected unless the application can
direct the request to that existing session. Cross-instance focusing is optional,
not a prerequisite. Do not create a second independent session or add concurrent
project-session merging. Enforce exclusive project admission before restoration or
editing, including simultaneous launches, and release ownership when the session
ends or its process exits. This is application policy, not a Workbench scope.
Instances for different projects still share global preferences and need a separate
global write/refresh policy.

**Current implementation evidence (2026-10-24):** `opening.rs::poll_folder_opens`
reuses the current canonical project or launches the current executable with the
new root and `--no-automation`, then requests normal quit even on admission/launch
failure. Terminal directory helpers, File → Open Folder, Git's Open repository root
and automation share this route.

Tearing down/restarting a native Bevy App in-process is **not** a prerequisite or
an established capability. Native event-loop recreation/platform restrictions and
renderer lifetime need separate investigation if in-place replacement becomes a
requirement. Do not build a host/session-world framework speculatively.

#### Launch and quit

Implemented on 2026-10-24, superseding the initial parent-retention policy in the
historical Task 1 checklist below. For a different validated project, attempt the
existing process launch, complete the folder result and request normal
quit of the current instance. Do not wait for child readiness or restoration, add
a startup acknowledgement, or reset the live World. Launch failure must not veto
the quit request. This deliberately permits the parent to close without a usable
replacement. Canonical-root reuse and exclusive project admission remain. Invalid
paths fail validation without requesting quit. Admission failures for a validated
different project follow the same quit path as process-launch failures.

Use the existing close request so unsaved-document decisions, admitted writes,
storage flushing and worker cleanup retain their current behavior. Cancelling quit
can leave both instances open. Complete shell folder results before the close gate
starts rejecting actions; there is no requirement that the invoking shell survive
shutdown. File arguments continue opening tabs in the current instance.

Orchestration stays in bEd2's shared opening route. Shell replies are completed by
the existing application terminal poll before CloseSystems handles the queued quit.
The invoking shell can be terminated during shutdown; successful shell completion
is not a readiness barrier. Error wording makes no parent-preservation promise.

Verified on 2026-10-24:

- `cargo test --offline --locked -p bed2 --features automation`: 157 passed,
  two explicit opt-in tests ignored. New regressions cover canonical-root reuse,
  invalid-path rejection, failed replacement admission still requesting quit, and
  Save All / Discard / Cancel after a replacement request.
- `cargo check --offline --locked -p bed2` and the automation build passed.
- Native macOS/Metal `smoke_session.py` passed five nonblank captures: replacement
  quit prompt, cancellation retaining unsaved shared documents and the child,
  exclusive project admission, failed replacement followed by Save All and orderly
  quit, and restoration. Artifacts: `target/launch-quit-session-smoke`.
- Native `smoke_open.py` passed two nonblank captures and a real `cd` + no-argument
  `+open` from a clean home instance: the parent exited successfully and its
  non-automation replacement remained alive. Existing same-root/alias/file/error
  and restore checks passed. Artifacts: `target/launch-quit-open-smoke`.

The session smoke restores native focus and observes fresh render progress plus
the existing capture settling interval before screenshots. Native Linux behavior
and actual OS process-spawn failure were not exercised; the deterministic
replacement failure test holds the destination's project lock.

### 2. Add opt-in reflected world persistence afterward

A type explicitly opts in as Workbench-persisted reflection data. Reuse Bevy
0.20's `bevy_world_serialization::{DynamicWorld, DynamicWorldBuilder, WorldFilter}`
and reflection serialization/deserialization instead of inventing a parallel
component codec system. Registration supplies data/type knowledge, not a panel
catalog: ordinary concrete panel spawning/drawing must remain registry-independent.
The current marker/capture API and schema revision are recorded below.

Capture a **persistent projection** of the world, never all reflected state by
default. Ordinary durable components/resources should need only the marker and
normal reflection registration. Split mixed durable/runtime components where that
clarifies ownership; use source/restore descriptors for runtime-heavy state.
Never persist drawing pointers, processes, managed jobs, caches, presentation
rectangles, redundant ownership links, document save-in-flight state or GPU handles.

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

### Prototype evidence and production boundary

The initial 2026-10-24 experiment established marked extraction, reference remapping,
required defaults and the need to rebuild relationships explicitly because Bevy's
writer skips relationship hooks. Its workspace partitions, staging insertion and
synchronous writes were experimental, not production policy.

The adapted [example](../bed2/examples/persist_world.rs) now consumes the same
optional library as bEd2: one session, two layouts/eight panels, independent
host-selected global/project resources, no copied persistence implementation.
Its three tests, renderer-free round trip and native save/mutate/load/capture passed.
Example writes remain synchronous/non-atomic. Production preserves the existing
bounded, asynchronous, atomic storage and close gates.

The [library contract](../bedtime2/crates/bedtime-workbench2/README.md#optional-reflected-persistence)
and [application contract](../bed2/README.md#settings-layouts-and-persistence) specify
schema versions, durable components, trust boundaries and compatibility. Decoding
validates values without inserting components or constructing required defaults.
Trusted custom Serde/FromReflect/hooks must remain pure; external runtime starts
only after admission and complete graph validation. Arbitrary hook rollback is not
provided. Opaque values must not conceal entity references or asset handles.

## Marker-selected capture revision — 2026-10-24

The API now follows ordinary component composition. `Workbench::capture_world::<M>`
selects registered component/resource types carrying reflection metadata M and
returns a detached `CapturedWorld<M>` exposing Bevy's editable `DynamicWorld`.
Applications can filter or replace values before `serialize`; `deserialize` and
`Workbench::restore_world` validate the chosen marker, values and graph again.
Capture may contain incomplete projected data until encoding; no arbitrary filter
silently repairs ownership, order, selection or application entity references.

Workbench directly marks `Layout`, `Area`, durable `PanelInstance` fields,
`ActiveLayout`, `LayoutAreas`, `AreaPanels`, `InPanel` and its existing layout-order
resource. Ordered targets encode meaningful order/membership; `InLayout`/`InArea`
sources are rebuilt through normal attachment hooks. No private structural DTOs
or replacement live ordering state remain. Panel runtime key/draw/dirty fields and
runtime-only panel command parts are excluded. Restore allocates/remaps IDs, inserts
ordinary reflected data with relationship hooks enabled, attaches Workbench structure
in saved order, flushes maintenance, then reapplies saved selection/focus.

bEd2 uses independent `GlobalPersist` and `ProjectPersist` resource markers alongside
`WorkbenchPersist` graph components. Feature-owned descriptors are added directly to
the detached capture, without temporary live-world insertion or title changes.
Session plugin setup owns default project resources. Existing storage admission,
late-read merging, atomic/debounced writes, global merging and quit gates remain.

Library snapshots are schema **2** (application envelopes remain schema **2**).
Schema-1 reflected snapshots are explicitly rejected and preserved, including the
previous reflected global preference envelope; legacy application schema-1 global
value DTO migration remains supported. Unsaved-content recovery/live-session graph
merging remain outside this revision. See the maintained library/application READMEs
for the current contract; the initial implementation evidence below is historical
where it describes schema 1 and private structural projections.

Verification on 2026-10-24:

- Workbench with persistence/UI: **173 passed plus one doctest**; renderer-free
  persistence: **113 passed plus one doctest**. Minimal Workbench without
  persistence checked. bEd2 automation suite: **155 passed, 2 ignored**.
- The example's **three tests**, executable headless round trip and native
  save/mutate/load/render smoke passed.
- Regressions cover independent application markers on components/resources,
  detached value edits/filtering, rejected dangling references before live mutation,
  remapped resource references, ordinary application relationship hooks, unsupported
  panels, runtime-only panel-part exclusion, required-default nonexecution during
  validation, drawing capability retention, empty layouts and saved tab order/focus.
- Native macOS/Metal session smoke passed **4 captures**, including unsaved work,
  project exclusion, orderly Save All quit and restart/shared-viewer restoration.
  Native storage-policy smoke passed global/project values, autosave modes, layout
  defaults versus sessions, restart and no idle/startup rewriting.
- The first sandboxed native attempt could not access a GPU. Desktop-access runs
  passed. Native Linux/Windows behavior was not exercised; arbitrary hook rollback,
  graph merge into live sessions and unsaved-content/history recovery remain outside
  the contract.

## Generic relationship revision — 2026-10-24

`register_persisted_relationship::<R>(app)` installs typed mechanics for a Bevy
source/target pair in the reflection registry without marking either side. Marked
sources preserve their fields; marked targets preserve collection order and payload.
When only the target is marked, `Relationship::from` rebuilds source defaults.
Both marked sides must agree. Workbench's InLayout/InArea/InPanel use this same path;
geometry, structural roles, selected/focused membership and final focus policy remain
Workbench-specific. No panel factory catalog or new snapshot envelope was added.

Pure validation constructs a generic relationship plan before live allocation:
reference closure, single-owner consistency per pair, duplicate members, target
cardinality, self-link permissions and acyclic linked-spawn ownership. Non-owning
cycles and allowed self-links remain supported. Restore initializes ordinary/target
data before source hooks, attaches through normal Bevy operations in ownership order,
drains attachment maintenance, then applies target fields while retaining ignored
runtime fields. Trusted hooks must preserve membership; arbitrary rollback remains
outside the contract. Generic graph traversal is iterative to the entity limit.

Schema **2** remains current. Canonical IDs now use valid entity indices instead of
opaque `from_bits` values, avoiding collision with Entity::PLACEHOLDER (needed for
empty one-to-one collections). Earlier schema-2 IDs retain precedence when declared
as snapshot identities, so existing Workbench snapshots remain readable. Registration
is needed for ordered target handling; unregistered mapped source insertion still
supports ordinary Bevy reverse-membership hooks.

Verification on 2026-10-24:

- Workbench UI/persistence: **183 passed plus one doctest**; headless persistence:
  **123 passed plus one doctest**. Workbench without persistence checked. bEd2's
  automation suite: **155 passed, 2 ignored**. Example: **3 tests**, executable
  headless round trip and native save/mutate/load/render smoke passed.
- Ten new regressions cover source/target field data, ordered collections, early
  parent-data visibility, late selection preservation with ignored runtime fields,
  target-only default sources and parent-first ownership, rejected inconsistent/
  duplicate/self/one-to-one/linked-cycle graphs before mutation, accepted non-owning
  cycles/self-links, empty one-to-one sentinels, legacy schema-2 ID compatibility,
  and a full **4096-entity** hierarchy round trip.
- Native macOS/Metal session smoke passed **4 captures** through Save All quit and
  restart/shared-viewer restoration. Storage-policy smoke passed autosave modes,
  global/project settings, tool defaults versus sessions, restart and idle no-write
  checks. Native example rendering passed. Native Linux/Windows behavior was not
  exercised; arbitrary hook rollback and live-session graph merging remain excluded.

## Scoped layouts and startup restore boundary — 2026-10-24

World restoration establishes a new session only. Hosts call
`Workbench::finish_world_restore` after their startup loading barrier; further world
restoration is rejected. Bevy Startup alone cannot define that boundary because
production storage loads asynchronously. Capture/save/decoding remain runtime APIs.
Clearing the World or persistence-marked data cannot infer runtime-service cleanup;
in-process session/workspace replacement remains unsupported.

`capture_layout`, editable `CapturedLayout`, its serialization/deserialization and
`replace_layout` reuse the reflected engine. A layout capture contains one linked
ownership tree, with no resources/ActiveLayout and no external entity references.
Replacement preserves the destination's order/activation, gives the entire layout
fresh identities, and uses ordinary panel-close ownership/cleanup. Documents and
session services survive; viewer drafts block replacement. Root slot metadata and
external activation remain application policy.

bEd2 global layout defaults now store reflected snapshots. The ToolKind/template
constructor path is removed from live code. Reset deliberately recreates its layout
instead of matching/reusing old tools or gathering extra tabs. Legacy global recipes
migrate through isolated read-only decoders, preserving values until a real edit;
the legacy reflected type is excluded from Bevy auto-registration to avoid colliding
with the current type path. Features own their creation commands/View contributions;
the application composes menus after plugin setup. Factories are creation actions,
independent of serialization/reconstruction.

See the maintained Workbench/bEd2 README contracts for ownership and compatibility.

Verification on 2026-10-24 for this revision:

- Workbench with persistence/UI: **188 tests plus one doctest passed**; renderer-free
  persistence: **128 tests plus one doctest passed**. Minimal Workbench without
  persistence checked successfully.
- bEd2 automation: **155 passed, 2 explicit opt-ins ignored**. The example's three
  tests and executable headless round trip passed.
- Native macOS/Metal storage-policy smoke passed with explicit destructive Reset
  assertions: the target layout and panel identities retire, sibling panels survive,
  reflected defaults persist and sessions restore without idle rewriting. Artifacts:
  `target/layout-persistence-smoke`.
- Native example smoke passed both seeded startup and `--load` startup, followed by
  runtime layout reload and a verified nonblank `target/persist-world-smoke.png`.
- Regressions cover rejected world restore after startup sealing; scoped ownership,
  data/remapping/runtime defaults; preserved sibling layouts/resources/documents;
  rejected resources, external references, invalid geometry and drafts before
  replacement; retired panel-work cancellation; filtered project sources; and both
  legacy global formats without startup rewriting and with upgrade on a real edit.
- The combined test build exposed duplicate reflected paths in two function-local
  one-to-one relationship fixtures. Distinct fixture names remove that collision;
  both full Workbench feature suites subsequently passed.

Native Linux/Windows were not exercised. Whole-session replacement, unsaved-content
recovery and arbitrary trusted-hook rollback remain outside this contract.

## Execution plan

Implementation followed the order below. Checked items are complete; each phase should
leave the affected targets coherent and buildable; coordinate internal breaking API
changes with their consumers rather than adding a long-lived compatibility facade.
Do not modify v1 workspaces or unrelated uncommitted work.

## Task 1 — remove Workbench workspaces

### Replacement ownership model

| State | Owner after removal |
| --- | --- |
| Canonical document content, identity, history and save/source generations | Document entity; source keys deduplicate across the World |
| Save destination exclusion | World resource |
| Managed work and shutdown admission | `ManagedWork` resource; optional panel/document entity shortens a job's lifetime |
| Layout and command requests | Separate World resources; batches capture targets before execution |
| Command handlers and bindings | Panel-owned entities/components or unscoped World entities; panel → World resolution |
| Layout order and selection revision | World resources; `ActiveLayout` remains the single optional selected entity |
| Areas, tabs and document views | Existing explicit entity relationships |
| Menu/status/frontend state | `WorkbenchUi` resource; presentation stays on the surface/view entities |
| Project identity, title and exclusive admission | bEd2 resources; OS-held lock precedes folder restore/editing |
| Global values and project overrides | Distinct bEd2 resources; inheritance remains per value |
| Filesystem, Git, LSP configuration and Structure source | One service/configuration resource per World; panel-local views remain entities |
| Five layout slots | Components on retained layout entities |
| Project metadata storage admission/read/write state | One bEd2 session-file resource; existing bounded/atomic recipe writer retained for Task 1 |

Home without an explicit folder stays memory-only. An explicit home-folder open
is a project and acquires project admission. Reopening the current project reuses
it; another folder launches the same executable with that canonical folder and an
internal flag disabling stdio automation in the GUI child.
Folder acknowledgement means validated reuse/process launch, not completed child
restoration. Child startup independently enforces admission. No cross-instance
focus broker is required. Global preference writes lock, read, merge independent
values against the writer's baseline, and atomically replace; local edits win
same-value conflicts, while untouched values refresh from disk in the background.

### 1A. Inventory and settle the replacement data model

- [x] Map production workspace dependencies across Workbench, bEd2, automation,
  Editor2, LSP2, Terminal2, Filetree2, Model2, Mascots2 and tests/examples. Read the
  current implementation rather than relying on older migration documents.
- [x] Record the owner for every affected store/queue/link: documents and document
  IDs/indexes; managed jobs/processes; menu/status state; command requests/default
  bindings/handlers; project root; settings; layout slots; persistence admission.
- [x] Specify one selected layout per world, panel → world command resolution,
  resource-backed session-wide stores and the remaining layout/area/panel/document
  relationships. Keep panel-owned work and document-view links explicit.
- [x] Identify which stale-target stamps are still necessary. Remove workspace
  selection generations, but retain checks for changed layout/targets, handler
  replacement and stale async document/source/save results.
- [x] Choose a minimal folder-launch implementation and root-normalization/error
  contract for bEd2. Explicitly decide home/no-folder behavior and enforce one live
  session per canonical project across instances. Reuse the current session for
  its folder; reject a project owned elsewhere unless routing to its existing
  session is supported. Don't retain a hidden in-world session switcher.

### 1B. Remove core workspace ownership and routing

- [x] Move session-wide document/work/request stores into world resources and
  update APIs to use the world plus relevant document/layout/panel identities.
  Migrate affected core tests/callers alongside each owner change.
- [x] Remove `Workspace`, `WorkspaceId`, `InWorkspace`/`WorkspaceMembers`,
  `ActiveWorkspace`, active/running membership propagation, workspace background
  updates, activation/close requests and workspace-only reconciliation paths.
- [x] Keep `Layout`, `Area`, `PanelInstance`, `InLayout`, `InArea`, tab/area order,
  layout selection and focus/selection fallback semantics. Keep required components
  supplying defaults/drawing immediately; no delayed panel initialization pass.
- [x] Collapse workspace-level commands/handlers/requests into world scope without
  merging definition registration, handler installation, binding resolution and
  menu placement into one operation. Preserve nearer disabled-handler blocking,
  conflict handling and stale-admission rejection; do not reroute on failure.
- [x] Remove cross-workspace relocation checks/APIs. Keep checked panel movement
  within/between layouts and its real remaining document/worker invariants.
- [x] Replace workspace-close worker/document preflights with the appropriate
  session/app shutdown APIs. Preserve pending-save/draft gates, cancellation,
  process termination and joining, admitted-write completion, and stale-result
  rejection. Dropping the world alone is not an orderly shutdown protocol.

### 1C. Migrate the foundation integrations and bEd2

- [x] Update optional document/editor, terminal, language-server and panel adapters
  to use world-owned session services. Preserve standalone crate feature boundaries.
- [x] Replace active-workspace queries and scope parameters in UI, menus,
  automation and panel creation. Keep no-selected-layout handling valid.
- [x] Move bEd2 project root/state/settings to application-owned resources or
  components with a clear owner, not a replacement Workbench workspace entity.
  Define the distinct global/local settings schemas and inheritance explicitly.
- [x] Make folder opening validate and launch a new application instance/window;
  route native menus, dialogs and other folder-open entry points through that
  policy. Enforce exclusive project admission on startup too, before restoration
  or editing; handle launch races and process-exit ownership release. Duplicate
  project rejection and launch failure must leave the existing session and unsaved
  work intact. No native App teardown/restart machinery in this task.
- [x] Preserve existing per-root configuration paths, persistence readiness,
  debouncing, retries, shutdown flushing and late-load protection while adapting
  `session.rs`/`storage.rs` to one live session. Keep the old on-disk recipes as an
  application DTO if useful until task 2; do not combine both migrations at once.
- [x] Adapt the reflection prototype to one world/session and application-selected
  global/local payloads. Keep it isolated from production persistence initially.
- [x] Define how separate instances observe global preference changes and avoid
  clobbering one another's writes. Project files have one admitted session writer;
  same-project independent sessions are unsupported. Distinguish existing
  limitations from requirements introduced by folder launch.

### 1D. Verification and completion gate

- [x] Core behavior: empty world/no selected layout; layouts/splits/tab ordering;
  selection/focus fallback; stale close target rejection; shared documents across
  viewers/layouts; panel cleanup; controlled document/worker shutdown.
- [x] Commands: panel/world binding and handler resolution, conflict/disabled
  behavior, stale admission and operation without a selected layout.
- [x] Application: startup/home/folder argument; new-folder launch and failure;
  same-project reuse/rejection, canonical-path aliases, simultaneous-launch
  exclusion and ownership release after normal exit or crash; existing session
  unchanged; settings inheritance; session restore; saves,
  autosave, terminal/LSP cleanup and quit flush errors. Exercise native opening and
  closing, not only mocked launch success.
- [x] Run targeted package tests, bEd2 checks and affected optional-feature checks.
  Audit remaining workspace identifiers in v2 by meaning, not a blind rename:
  application settings/file-format vocabulary can remain.
- [x] Update crate READMEs and decision records to describe actual behavior. Remove
  obsolete runtime workspace code/tests, not the invariants those tests protected.

**Task 1 done:** production v2 Workbench supports one world/session without a
workspace execution/ownership scope, consumers compile, folder launch is explicit,
and saves/document lifecycle/worker shutdown retain their protections. The current
application recipe format may still be in use. Begin production persistence work
only after this gate; do not assert completion without verification results.

## Task 2 — reusable marked-type world persistence

### 2A. Settle the snapshot and restore boundary

- [x] Adapt/re-run the prototype against task 1's actual data model. Establish the
  smallest capture, application-routing and restore API with real callers.
- [x] Choose direct reflection versus structural projections for each core type;
  keep a single live source of truth. Ordered relationship targets carry durable
  order/membership; reconstruct sources rather than persisting both directions.
- [x] Specify resource/unscoped-entity capture, marked panel-local components,
  structural inclusion, unsupported/unmarked concrete panels, reference closure,
  duplicate/unknown types, selection/order and library schema versions. Unsupported
  durable data must not silently disappear when overwriting a good session.
- [x] Specify restore into an empty/admitted session initially, not arbitrary merge
  or replacement of a running world. Application approval precedes destructive
  work. Define failure effects, side-effect-free validation limits, and rollback
  or staged commit requirements before enabling production replacement.
- [x] Keep persistence optional and headless-capable without a renderer/window or
  asset-backed scene spawning. Choose the smallest feature/dependency boundary
  consistent with Bevy's actual world-serialization dependencies.

### 2B. Implement the reusable layer

- [x] Add the persisted-type reflection marker, deny-by-default extraction and
  load-time allowlist enforcement. Registration must not become a panel factory
  catalog or a requirement for ordinary panel composition.
- [x] Capture marked values plus required structural metadata; support the host's
  global/local routing without reintroducing workspace membership as storage policy.
- [x] Validate the envelope/schema, byte/entity/component limits, types, references,
  ownership, geometry and selections before live mutation. Do not trigger external
  loading or managed workers while inspecting untrusted stored state.
- [x] Allocate/remap entity identities, restore durable concrete components and
  rebuild structure through controlled parent-first relationship insertion. Preserve
  required-component defaults, order and selection/focus despite attachment hooks.
- [x] Recreate runtime state through explicit application/plugin activation only
  after the graph is ready. Regenerate presentation, reverse links and runtime
  bookkeeping; don't deserialize them as authority.
- [x] Add tests for an independently defined marked component/resource without
  changes to a central panel enum, remapped references, runtime exclusion, shared
  viewer/document identity where supported, round trips, and rejected-load effects.

### 2C. Integrate production bEd2 storage

- [x] Use distinct global/local settings and binding declarations to route durable
  data to the existing application-chosen destinations. Persist inheritance/unbind
  intent, not effective settings or runtime command handlers.
- [x] Move panel-specific persistence declarations near their types/plugins;
  remove central `PanelKind`/field matching where reflected durable data replaces
  it. Keep file opening, canonical document creation and external runtime loading
  through their checked application paths.
- [x] Keep global layout defaults/templates distinct from full session restoration:
  saving a tool/geometry default must not capture project paths or live documents.
- [x] Preserve storage admission, canonical project identity, bounded reads,
  atomic writes, debounce/retry, malformed/newer-file protection and shutdown flush.
  Preserve exclusive project admission and the selected global preference
  write/refresh policy across instances; reflection doesn't solve either.
- [x] Choose and document compatibility with existing `.bed` files before switching
  writers: deliberate migration or explicit rejection with preserved original files.
  Never treat a rejected old file as an empty session and overwrite it silently.
- [x] Keep unsaved-content/history recovery a separately specified milestone unless
  explicitly included. Reopening a file descriptor is not unsaved-content recovery.

### 2D. Verification and completion gate

- [x] Test fresh-world round trips of global/local settings, layout geometry/order,
  active layout, focus/tab selection, panel data and scoped binding declarations.
- [x] Test invalid/unknown/newer data and cross-partition references; rejected loads
  leave live state and saved originals intact. Test write/flush failure and retry.
- [x] Exercise native edit/layout changes → save → restart → restore, shared-file
  viewers, tool defaults versus sessions and clean runtime worker recreation.
- [x] Run affected package/feature checks and native smoke routes. Update README
  contracts, context and older decision records; retire obsolete manual capture/
  restore paths and prototype scaffolding once equivalent coverage exists.

**Task 2 done:** marked durable types can round-trip without central application
panel recipes; Workbench owns graph correctness while bEd2 still owns destinations,
external I/O and recovery policy. Production storage/lifecycle protections are
verified, not replaced by the prototype's shortcuts.

## Verification commands

Use these commands to reproduce the focused gates:

```sh
cargo test --offline --locked -p bedtime-workbench2 --features persistence
cargo test --offline --locked -p bedtime-workbench2 --no-default-features --features persistence
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bed2 --features automation
cargo check --offline --locked -p bed2
cargo test --offline --locked -p bed2 --example persist_world
cargo run --offline --locked -p bed2 --example persist_world -- --roundtrip
cargo run --offline --locked -p bed2 --example persist_world -- --smoke
```

Add the affected editor/LSP/terminal/panel standalone and Workbench feature checks
from their maintained READMEs. The example consumes the optional production API; its I/O remains illustrative.
Native smoke coverage must explicitly distinguish automated lifecycle/render checks
from pointer-driven editing/dragging and platform behavior that was not exercised.

## Implementation and verification — 2026-10-24

- Task 1 removed workspace types/membership/activation/facades, collapsed command
  resolution and queues to panel/world resources, and migrated every production
  v2 adapter before replacing bEd2's old session recipes. The existing source/save,
  draft, history and worker shutdown regressions passed at that gate.
- Task 2 added optional library schema 1 using ephemeral structural projections;
  unsupported concrete panels fail capture. Validation checks types/counts/reference
  closure/mapping, structural roles, geometry/order/selections and asset exclusion
  before mutation. Tests cover required-default nonexecution during prepare, unknown/
  unmarked/newer/duplicate data, remapping, runtime exclusion, nonempty destination
  rejection and full multiple-layout round trips.
- bEd2 schema 2 stores separate graph/project-resource payloads; global storage uses
  its own resource-only payload. Slot ownership/uniqueness and resource-only
  partitions validate before startup retirement. Distinct setting/binding layers
  preserve per-value inheritance and explicit unbinds. Feature-owned descriptors
  replace the central panel-recipe enum. Global schema 1 migrates on an edit;
  old project records are explicitly rejected and preserved.
- Latest application suite: **155 passed, 2 ignored**. Latest Workbench with UI and
  persistence: **166 passed plus one doctest**; renderer-free persistence:
  **106 passed plus one doctest**. The three example tests and executable round
  trip passed. Normal bEd2 checking/building and the automation build passed.
- Affected integration suite passed: Filetree **101**, Terminal **47**, Structure
  **9**, automation **12**, Editor **64** (two performance probes ignored), model
  **62**, mascots **5**, and LSP backend/stdio/Workbench suites **9/15/6**. Real-server
  and performance probes remained explicit opt-ins. Standalone editor/model/mascot
  examples, headless LSP, bare Workbench, UI-only and native-only Workbench checked.
- Project admission tests cover canonical aliases, simultaneous subprocess launches,
  distinct config directories, ordinary exit and process exit without destructors.
  Global preference tests cover independent edits and refresh without rewrite.
  Storage tests cover preserved invalid/old files, late settings/bindings/visibility
  merges, live-document admission, unsupported panel capture and write/retry/flush.
- Native macOS/Metal session smoke: **4 captures**, real text input/unsaved edits,
  a second folder process, same-root reuse, duplicate-project rejection, parent
  preservation, Save All quit and restart with two viewers sharing one canonical
  document. The quit test waits for the rendered button before clicking.
- Native settings/storage policy smoke passed Timer/Change Focus/Never/manual Save,
  menu actions, tool defaults excluding file paths, restart and idle no-write checks.
  Real `+open`/`+o` helper smoke passed **2 captures**. The adapted example passed
  save/mutate/restore/render capture. Task/process lifecycle smoke passed **6 captures**; terminal lifecycle passed
  **15 captures**, including panel/layout cleanup, live foreground processes and
  orderly quit with a running terminal.

Native smokes use injected pointer/key/text events and real windows/processes;
OS file-picker clicks, all old feature smokes and native Linux/Windows behavior
were not rerun. Library feature checks do not establish cross-platform native App
teardown/restart, which remains outside this implementation. Filetree non-UTF8
fixtures and socket tests ran outside the filesystem sandbox. The existing editor
standalone check reports its preexisting conditional unused-mut warning.

The same-canonical-project exclusion is per-user and per canonical root; it does
not exclude overlapping ancestor/descendant projects. Folder acknowledgement means
process launch, not child restoration success. Unsaved-content/history recovery and
arbitrary graph merge into a running session remain outside this milestone.

## Relationship to earlier records

[Application composition](application-composition.md) retains the entity-first
composition decisions. This implementation supersedes its former requirement for workspace execution scopes and its blanket exclusion of reusable
persistence. It does **not** reverse concrete component composition, no panel
factory registry, explicit handler installation, checked close decisions, or
application ownership of external I/O and recovery.

[Workbench documents](workbench-documents.md) retains its content/history/save/
source consistency requirements; workspace ownership there is superseded by world ownership, without discarding
those requirements. Older composition/overview files
are historical where they disagree. Keep `CONTEXT.md` as the entry point and
record implementation/verification progress here when work actually occurs.
