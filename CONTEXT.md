# Project context

Last reviewed: 2026-10-19. This is the entry point for current project knowledge;
keep detailed contracts and decision history in the linked documents.

## Direction and code map

bEd is an IDE built on the reusable Bedtime desktop foundation. Bedtime 2 is
being developed alongside v1; `bed` remains Cargo's default application, so use
an explicit package when working on v2.

| Area | Entry point |
| --- | --- |
| v1 IDE, reusable crates, examples and test commands | [Root README](README.md) |
| v2 foundation and crate boundaries | [Bedtime 2](bedtime2/README.md) |
| v2 application: viewports, tasks, documents, Files and Theme controls | [bEd 2](bed2/README.md), [main.rs](bed2/src/main.rs), [files.rs](bed2/src/files.rs), [documents.rs](bed2/src/documents.rs) |
| v2 drawing, themes, input, components and viewports | [UI2](bedtime2/crates/bedtime-ui2/README.md) |
| v2 virtualized text editor, buffer and optional document adapter | [Editor2](bedtime2/crates/bedtime-editor2/README.md) |
| v2 workspaces, layouts, panels, commands and snapshots | [Workbench2](bedtime2/crates/bedtime-workbench2/README.md) |
| v2 lazy local filetree, watching, operations and visibility | [Filetree2](bedtime2/crates/bedtime-filetree2/README.md) |
| v2 local terminal plugin and lifecycle | [Terminal2](bedtime2/crates/bedtime-terminal2/README.md) |
| Engineering guidance by task | [Engineering index](ENGINEERING.md) |

## Current implementation and decision records

- Applications own Bevy setup, windows, cameras and the renderer. UI2 remains
  independent of Workbench; Workbench's native and UI capabilities are optional.
- UI drawing uses ordinary sequential functions and borrowed component props.
  Application state belongs in ECS data; egui IDs hold presentation state.
  egui owns widget input, and Workbench consumes eligible shortcuts after drawing.
- Workspaces, layouts, areas and panels compose in the application's World.
  Active markers select workspaces/layouts; empty workbenches are valid. Required
  components supply local defaults, lifecycle hooks maintain membership, and
  reconciliation validates the graph and initializes panels.
- Workspace/layout snapshots validate external data before restoration.
  Applications own storage I/O, project data and layout-template policy. The
  current bEd2 application does not persist its Theme controls.
- Workspace-owned typed/text documents share canonical content, transactions,
  history and save/source checkpoints across panel viewers and layouts. Async
  saves and external observations are generation-gated; conflicts require
  explicit resolution. Controlled close rejects pending writes/drafts; closing
  the last viewer retains its document. See the [document contract](MIGRATION/workbench-documents.md).
  Storage I/O, source ordering/CAS, merge algorithms and restore descriptors stay
  application-owned; bEd2's Documents demo uses memory-only versioned sources.

Read the current crate README and relevant implementation for the contract.
[Workbench composition](MIGRATION/workbench-composition-plan.md) records accepted
decisions and later revisions; read the section relevant to the change, since
earlier entries can describe superseded behavior.
[Migration overview](MIGRATION/overview.md) explains the broader goals, including
planned document/settings work. Its older input-routing description and
[stage 4](MIGRATION/step-4-egui.md) predate the current shortcut-consumption model.
Distinguish those plans and historical notes from implemented capabilities.

## Work status

Recent work establishes component composition, workspace/layout snapshots,
plain UI rendering and the small bEd2 application. Optional v2 automation is in
`bedtime2/crates/bedtime-automation2`, with clients in `bedtime2/tools`.
bEd2's `cube.rs` exercises main-world scene rendering with independent orbit
viewports, `tasks.rs` exercises workspace/panel-owned workers and process I/O,
and `documents.rs` provides typed/text viewers and save/discard/cancel quit policy.
The optional `bedtime-terminal2` plugin adds local Unix PTY panels with egui
input/rendering and workbench-owned cleanup; no SSH, `+` commands or inline images.
It reuses the v1 headless core without installing its frontend or shell bridge.
The canvas uses the active UI2 theme's 16 ANSI colors (bed1 Everforest presets),
real PaperMono bold and a pixel-aligned grid. Tabs follow worker-observed foreground
process names. Theme edits preserve the session and shell OSC overrides; appearance
remains in memory. See the application and terminal
READMEs for controls and verification limits.

`bedtime-filetree2` adds lazy watched filesystem panels with explicit per-panel
roots, viewport-only animated rows, file operations and root-scoped hiding.
Buffered activations capture panel/workspace/path/click type; the application
owns routing and persistence. `bed2/files.rs` supplies CLI/cwd root policy,
editable previews and atomic `.bed/filetree2.json` preferences. Git is deferred.
macOS uses directory-only kqueue (not notify's recursive-child kqueue behavior or
its FSEvents restart path). Reloads retain last-known rows; results outlive panel
and workspace retirement, and ordinary snapshot disposal returns to work.
See the filetree contract for bounds, clipboard ownership and platform limits.
Review follow-up adds root collapse, folder-wide eligible drop regions, per-panel
`FileTreeSettings`, direct menus without operation shortcuts, and corrected
rename/move retirement. Native macOS drag coordinates are bridged by Workbench.
Bed's clean real-file viewers close/unload deleted sources; dirty/drafted/saving
documents retain edits and lose their backing save binding. Per-item
`PanelTabStyle` supports italic/dim previews. Files/preview/preferences failures
use acknowledged human-readable Workbench error dialogs with optional details.

`bedtime-editor2` implements the first plain editor slice: persistent indexed
UTF-8 text, viewport-only line/long-line layout, selection/navigation/clipboard,
shared history, IME drafts, line numbers and bed1 caret motion. Everyday editing
adds indentation-aware soft rows, smart whitespace, selection indent/outdent and
literal Find/Replace. Cancellable workers prepare wrapping/search/bulk edits;
revision checks gate acceptance. Folding waits for Tree-sitter integration. Its Workbench
adapter uses canonical document transactions and a 128-commit change journal;
default builds and the standalone example require no Workbench or language tools.
`bed2/file_editor.rs` owns path identity, worker source comparison and streaming
saves. Files tabs now edit shared text and expose Split view and Save. See the
editor README for contracts, limits and later editing/tooling milestones.

## Verification recipes

For v2 library/application work, start with the affected package or test filter:

```sh
cargo test --offline --locked -p bedtime-ui2 -p bedtime-workbench2 -p bedtime-terminal2 -p bedtime-filetree2 -p bedtime-clipboard2 -p bed2
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bedtime-editor2 --features workbench
cargo test --offline --locked -p bedtime-editor2 --features workbench million_line_ -- --ignored --nocapture
```

When changing feature boundaries, also check the relevant `ui` and `native`
combinations with default features disabled. For v1, use the focused commands and
broader suites documented in the [root README](README.md).

For UI changes in a desktop session, the gallery smoke covers widgets/rendering:

```sh
cargo run --offline --locked -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-egui-smoke
```

The v2 application automation smoke builds with its opt-in feature and drives the
real executable through the stdio client:

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
python3 bedtime2/tools/smoke_cube.py
python3 bedtime2/tools/smoke_tasks.py
python3 bedtime2/tools/smoke_documents.py
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_filetree.py --binary target/debug/bed2 --wide-entries 20000
python3 bedtime2/tools/smoke_editor.py
```

These are recipes, not a claim of a passing run on the current worktree.
Prior implementation verification is recorded in the composition document;
rerun the relevant checks for a new code change. Offline commands require cached
dependencies. Documentation-only changes need link/content checks, not Rust builds.
