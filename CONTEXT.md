# Project context

Last reviewed: 2026-10-26. Read this entry point, the affected crate README and
applicable `.skills/` engineering guides before changing implementation.

## Direction and code map

bEd is an IDE on the reusable Bedtime desktop foundation. Bedtime 2 develops
alongside v1; `bed` remains Cargo's default application. Select v2 packages explicitly.

| Area | Current contract |
| --- | --- |
| v1 application/crates/examples | [Root README](README.md) |
| v2 foundation and boundaries | [Bedtime 2](bedtime2/README.md) |
| bEd2 opening, settings, sessions, Git, tools and quit | [bEd2](bed2/README.md) |
| Drawing, themes, input, fonts, widgets and viewports | [UI2](bedtime2/crates/bedtime-ui2/README.md) |
| Layouts, panels, world services, commands, documents and optional persistence | [Workbench2](bedtime2/crates/bedtime-workbench2/README.md) |
| Source/model/history contracts, persistent text and bounded binary snapshots | [Document2](bedtime2/crates/bedtime-document2/README.md) |
| Native file sources, streaming writes and filesystem operations | [Filesystem2](bedtime2/crates/bedtime-filesystem2/README.md) |
| Byte canvas and complete read-only panel | [Hex2](bedtime2/crates/bedtime-hex2/README.md) |
| Files, watching, operations and visibility | [Filetree2](bedtime2/crates/bedtime-filetree2/README.md) |
| Local terminals, managed PTY lifecycle and shell helpers | [Terminal2](bedtime2/crates/bedtime-terminal2/README.md) |
| Virtualized editor, canonical document adapter and optional syntax/diff | [Editor2](bedtime2/crates/bedtime-editor2/README.md) |
| Analysis, bundled grammars and Structure | [Tree-sitter](bedtime2/crates/bedtime-treesitter2/README.md), [grammars](bedtime2/crates/bedtime-grammars2/README.md), [Structure](bed2/crates/bed-structure2/README.md) |
| Language servers and robustness decisions | [LSP2](bedtime2/crates/bedtime-lsp2/README.md), [investigation](MIGRATION/lsp2-investigation.md) |
| Models and mascots | [Model2](bedtime2/crates/bedtime-model2/README.md), [Mascots2](bedtime2/crates/bedtime-mascots2/README.md) |
| Engineering guidance | [Index](ENGINEERING.md) |

## Implemented ownership and persistence direction

The [world/session decision](MIGRATION/workbench-world-session-plan.md) supersedes
older workspace-scope and application-only serialization statements. Workbench now
owns **one editing session per Bevy World**, with no Workspace entity, membership,
activation/running markers, workspace facade or replacement mandatory Session entity.
Layouts/panels/documents retain identities; world resources own work, queues and
shell state. There may be no selected layout. Commands resolve panel → world.
Moving a viewer across layouts retains its canonical document and owned work.

The host owns session admission/replacement and orderly exit: complete admitted
writes/drafts, flush storage, preflight documents, cancel managed work, continue
updating until cleanup finishes, then exit. World drop is only fallback cleanup.
[Document history/source/save invariants](MIGRATION/workbench-documents.md) remain
in force; that record's old workspace ownership is superseded.

bEd2 attempts a different folder in a separate instance and immediately requests
normal quit, without waiting for replacement readiness; admission/launch failure
does not veto quit. **Independent sessions editing the same canonical project are
unsupported**, enforced by a lifetime OS lock before
restore/editing. Lock identity is independent of config directory overrides.
Same-root opening reuses the current session; invalid paths fail without quitting.
Normal save/discard/cancel, flush and cleanup gates remain authoritative.
Global preference writers lock/read/merge/atomically replace and periodically refresh
untouched values across instances. Project files have one admitted writer.

See the [opening decision](MIGRATION/workbench-world-session-plan.md#launch-and-quit)
for replacement semantics and verification.

Optional Workbench `persistence` uses Bevy 0.20 DynamicWorld and an explicit reflected
marker selected by `Workbench::capture_world::<M>`, with editable detached captures,
bounded validation, reference closure/remapping and ordered structural restore.
Workbench components and existing ordered relationship collections capture directly.
`register_persisted_relationship::<R>` shares typed membership/order/payload restoration
with application pairs; markers remain the allowlist. Generic checks reject conflicting
membership/cardinality, disallowed self-links and linked ownership cycles before commit.
Encode/load/restore validate edits before committing. Required defaults rebuild
runtime; validation does not insert components. Trusted Serde/FromReflect remain pure;
hooks preserve restored membership and external activation follows graph restoration.
Hosts permanently seal world restoration after startup loading. Scoped reflected layout
capture/replacement supports runtime defaults/Reset without replacing session resources;
replacement recreates panel identities and follows ordinary panel cleanup. No panel
factory catalog exists.
See the [library contract](bedtime2/crates/bedtime-workbench2/README.md#optional-reflected-persistence).

bEd2 uses application schema 2 global/project envelopes and library snapshot schema 2.
Previous library schema-1 snapshots are rejected and preserved. Independent reflected
global/project markers route resources; feature descriptors are added to detached
captures without temporary live-world insertion. Checked file reopening
recreates shared canonical documents. Legacy global value/tool-recipe defaults migrate
to reflected layout snapshots on an actual edit, without startup rewriting. Old project
sessions/unknown types are rejected and preserved with visible errors. Storage retains bounded reads, readiness/late-load gates, debounce/retry,
atomic writes and quit flushing. Global layout defaults exclude document viewers,
Git comparisons and project paths.
Global policy readiness latches after successful initial loading; later save/refresh
failures and retries preserve editing/opening/LSP admission. Storage health still
owns errors/retry/quit flush. Persisted source/startup paths use shared lossless
native-unit records with legacy UTF-8 reads; startup does not rewrite old fields.
See [storage compatibility](bed2/README.md#settings-layouts-and-persistence).
Feature plugins own creation commands/View contributions; live reconstruction has no
ToolKind/constructor catalog. Reset is destructive to panels and retains canonical
documents, other layouts and session services.
[Compatibility and routing](bed2/README.md#settings-layouts-and-persistence) are
application policy. Unsaved-content/history recovery, Save As and New/Untitled remain
separate work; reopening file descriptors is not recovery.

## Source-backed document migration

The [source-backed document decision](MIGRATION/source-backed-documents.md) supersedes
application-owned source identity/loading/checkpoints/save bindings/observation in
older records. Providers intern sources; Workbench owns one typed document per source
per World, including loading/failed states. TextDocument and read-only BinaryDocument
share the headless document core/history; the editor's standalone owner is now
MemoryTextSession. Hex reads current document bytes, including unsaved text. Viewer
preparation and filesystem source lifecycle live in Bedtime; bed2 selects viewers,
placement, settings and application storage. Binary editing and paged text editing
remain deferred. Existing session schemas/viewer IDs/native path records are retained.

## Other boundaries worth preserving

- Applications own Bevy setup/windows/cameras/rendering. UI2 is independent of
  Workbench; Workbench UI/native/persistence features are optional. Concrete panel
  composition needs no registry. Hooks maintain relationships and selection/focus;
  reconciliation validates, not initializes. Destructive UI requests use checked
  `commit_close`; raw despawn cannot prompt or veto.
- Canonical documents own content, revision/history and save/source checkpoints.
  Independent viewers share transactions and analysis. Source observations, save
  receipts and navigation results retain generation/revision checks. Closing the
  last viewer retains its document. Widget-private undo must not replace canonical
  history. Workbench admits source I/O and owns generation checks; hosts choose
  merging/recovery policy. Filesystem2 supplies canonical sources and checked
  streaming replacement.
- Filesystem2 owns headless local reads, version-checked ranges, staged exact-baseline
  writes, directory operations and native watching. Text/image/model/Hex access uses
  it; application metadata stays local. Hex2 owns the complete optional Workbench
  panel; Workbench owns shared bounded document byte caches. No SSH implementation or binary editing was added.
- Filetree owns one world filesystem worker with independent panel roots and
  generation validation. Native paths retain their original units; visibility's
  validated path record is shared with app descriptors/startup identity. Retained managed
  completions own outcome delivery through orderly drain; Filetree keeps only domain
  IDs/origins/reconciliation. Operation outcomes survive panel/backend close.
- LSP uses lazy local stdio processes shared by launch identity/root. Hidden views
  continue syncing; observer closure preserves workers. Stop suppresses relaunch
  until Restart/config change. Real-server latency is distinct from deterministic
  protocol tests; native Linux behavior remains unverified here.
- bEd2's Cube/3D Viewport and Tasks & Processes demos and their automation smokes
  are removed. Persisted layouts/sessions containing their types follow unknown-type
  rejection; legacy demo tool defaults are rejected rather than substituted.
- Models use local bounded sidecar loading, canonical textual glTF and per-tab
  camera/appearance descriptors. Remote/absolute URI resources are rejected;
  relative parent paths are permitted. Model animation is deferred. Hidden model/
  mascot/cube views suspend cameras. Mascot audible output remains unverified.
- Application Settings keeps Global/Workspace user vocabulary, per-value resets,
  asynchronous font discovery and defaults. Keybinding declarations support inherit/
  override/unbind; the binding editor remains deferred. Autosave uses checked saves.
  Package scripts and platform-specific limitations are documented in bEd2's README.

## Verification recipes and evidence

Use cached dependencies with `--offline --locked`; choose affected packages first.
The 2026-10-26 source-backed migration passed Document2 (5), Editor2 (53),
Filesystem2 with Workbench (49), Filetree2 (68), Hex2 (9), Image2 (12), Model2 (63),
LSP2 (30 across unit/backend/integration), Workbench with persistence (197),
and bEd2 (157) tests, plus doctests. Existing performance/real-server/application
opt-ins remain ignored. Workbench headless persistence passed 137 tests; minimal
Workbench, filesystem integration, plain Hex and standalone editor checks passed.
The persistence example passed three tests and its headless round trip. Normal
application checking and the automation build passed.
Native macOS flows passed Code's 112 MB file, Hex's unsaved-text/history projection
(eight captures), Filetree operations/10,000 entries (eleven), images, models,
Git (eleven), session restoration (five), autosave/storage/quit and file dragging.
Artifacts are under `target/source-documents-*`. Git's confirmation smoke now
selects the visible floating button rather than its obscured inline duplicate;
image/model/editor fixtures use current layout/settings helpers. One Git restored
quit attempt timed out; a rerun passed, and quit-timeout diagnostics now preserve
the actual UI/source gate if it recurs. Native Linux and real-server opt-ins were
not run. Earlier results below describe prior revisions.

The 2026-10-24 filesystem/Hex extraction passed the focused application, filesystem,
Filetree and Hex suites (157/44/67/9 tests; two existing application opt-ins ignored),
the automation build, and independent Hex workbench/plain-feature checks.
The native verification repair also passed UI2/Automation suites (30/12 tests),
the application/Filetree suites (157/67; two application opt-ins ignored), strict
Hex/Filetree flows (eight/eleven captures, including 10,000 Filetree entries), and
the colored/transparent screenshot regression without a drawable. The
[renderer patch](vendor/bevy_render/BED_PATCHES.md) fixes readback on frames without
a swapchain drawable; Filetree's smoke waits for paste's asynchronous row reveal
instead of toggling the destination folder. See [filesystem verification](bedtime2/crates/bedtime-filesystem2/README.md#verification)
for artifacts and platform limits.
The world/session record tracks this migration's verified results and limitations.
The 2026-10-24 policy/native-path/completion revision passed Workbench UI/headless
suites (192/132 tests plus doctests), Filetree (102 tests), bEd2 (161 passed, two
opt-ins ignored), the minimal-feature check/automation build, native macOS storage/
autosave/Reset and eleven-capture Filetree flows. Native path records/metadata
restart are exercised unconditionally; APFS rejects non-UTF-8 names, so the full
source-file restart case runs only where that filesystem capability is present.
Native artifacts: `target/policy-native-path-smoke` and
`target/managed-completion-filetree-smoke`. Filetree's smoke now isolates config
and reflects current Hex binary/large-file behavior.
The 2026-10-24 scoped-layout revision passed Workbench UI/headless suites (188/128
tests plus doctests), bEd2 (155 passed, 2 opt-ins ignored), minimal-feature checks,
example tests/headless round trip, and native macOS storage-policy/destructive Reset
and example seeded/startup-load/reload smokes. See the world/session record for
contracts and artifacts. Earlier session/helper/task/terminal and feature-smoke
results are historical evidence, not claims about every subsequent change.
The launch/quit revision passed bEd2 (157 tests, two opt-ins ignored), the normal
check/automation build and native session/helper smokes; see the opening decision
above for failure semantics and platform limits.

```sh
cargo test --offline --locked -p bedtime-workbench2 --features persistence
cargo test --offline --locked -p bedtime-workbench2 --no-default-features --features persistence
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bed2 --features automation
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter
cargo test --offline --locked -p bedtime-lsp2 --features workbench
cargo test --offline --locked -p bedtime-filetree2 -p bedtime-terminal2
cargo test --offline --locked -p bedtime-filesystem2
cargo test --offline --locked -p bedtime-hex2 --features workbench,automation
cargo test --offline --locked -p bed2 --example persist_world
cargo run --offline --locked -p bed2 --example persist_world -- --roundtrip
cargo build --offline --locked -p bed2 --features automation
cargo run --offline --locked -p bedtime-automation2 --example screenshot_without_drawable
python3 -B bedtime2/tools/smoke_hex.py
python3 -B bedtime2/tools/smoke_filetree.py --wide-entries 10000
python3 -B bedtime2/tools/smoke_session.py
python3 -B bedtime2/tools/smoke_workspace_policy.py
```

For native feature checks, use the focused `bedtime2/tools/smoke_*.py` route and
an isolated XDG_CONFIG_HOME under target. Native GUI commands require desktop
access; non-UTF8 Filetree fixtures and local socket tests may need to run outside
the filesystem sandbox. Real-server/performance/GPU probes remain explicit opt-ins.
Do not run every ignored test as a generic check. V1 recipes stay in the root README.

[Application composition](MIGRATION/application-composition.md) is historical for
workspace scopes/snapshots, current for entity-first panels and checked close.
[Workbench composition](MIGRATION/workbench-composition-plan.md) and older migration
stages are historical where they disagree with the current decision/library README.
