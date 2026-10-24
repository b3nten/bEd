# Project context

Last reviewed: 2026-10-24. Read this entry point, the affected crate README and
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

bEd2 opens a different folder in a separate instance. **Independent sessions editing
the same canonical project are unsupported**, enforced by a lifetime OS lock before
restore/editing. Lock identity is independent of config directory overrides.
Same-root opening reuses the current session; errors preserve it and unsaved work.
Global preference writers lock/read/merge/atomically replace and periodically refresh
untouched values across instances. Project files have one admitted writer.

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
Feature plugins own creation commands/View contributions; live reconstruction has no
ToolKind/constructor catalog. Reset is destructive to panels and retains canonical
documents, other layouts and session services.
[Compatibility and routing](bed2/README.md#settings-layouts-and-persistence) are
application policy. Unsaved-content/history recovery, Save As and New/Untitled remain
separate work; reopening file descriptors is not recovery.

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
  history. External I/O, CAS, merging and recovery belong to the host.
- Filetree owns one world filesystem worker with independent panel roots and
  generation validation. Native paths retain their original units; visibility's
  validated codec is separate from storage. Operation outcomes survive panel close.
- LSP uses lazy local stdio processes shared by launch identity/root. Hidden views
  continue syncing; observer closure preserves workers. Stop suppresses relaunch
  until Restart/config change. Real-server latency is distinct from deterministic
  protocol tests; native Linux behavior remains unverified here.
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
The world/session record tracks this migration's verified results and limitations.
The 2026-10-24 scoped-layout revision passed Workbench UI/headless suites (188/128
tests plus doctests), bEd2 (155 passed, 2 opt-ins ignored), minimal-feature checks,
example tests/headless round trip, and native macOS storage-policy/destructive Reset
and example seeded/startup-load/reload smokes. See the world/session record for
contracts and artifacts. Earlier session/helper/task/terminal and feature-smoke
results are historical evidence, not claims about every subsequent change.

```sh
cargo test --offline --locked -p bedtime-workbench2 --features persistence
cargo test --offline --locked -p bedtime-workbench2 --no-default-features --features persistence
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bed2 --features automation
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter
cargo test --offline --locked -p bedtime-lsp2 --features workbench
cargo test --offline --locked -p bedtime-filetree2 -p bedtime-terminal2
cargo test --offline --locked -p bed2 --example persist_world
cargo run --offline --locked -p bed2 --example persist_world -- --roundtrip
cargo build --offline --locked -p bed2 --features automation
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
