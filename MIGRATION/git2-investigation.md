# Git and comparison viewers for Bedtime 2

Status: investigation and proposed implementation sequence, 2026-10-22.
Git is not installed in bEd2. This proposal starts with the Git panel and text
diffs, with image and glTF comparisons as later extensions.

Agreed direction: inline text diff presentation belongs inside Editor2 and is
embedded in a diff panel through the ordinary editor drawing API. The narrow
Git-independent integration below is proposed; direct Git coupling behind a
feature is acceptable if implementation exposes a concrete need for it.

Reuse `bedtime-git` for repository operations. Build a UI2 Git panel and text
comparison presentation against v2 documents. Keep comparison rendering separate
from the authority to stage, discard or replace content. Ship concrete viewers;
introduce a shared comparison API as additional viewers need it, without adding
a Workbench viewer registry.

## Existing implementation and reuse

| Area | Current implementation | Implication for v2 |
| --- | --- | --- |
| Repository operations | [`bedtime-git`](../bedtime/crates/bedtime-git/src/lib.rs) depends on serde and Unix libc, with no Bevy, UI or document dependency. | Reuse the existing crate directly. No backend fork or GUI migration is necessary. |
| Git panel | [`panels.rs`](../bedtime/crates/bedtime-plugin-git/src/panels.rs) supplies branch controls, fetch/pull/push, refresh, conflicts, tracked/untracked lists, partial staging checkboxes, commit message and cancellation. | Reimplement the UI in egui. Use its behavior as a reference; its ImGui widgets do not port directly. |
| Execution | [`controller.rs`](../bedtime/crates/bedtime-plugin-git/src/controller.rs) coordinates reads, writes, saves, refresh, comparison identities and errors. [`jobs.rs`](../bedtime/crates/bedtime-plugin-git/src/jobs.rs) selects local or SSH workers. | Replace v1 document, remote-service and feature-registry coupling with workspace-owned state and managed work. Start local. |
| Text diff | [`diff_feature.rs`](../bedtime/crates/bedtime-plugin-git/src/diff_feature.rs) binds a normal editor to either a snapshot or the canonical working document. [`diff.rs`](../bedtime/crates/bedtime-editor-widgets/src/diff.rs) projects historical rows and hunk controls over real text. | Preserve this ownership model. Editable inline parity requires Editor2 layout/input work, beyond a new panel. |
| Non-text diff | bed1 displays a binary/non-text summary with whole-file actions. | Image and model comparisons add new capabilities. |
| Ordinary editor Git gutter | v1 [`DocumentGitChanges`](../bedtime/crates/bedtime-documents/src/git/git_service.rs) maintains a separate HEAD baseline. | Defer file-tree badges and ordinary editor gutters; the requested panel/diff does not require them. |

The backend already supplies status, branch/upstream/ahead/behind information,
raw before/after bytes, zero-context hunks and a comparison snapshot token.
It implements whole-file and hunk/range stage, unstage and discard, commits,
branches, network operations and conflict operations through the installed Git
executable. Git retains responsibility for hooks, signing and configuration.

Important constraints in
[`repository.rs`](../bedtime/crates/bedtime-git/src/repository.rs) and
[`process.rs`](../bedtime/crates/bedtime-git/src/process.rs):

- Repository-root scope is required for writes. Opening a subdirectory discovers
  its parent repository but filters status and remains read-only. bEd2 should
  offer an explicit open-root action rather than silently expanding a workspace.
- Paths are literal, repository-relative UTF-8 strings. Non-UTF-8 names fail;
  do not silently convert them into a different filename.
- Hunk/range actions and discard re-read the diff and reject a changed snapshot.
  Whole-file stage and unstage do not carry that snapshot precondition.
  These checks do not make a sequence of external Git commands an atomic
  transaction against other processes.
- `binary` also covers symlinks and gitlinks. A future image/model dispatcher
  needs explicit file-kind information before treating a non-text result as an
  ordinary asset. Empty bytes alone cannot distinguish absence from an empty
  file; additions/deletions need explicit presence information in a shared API.
- Hunk actions are limited to supported regular text changes. Renames,
  untracked files, deletions, conflicts and relevant mode changes use whole-file
  actions. `can_apply_hunks` is authoritative; a displayed text diff alone does
  not establish that it is safe to apply a range.
- File reads and process output have 16 MiB limits. Binary patch output can hit
  the output limit independently of either blob. Large image/model comparisons
  will need a bounded blob-reading path that does not generate a binary patch.
- `Job` supports cancellation, but its drop waits only briefly before potentially
  detaching the worker. Workspace lifecycle integration must explicitly forward
  cancellation and drain completion. Exceptional detached-hook cleanup also
  needs review before promising stronger shutdown guarantees.

## Proposed ownership and application integration

Keep repository status, pending operations, comparison cache and commit draft
on a workspace-owned Git session. Panels own selection, scrolling, confirmation
UI and references to comparisons. Closing a panel must not interrupt a commit.
Multiple Git/diff panels in different layouts share repository knowledge.
The backend's common-directory lock already rejects overlapping in-process
writes across linked worktrees; it does not supply scheduling or shared UI state.

Use Workbench managed tasks for operations. An initial adapter can own a
`bedtime_git::Job` inside a task, forward cancellation, and poll until a terminal
result before returning. Calling synchronous `execute` inside a task alone
would not connect Workbench cancellation to Git. Workbench suppresses task
values after cancellation, so cancellation must also clear the controller's
pending operation and require a refresh; it cannot await a success receipt.
Refresh after success, failure or cancellation because an operation may already
have had effects. Do not infer rollback from cancellation.

Application work belongs alongside the existing integration points:

- [`files.rs`](../bed2/src/files.rs) and
  [`file_editor.rs`](../bed2/src/file_editor.rs): expose a narrow way to find or
  bind a canonical file document. File identity is private application state;
  neither panel titles nor save destination keys are source identity.
- [`opening.rs`](../bed2/src/opening.rs): route **Open file** through the existing
  path. Comparisons need their own identity, including repository, path and
  staged/unstaged side; path equality must not reuse an ordinary editor as a diff.
- [`menus.rs`](../bed2/src/menus.rs), titlebar and
  [`layouts.rs`](../bed2/src/layouts.rs): concrete Git panel creation and placement.
- [`session.rs`](../bed2/src/session.rs): persist Git tool placement and validated
  comparison descriptors, then refresh their content on restore. Never persist
  live entities or restore a snapshot token as permission to mutate Git.
- [`close.rs`](../bed2/src/close.rs): drain managed repository work through the
  existing workspace/quit lifecycle. Decide whether commit drafts survive
  workspace restart; a runtime draft can be shared without adding persistence.

The implementation has three natural boundaries: the existing `bedtime-git`
backend, Git-independent text comparison in `bedtime-editor2`, and a new v2
Git session/panel with a bEd2 adapter for roots, file documents and saves.
An optional `bedtime-git2` crate can own the session/panel if that boundary stays
small; begin with concrete integration rather than designing host callbacks
before the file/document transitions are working.

Polling with manual refresh is enough initially. Later, debounce filesystem/save
observations and retain occasional polling for external index/ref changes.
Refreshing must work without an open Files panel and across inactive layouts.

## Text comparison semantics

| Comparison | Before | After | Editing |
| --- | --- | --- | --- |
| Staged | HEAD blob | Index blob | Both snapshots read-only |
| Unstaged on disk | Index blob | Captured working-file bytes | Read-only intermediate milestone |
| Editable unstaged | Index blob | Canonical live document | Working side shares edits/history with ordinary Code panels |

The intermediate disk comparison should say when an open document has unsaved
edits that it does not include. A read-only view is a smaller milestone, not
equivalent to bed1's editable working-document comparison.

For editable comparisons, keep three distinct facts: the Git comparison
baseline, the file save checkpoint and the canonical document revision. An index
blob is not the save checkpoint. A dirty buffer can differ from both the index
and disk even when Git reports no on-disk change.

Stage from the live view by capturing the document revision and comparison
identity, saving through the checked file backend, checking the acknowledged
revision/source identity, refreshing Git, and verifying that the displayed
baseline/ranges still match before applying. Recheck immediately before dispatch
if another operation delayed it. Refuse destructive disk changes while affected
documents have dirty content, drafts, saves or unresolved conflicts.
Coordinate autosave so a pending save cannot race a discard. Feed successful
disk changes back through the existing generation-checked source observation
route, preserving shared documents and undo history.

Editor2 currently maps text bytes through wrapping and folding in
[`layout.rs`](../bedtime2/crates/bedtime-editor2/src/layout.rs).
Inline diffs additionally need historical rows, two sets of line numbers,
change controls, navigation and consistent hit testing. Historical rows must
never enter canonical text, copy or undo. Extend that geometry coherently;
painting a few extra red/green rows would leave input and navigation incorrect.
Reuse v1 regression cases, while preserving v2's indexed buffers and bounded
viewport work rather than copying its complete row vectors and local LCS work
onto the UI thread. Compute comparisons off-thread and gate results by baseline
identity and document revision. Plain Editor2 must continue to work without Git.

### Built-in editor support without a Git dependency

Add an optional `diff` feature to `bedtime-editor2`, independent of `workbench`
and `treesitter`. Its concrete presentation API needs a baseline text snapshot,
a comparison identity and revision-bound change ranges. The host can supply
authoritative ranges from Git; the editor's background comparison work handles
subsequent live edits. These are ordinary text concepts and need no repository
path, branch, index or Git operation in their representation.

Editor2 owns the complete inline presentation: historical rows, change colors,
old/new line numbers, navigation, hit testing and hunk-action placement. A diff
panel binds its working side to the existing canonical document and draws the
same embedded editor. Git translates its hunks into editor ranges and retains
its snapshot token for mutation checks.

For inline buttons, the host supplies small action descriptions with an ID,
label and enabled state. The editor returns the chosen ID, comparison identity,
document revision and old/new ranges through its drawing output. Git supplies
Stage/Unstage/Discard actions and interprets those requests; the editor simply
draws the supplied buttons. No provider registry, arbitrary rendering callback
or trait hierarchy is required. Exact public types remain implementation work.

This also accommodates the existing Workbench external-source conflict data:
an application can compare incoming text with its live document and offer
accept/reject actions through the same presentation, then use document
transactions and explicit conflict resolution. It is a second concrete use for
text comparison without repository semantics. A three-way merge UI remains
separate work.

The small conversion from Git hunks to editor ranges is the expected integration
cost. Begin with this boundary; a `git` feature is unnecessary unless actual
editor behavior requires Git knowledge that cannot stay in the caller.

## Comparison API for other viewers

Own the initial text viewer and Git workflow. Leave a narrow boundary that can
also serve an image viewer, model viewer or another source of changes:

1. **Inputs:** labeled before/after versions with source identity, explicit
   presence, content kind and immutable bytes or typed snapshots. A live document
   adds its revision. Keep Git snapshot tokens in the Git adapter.
2. **Presentation:** the concrete viewer computes and draws changes. Text can
   identify line ranges; models can identify properties or nodes. No universal
   hunk representation is necessary.
3. **Requests:** a viewer returns a change selection and the comparison/revision
   it observed. The owner validates and executes an available action. In the
   initial implementation these can be direct typed calls and returned values;
   no global provider registry or callback framework is needed.

Two meanings of accepting changes need separate execution owners:

- **Change repository state:** stage copies working content into the index;
  unstage restores the index from HEAD; discard restores working content from
  the index. Git owns these operations and their preconditions.
- **Change a document:** accepting some source content submits one ordinary
  revision-checked, undoable document transaction. Saving remains a separate
  operation. A retained external-source conflict must also use Workbench's
  [explicit resolution contract](workbench-documents.md#external-source-observation);
  an arbitrary edit does not acknowledge the incoming checkpoint.

A generic comparison viewer need not understand Git to render two versions or
request a document edit. Conversely, a renderer must not receive unrestricted
authority to write arbitrary source bytes. Introduce shared public types when
the second viewer or non-Git source establishes which semantics are common.

## Images and glTF

**Images:** start with two decoded snapshots, synchronized pan/zoom and either
side-by-side or wipe presentation. Overlay/pixel difference can follow with
explicit dimension, alpha and color handling. Whole-file stage/unstage/discard
can use the existing Git workflow without inventing pixel patches. Partial image
acceptance requires image-editing and encoding semantics beyond comparison.
bed1's image decoder accepts bytes, but lives in the ImGui plugin; reuse its
bounded decoding implementation without importing that plugin wholesale.

**glTF:** start with self-contained GLB or embedded-resource glTF rendered with
the same camera, lighting and animation time. UI2's
[viewport support](../bedtime2/crates/bedtime-ui2/README.md#icons-and-viewports)
and bEd2's cube example provide the rendering foundation; there is no v2 model
viewer yet. bed1's
[`prepare.rs`](../bedtime/crates/bedtime-plugin-model-viewer/src/prepare.rs)
already rejects external resource URIs and prepares self-contained assets.

General multi-file glTF requires a version-scoped resource resolver: an old
`.gltf` must resolve its old `.bin` and textures, rather than today's filesystem
versions. Git needs additional tree/index/blob access for this; `Operation::Diff`
alone is insufficient. Introduce that resolver when supporting external assets.
Visual comparison and whole-file actions can precede semantic node/material
diffs. Selectively accepting model changes additionally needs stable matching,
reference repair, validation and writing of all affected resources. Array indexes
or names alone are not reliable model identity.

## Suggested implementation sequence

| Stage | Deliverable | Relative effort |
| --- | --- | --- |
| 1 | Local repository session, Git panel status/refresh, staged and unstaged read-only text comparisons, open-file action, binary summary, lifecycle and session integration. | Moderate; backend reuse removes most repository logic. A unified read-only diff is a useful intermediate target. |
| 2 | Whole-file stage/unstage, commit and confirmed discard, with canonical save/reconciliation and autosave gates. | Moderate; document/source coordination is the consequential part. Together with stage 1 this is the suggested first usable Git workflow. |
| 3 | Editable inline text comparison, previous/next change and stage/unstage/discard ranges. | Largest core change; Editor2 geometry and save/refresh/revalidate behavior need focused coverage. Required for bed1 editable-diff parity. |
| 4 | Branch/network controls, richer conflict UI, file-tree badges and ordinary editor gutters as selected follow-ups. | Separate increments; a three-way conflict viewer also needs base/current/incoming inputs beyond today's two-version Diff output. |
| 5 | Image comparison, then self-contained model comparison. | Images provide a smaller second consumer before committing to public comparison APIs; model loading and rendering add substantially more work. |

These are scope comparisons, not elapsed-time estimates. If editable diff parity
is required in the first delivery, stages 1–3 belong in that scope.

## Verification

On 2026-10-22 all 19 existing
[`git/regressions.rs`](../bedtime/tests/integration/git/regressions.rs) tests
passed against `bedtime-git` in a temporary Cargo harness with only that backend
dependency, using offline cached dependencies. This exercises real Git in
temporary repositories, including stale actions, literal paths, CRLF/BOM,
renames, conflicts, linked worktrees, cancellation and a local remote.
It establishes independent backend reuse; no v2 Git UI was implemented or tested.

The normal repository entry point for those tests is:

```sh
cargo bedtime-test git::regressions
```

Implementation coverage should protect shared canonical edits/undo, immutable
staged snapshots, partial staging with both sides present, missing versus empty
files, unsupported kinds, stale worker results, save failure/edits after save,
autosave/discard exclusion, retained external conflicts, panel/workspace closure
during hooks and cache refresh across layouts. Native automation should exercise
real panel interactions and bounded diff rendering. Keep the standalone editor
and affected Workbench feature combinations building independently of Git.
