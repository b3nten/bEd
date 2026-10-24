# Entity-first application composition

Ownership update (2026-10-24): the implemented [world/session migration](workbench-world-session-plan.md)
supersedes workspace entities/membership, workspace command scope and the blanket
exclusion of reusable persistence in this record. The World owns one session;
layouts/panels/documents and their checked save/source/close invariants remain.
Use the current crate README for signatures and opt-in reflected persistence.
The scoped-layout revision also supersedes the tool-recipe/default reset behavior:
world restoration is sealed after startup, and reflected layout defaults destructively
replace one layout while retaining canonical documents and session services.

Status: in progress, reviewed against the worktree on 2026-10-21. This is the
current decision record for panel composition and application policy. It supersedes
the registry, delayed initialization and core snapshot/restore portions of
[workbench composition](workbench-composition-plan.md); that older document remains
historical evidence, not the current API contract. The
[workbench documents](workbench-documents.md) record retains the document-model
contracts and now links this application-owned persistence boundary. A checked item below means implementation is present in the inspected
code, **not** that verification passed.

> **Accepted next direction (2026-10-24; not yet implemented):** the
> [one-world/session and reflected persistence plan](workbench-world-session-plan.md)
> supersedes future workspace execution scopes and the blanket exclusion of reusable
> persistence below. This file still describes the implemented workspace APIs and
> original phase history until that migration lands. Concrete entity-first panels,
> no factory catalog, explicit close decisions and host-owned I/O/recovery remain.

## Accepted ownership and design

### Concrete entities, not registered panel types

A panel is an ordinary concrete Bevy component. The usual declaration is:

```rust,ignore
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
struct Notes { text: String }

impl Panel for Notes {
    const KEY: &'static str = "example.notes";
    const LABEL: &'static str = "Notes";
    // With UI enabled, draw is an ordinary function with exclusive World access.
}

world.spawn((Notes { text: "Draft".into() }, InArea(area)));
```

`Panel` supplies presentation, not construction. It has no `initialize` method.
Required components provide missing defaults, `PanelInstance` metadata and the
draw capability as soon as composition applies; explicitly supplied state and
required-component overrides survive. There is no `PanelType`, registration
entity, catalog lookup or scheduled initialization step. Reconciliation validates
relationships/geometry and updates bookkeeping; it does not finish constructing
panels. Do not reintroduce a plugin-order-dependent initialization sequence.

`Panel::KEY` remains descriptive identity for application inspection or persistence
policy. It is not a constructor lookup or globally validated type identity; an
empty descriptive key is usable in practice. Keeping a key does not imply a
registry. The application can choose its own stable restore descriptors and map
them to concrete code without teaching Workbench how to construct arbitrary types.

Applications own creation discovery and commands, such as bEd2's View menu. For
creation drags Workbench accepts `PanelFactory::new(label, create)`, with
`create: fn(&mut World, Entity) -> Entity`. This application-owned infallible
function must create exactly one panel in the supplied area and return it. Validate
host-specific inputs before offering a factory. No global registration is needed
and empty areas do not offer a library-maintained type picker.

### Selection is not focus

Attaching `(ConcretePanel, InArea(area))` selects the new tab in that destination
area. It does not take area focus from another area. `Workbench::select_panel`
changes tab selection; `focus_area` changes area focus. `focus_panel` remains an
explicit convenience that does both. A host that opens a preview, background view
or navigation result must choose focus intentionally, rather than repair implicit
focus changes after construction.

Lifecycle hooks maintain immediate workspace ownership, active/running membership,
and selection/focus fallbacks on attachment, move, relationship removal and despawn.
Direct World operations expose these changes immediately; deferred Commands expose
them when applied. Keep deferred writers/readers ordered normally. Reconciliation
is the validation boundary for a valid composed graph, not an initialization hook.
Activation hooks invalidate interaction stamps immediately on marker changes,
including leaving and returning before a reconciliation pass.

`Workbench::relocate_panel` is the checked path between layouts/workspaces. A
cross-workspace move rejects drafts and panel-owned work, then detaches `DocumentView`;
canonical documents never migrate implicitly. Hosts must resolve other scoped links
before moving. Raw reparenting of a bound viewer across workspaces is unsupported
and rejected by graph validation.

### Commands have independent definitions and three scopes

Command definition registration and handler installation are independent. Installing
a handler must not silently install metadata; definition registration and world
handler installation must work without any selected workspace.

Bindings overlay by chord at the nearest scope: panel → workspace → world. Handler
selection independently resolves by command ID along panel → workspace → world.
A disabled nearer handler blocks fallback. Conflicts, handler errors and stale
admitted targets must not reroute to another binding/handler. Admitted handlers
capture generation and scope identity; replacement or retargeting rejects the
request instead of invoking a new handler under an old admission. Definitions,
bindings, handlers and menu placements remain separate data.

`ContextCommandRequests` is a workspace-owned component. `GlobalCommandRequests`
is a distinct application resource, including commands when no workspace exists;
resources must not be reused as workspace components. `Workbench::register_handler`
is workspace-scoped; `install_global_handler` explicitly targets world scope.
Bevy system handlers remain available. For simple direct calls,
`CommandHandler::<C>::new(call)` can be attached to its panel/workspace, or an
unscoped world-handler entity, without running application code during composition.

Document handlers are an explicit host choice through `register_document_handlers`;
core plugin installation no longer installs Save/Undo/Redo policy. Document shortcut
installation is separately opt-in.

### Applications own sessions, settings and recovery

Core workspace/layout snapshot types, capture/restore/replace APIs, template policy
and snapshot-derived workspace change notifications are removed. There is no
replacement generic serialization registry. Persistence must describe the real
application state, validate external data before composing live entities, and
reconstruct concrete panels/documents explicitly. Library document history/save
checkpoints and worker ownership remain reusable; they are not session persistence.

Global settings belong to application resources; per-value overrides and roots
belong to workspace entities. bEd2 now stores versioned RON `.bed` files centrally:
`preferences.bed` holds global settings/layout defaults, and one
`workspaces/<canonical-root-digest>.bed` holds each folder workspace's overrides,
visibility and structural session. Folder opening reuses canonical identities.
No workspace metadata is read/written inside a project; old JSON is not imported.
Default home workspace state stays in memory. See the maintained
[settings/layout/persistence contract](../bed2/README.md#settings-layouts-and-persistence).

Five application layout slots retain live entities. Global defaults contain a
reflected layout graph without project paths/document viewers. Reset recreates the
selected layout and retires its panels/workers, retaining canonical documents and
session services. Startup resolves persistence before composing initial tools.
Settings inherits per value, and document autosave uses canonical checked saves.
The titlebar uses the existing concrete panel-factory drop path.

Storage admission, errors/retry, debouncing and shutdown flushing remain explicit
application responsibilities. Admitted writes survive owner retirement. Sessions
restore concrete panels and file descriptors, not unsaved content/history/process
state. Save As, New/Untitled and recovery remain separate milestones.

### Close requests require an application decision

Destructive UI layout intentions produce `PanelCloseRequested` with captured
workspace/layout stamps and edit targets. The application may reject, retain while
prompting/saving, or approve with `Workbench::commit_close`. Commit revalidates the
stamp/targets before structural mutation; delayed approval cannot act on a silently
changed graph. `close_targets` is a read-only policy preflight using the same geometry
planner as commit, including the exact panels removed by joins/docks. Panel/area
closure and destructive join/dock gestures use this path.

Raw despawn still cleans up ownership and managed work. It has no veto and cannot
show a save/discard/recovery prompt. Controlled workspace/document APIs retain their
preflight gates; the host owns user policy and waits for admitted writes/work cleanup
before destructive shutdown. Closing a document viewer is distinct from discarding
its retained canonical document.

### Automation follows real ownership

Generic `panel.open`, `workspace.capture`, `workspace.restore`, `layout.capture`,
`layout.restore` and `layout.replace` are removed. Automation may observe live
entities and invoke real transitions, but must not recreate a second panel registry
or persistence model. Hosts expose concrete creation/session operations through
application UI or explicit custom operations. Rendered PNG `capture` remains a
different operation and is unaffected by removal of session snapshots.

## Phase checklist

### Core entity-first model — implementation present

- [x] Concrete panels require their own `PanelInstance::of::<Self>()`; no `Panel`
  initializer or type catalog. Metadata and drawing exist immediately.
- [x] New panel attachment selects its destination tab without taking area focus;
  explicit selection/focus APIs and relationship fallback hooks are present.
- [x] Reconciliation is graph validation/bookkeeping, not panel initialization.
- [x] Core snapshot/restore/template and snapshot-notification paths are removed.
- [x] Command definitions/installations are independent; three-scope resolution,
  world requests, explicit document handlers and direct attached handlers exist.
- [x] Destructive UI intentions use `PanelCloseRequested`/`commit_close`; raw
  despawn remains cleanup without application acceptance.
- [x] Generic automation creation/session endpoints are removed.
- [x] Exact close-target planning, checked cross-workspace relocation, immediate
  activation-stamp invalidation and admitted handler generation/scope checks exist.

Evidence: [layout.rs](../bedtime2/crates/bedtime-workbench2/src/layout.rs),
[membership.rs](../bedtime2/crates/bedtime-workbench2/src/membership.rs),
[composition.rs](../bedtime2/crates/bedtime-workbench2/src/composition.rs),
[commands.rs](../bedtime2/crates/bedtime-workbench2/src/commands.rs),
[document_commands.rs](../bedtime2/crates/bedtime-workbench2/src/document_commands.rs),
[automation adapter](../bedtime2/crates/bedtime-automation2/src/workbench.rs).

### Concrete panels and application policy — implemented slice

- [x] Inspected bEd2 startup, Theme, viewport, Files/Code, Tasks, Structure and
  Terminal creation uses concrete components, with application-owned View choices.
- [x] bEd2 explicitly installs document handlers and consumes panel close intentions;
  its current panel policy retains canonical documents and rejects viewer drafts.
- [x] Workspace-owned project roots and file visibility storage exist; Files and
  View-created terminal defaults consult workspace data.
- [x] Centralized RON global/workspace files, per-value inheritance and the
  General/Appearance/Editor settings frontend. Keybinding settings are deferred.
- [x] Checked document autosave (Never, Change Focus, one-second-idle Timer).
- [x] Five retained application layout slots, global defaults and preserving reset.
- [x] Titlebar Terminal/Files/Structure/Settings click and factory-drag creation.
- [x] Native app/File/Edit menus expose Quit, Open Folder…, Save, Undo/Redo and
  Cut/Copy/Paste. Native View exposes auxiliary panels without a Code creation route. Save As remains unfinished.
- [x] Integration review covered creation, command admission, close targets/stamps,
  settings load/write/retry, canonical shared-project storage and automation.
  Reviewed defects received fixes and regression coverage.
- [x] Targeted builds/tests, feature boundaries and desktop scenarios ran; results
  and intermittent verification failures are recorded below. This does not imply
  the later session/recovery milestones are complete.

Evidence: [main.rs](../bed2/src/main.rs), [menus.rs](../bed2/src/menus.rs),
[settings.rs](../bed2/src/settings.rs), [storage.rs](../bed2/src/storage.rs),
[keybindings.rs](../bed2/src/keybindings.rs), [files.rs](../bed2/src/files.rs),
[close.rs](../bed2/src/close.rs), and the current panel crate READMEs.

### Application sessions and document recovery — structural slice implemented

- [x] Application-owned bounded/versioned folder workspace/layout recipes and storage.
- [x] Concrete panel kinds/file descriptors, tab selection, area focus and active
  layout reconstruct without core snapshots. Detailed widget state, history and
  unsaved content are intentionally not in this format.
- [x] Default workspace browses home and remains memory-only; only explicit CLI/
  Open Folder identities opt into project metadata. Global settings still persist.
- [x] Meaningful workspace/layout mutations buffer before asynchronous atomic save;
  clean quit flushes without a workspace decision, and dirty documents still prompt.
- [ ] Save As with source-identity changes and error/cancellation handling.
- [ ] Complete save/recovery flow for new Untitled documents and missing/deleted sources.
- [ ] Application workspace-management UI and policy beyond existing core operations.
- [ ] Recovery/close flows and durable settings/session error behavior verified with
  real I/O and native UI where applicable.

These pending items are accepted application responsibilities, not implemented
capabilities promised by Workbench. The [Workbench README](../bedtime2/crates/bedtime-workbench2/README.md)
and [bEd2 README](../bed2/README.md) describe current public contracts. Verification
commands in [CONTEXT](../CONTEXT.md) remain recipes; the following records actual
runs for this refactor.

## Historical verification — 2026-10-19

- `cargo test --offline --locked -p bedtime-ui2 -p bedtime-clipboard2 -p bedtime-workbench2 -p bedtime-filetree2 -p bedtime-terminal2 -p bedtime-automation2 -p bed2 -- --test-threads=1`: **438 unit tests and one workbench doctest passed**.
- `cargo test --offline --locked -p bedtime-workbench2 --no-default-features`:
  **138 unit tests and one doctest passed**. Separate `ui` and `native` feature-only
  checks with defaults disabled passed.
- `cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter`:
  **41 passed, two explicit large performance fixtures ignored**.
- `cargo test --offline --locked -p bedtime-treesitter2 -p bed-structure2`:
  **seven analysis and seven Structure tests passed**; the explicit analysis timing
  fixture remained ignored.
- `cargo build --offline --locked -p bed2 --features automation` passed. The basic,
  Cube, Tasks, Terminal, Code/Structure, Editor and Files desktop smoke scripts each
  passed with isolated `XDG_CONFIG_HOME` directories under `target`. This verifies
  the migrated application-menu creation paths, not OS-native file-drag coverage.
- `git diff --check` and Python compilation of the seven migrated smoke scripts
  passed.

Verification limitations: one parallel terminal unit-test run timed out in
`stopping_an_exited_terminal_is_idempotent_and_keeps_restart_available`; its isolated
rerun and the complete serial suite passed. Earlier Terminal and Files desktop
runs each returned a completely transparent GPU capture despite live UI state;
full smoke reruns passed. These intermittent failures are retained as unresolved
verification observations, not claimed fixed or attributed to this refactor.
Recovery and workspace-management items above remain unfinished; the constrained
native-menu and workspace-autosave follow-up is described below.


## Workspace and menu policy follow-up — historical 2026-10-19

`WorkspaceRoot` is browsing/cwd data. `FolderWorkspace` is the explicit application
opt-in to workspace persistence. Opening without a directory seeds the default
root from the user's home, not launch cwd; no default-workspace `.bed` files are
read or written, including existing home metadata. Explicit folder choices are
canonicalized off-thread, reuse an existing folder workspace, and leave the
memory-only default distinct. Global appearance/preferences remain independent.

At that stage, folder `.bed/workspace2.json` recipes compared meaningful state after mutation and
save after 250 ms debounce, with bounded validated reads and atomic background
writes. Missing files use the current default composition without writing solely
because startup happened. Existing valid recipes restore concrete panels and
file descriptors after project policy resolves. Invalid data, failed writes and
late-load conflicts remain visible and do not silently overwrite old data.
Admitted pending snapshots outlive retired workspace entities. Settings exposes
retry; quit flushes metadata and waits for cleanup without asking to save a
workspace. Actual document edits/conflicts/drafts still require a decision.

The global native menu root works even without a workspace. macOS menus are
**bEd → Quit bEd**, **File → Open Folder…, Save**, and **Edit → Undo, Redo,
separator, Cut, Copy, Paste**, with actual standard accelerators. Text editing
routes through the focused widget or canonical document history; stale focus is
not retargeted. Application close state gates native actions and deferred folder
outcomes; startup-root resolution and admitted editing follow-ups finish before
the shutdown persistence barrier. Native View retains auxiliary panel choices,
without Code; Code opens via files. The titlebar no longer renders its own menus. Nonnative presentation uses the same menu tree.

Session recipes intentionally omit unsaved document data/history/drafts, editor
cursor/scroll/folds, tree expansion/selection, terminal processes/transcripts and
viewport orbit state. They do not implement crash recovery or Save As.


### Follow-up verification — 2026-10-19

- The seven-package serial test run with application `automation` enabled passed:
  **82 bEd2, 197 Workbench, 100 Filetree, 44 Terminal, 12 Automation, 27 UI and seven
  Clipboard unit tests** (469 total), plus one Workbench doctest. The final app-only
  rerun after child cwd and close-system ordering changes also passed 82 tests.
- Headless Workbench passed 138 tests and one doctest; `ui`-only and `native`-only
  feature checks passed. Editor with `workbench,treesitter` passed 41 tests, with
  its two explicit performance fixtures ignored.
- The automation build and `smoke_workspace_policy.py` passed. The policy scenario
  checked actual native menu installation including View, actual child-shell `pwd`
  at the isolated home, no default `.bed` metadata, independent global appearance
  saving, autosave before quit, unchanged idle/startup storage, multi-layout and
  real file-descriptor reconstruction, explicit-folder reuse and clean quit.
- Basic, Cube, Tasks, Terminal, Code/Structure and Editor desktop scenarios passed
  (9/8/6/15/8 captures; the editor processed a 112 MB fixture with viewport-bounded
  layout). Files **functional-only** passed, explicitly without rendering validation.
  Full Files attempts still produced transparent GPU captures; another attempt
  failed waiting for initial OS window focus. These are retained unresolved
  observations, not claimed fixed. The current smoke helper accounts for session
  restoration and excludes app metadata from read-only payload comparisons;
  visibility-file bytes/mtime remain separately asserted.
- Native OS picker clicking was not automated. Cancellation, accepted-result
  routing, native action dispatch and modal/shutdown gating have unit/application
  boundary coverage. Smoke View activation revalidates the same abstract menu
  action; native menu text is not part of Bevy's text inspection.
- Python compilation of the eight scripts and `git diff --check` passed.
