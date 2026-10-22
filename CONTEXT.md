# Project context

Last reviewed: 2026-10-22. This is the entry point for current project knowledge;
keep detailed contracts and decision history in the linked documents.

## Direction and code map

bEd is an IDE built on the reusable Bedtime desktop foundation. Bedtime 2 is
being developed alongside v1; `bed` remains Cargo's default application, so use
an explicit package when working on v2.

| Area | Entry point |
| --- | --- |
| v1 IDE, reusable crates, examples and test commands | [Root README](README.md) |
| v2 foundation and crate boundaries | [Bedtime 2](bedtime2/README.md) |
| v2 application: Files, code/Structure, viewports, tasks, terminal, settings and quit policy | [bEd 2](bed2/README.md), [main.rs](bed2/src/main.rs), [close.rs](bed2/src/close.rs), [files.rs](bed2/src/files.rs) |
| v2 drawing, themes, input, components and viewports | [UI2](bedtime2/crates/bedtime-ui2/README.md) |
| v2 virtualized text editor, buffer and optional document adapter | [Editor2](bedtime2/crates/bedtime-editor2/README.md) |
| v2 shared Tree-sitter analysis, bundled grammars and code Structure | [Analysis](bedtime2/crates/bedtime-treesitter2/README.md), [grammar bundle](bedtime2/crates/bedtime-grammars2/README.md), [Structure](bed2/crates/bed-structure2/README.md) |
| v2 workspaces, layouts, concrete panels, commands and documents | [Workbench2](bedtime2/crates/bedtime-workbench2/README.md) |
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
  components supply missing defaults and panel metadata/drawing immediately;
  lifecycle hooks maintain ownership and selection/focus fallback. Reconciliation
  validates the graph, not initialization. Concrete panel spawns select the
  destination tab without changing area focus; selection and focus are separate.
- The entity-first application-composition refactor removes panel type registration,
  the built-in type picker and core workspace/layout snapshots/restoration/templates
  and snapshot notifications. Applications own creation discovery and persistence.
  Command definitions and handlers are independent; bindings and handlers resolve
  panel → workspace → world. Document handlers are explicitly installed.
  Destructive UI intentions use `PanelCloseRequested` and application-approved
  `commit_close`; raw despawn cleans up but cannot be vetoed. See the
  [accepted design and phase checklist](MIGRATION/application-composition.md).
- Workspace-owned typed/text documents share canonical content, transactions,
  history and save/source checkpoints across panel viewers and layouts. Async
  saves and external observations are generation-gated; conflicts require
  explicit resolution. Controlled workspace close rejects pending writes/drafts;
  closing the last viewer retains its document. Checked cross-workspace relocation
  rejects drafts/owned work and detaches its document view; raw bound reparenting is
  unsupported. See the [document contract](MIGRATION/workbench-documents.md).
  Storage I/O, source ordering/CAS, merge algorithms and restore descriptors stay
  application-owned; bEd2 opens file-backed code viewers through Files, with
  canonical history and a source-checked save backend.

Read the current crate README and relevant implementation for the contract.
[Application composition](MIGRATION/application-composition.md) is the current
entity-first decision record and unfinished-work checklist.
[Workbench composition](MIGRATION/workbench-composition-plan.md) is historical;
its registry, initialization and core snapshot decisions are superseded by that
record.
[Migration overview](MIGRATION/overview.md) explains the broader goals, including
planned document/settings work. Its older input-routing description and
[stage 4](MIGRATION/step-4-egui.md) predate the current shortcut-consumption model.
Distinguish those plans and historical notes from implemented capabilities.

## Work status

bEd2 now owns centralized RON `.bed` metadata under the user config directory:
`preferences.bed` holds global per-value settings and five layout defaults;
`workspaces/<canonical-root-digest>.bed` combines each folder workspace's overrides,
visibility and structural session behind one writer. No project metadata is read
or written and no legacy JSON import runs. The default home workspace remains
memory-only. See the [application contract](bed2/README.md#settings-layouts-and-persistence).

Settings has General, Appearance and Editor sidebar sections, a Global/Workspace
switch and individual override reset icons. Keybinding configuration is deferred;
fixed Save/Open Folder/Quit and Primary+1–5 shortcuts remain. Document autosave
supports Never, Change Focus and Timer (default: one second idle), using canonical
checked saves and the existing quit gates. Numbered slots retain live entities;
View → Layout saves global tool/geometry defaults or resets the active layout
while preserving existing documents/workers. Initial layout composition waits
for workspace restore. Fresh startup creates one terminal layout. General has a
global, default-off **Open last workspace on open** option; it restores the last
active folder and its session unless an explicit folder argument takes precedence. Titlebar Terminal/Files/Structure/Settings buttons open or
focus on click and create via existing panel-factory drop handling on drag. The
standalone Theme editor is retired. Save As, New/Untitled, crash recovery and
broader workspace-management UI remain pending. Optional v2 automation is in
`bedtime2/crates/bedtime-automation2`, with clients in `bedtime2/tools`.

bEd2 owns native Bevy CRT effects in `effects.rs`/embedded WESL: Appearance has
an enable toggle (default off) and per-value persisted sliders, without presets,
jitter or chromatic aberration. Input uses the same curvature map before egui;
rendering runs after egui and before UI2's `PrepareOutput` platform alpha phase.
See [the application contract](bed2/README.md#settings-layouts-and-persistence)
for history/color/input ownership and `smoke_effects.py` verification. Bloom
uses half-width/height float textures and a separable blur; zero intensity
releases its buffers and bypasses all bloom passes. On 2026-10-22, 97 bEd2
tests and the opt-in native bloom GPU regression passed. Its synthetic M4 Pro
4K median bloom + CRT GPU time fell from 2.900 ms to 0.447 ms (6.49×); this is
not whole-app frame time or a thermal measurement. The native effects smoke
passed 11 captures, including zero-bloom transitions, odd-size resizing, curved
clicks/slider dragging, alpha and settings restoration. UI2's 27 tests passed
earlier the same day. Native Linux rendering was not exercised.

bEd2's `cube.rs` exercises main-world scene rendering with independent orbit
viewports, `tasks.rs` exercises workspace/panel-owned workers and process I/O,
and `close.rs` owns panel-close acceptance, quit decisions and managed worker
shutdown. Files/editor is restored as the document-opening flow; memory-only Documents and Hello World
remain removed. Settings remains available through application-owned UI.
Local release packages use `scripts/package-bed2-macos.sh` (`--install` stages
and replaces `/Applications/bEd2.app`) and `scripts/package-bed2-linux.sh`.
The distinct bundle/executable identities coexist with bed1; v2 runtime assets
are embedded and no remote helpers are built. See the bEd2 README's local
packaging contract for overrides and artifact paths. macOS release packaging,
signing and `--install` into an isolated test directory passed on 2026-10-21;
all 34 script tests passed, including Linux assembly fixtures and installation
rollback. Native Linux packaging has not been run here.
The optional `bedtime-terminal2` plugin adds local Unix PTY panels with egui
input/rendering and workbench-owned cleanup; no SSH, `+` commands or inline images.
It reuses the v1 headless core without installing its frontend or shell bridge.
The canvas uses the active UI2 theme's 16 ANSI colors (bed1 Everforest presets),
real PaperMono bold and a pixel-aligned grid. Tabs follow worker-observed foreground
process names. Theme edits preserve the session and shell OSC overrides; global
appearance now has application-owned storage, not terminal-session persistence.
See the application and terminal
READMEs for controls and verification limits.

`bedtime-filetree2` adds lazy watched filesystem panels with explicit per-panel
roots, viewport-only animated rows, file operations and root-scoped hiding.
Buffered activations capture panel/workspace/path/click type; the embedding
application owns routing, root choice and persistence. Git is deferred. macOS
uses directory-only kqueue (not notify's recursive-child kqueue behavior or its
FSEvents restart path). Reloads retain last-known rows; results outlive panel and
workspace retirement. Per-panel settings, direct file menus, native drop bridging
and root-scoped preferences remain library capabilities. bEd2 installs the tree
and explicitly chooses home for the default memory workspace, or an explicitly
opened folder for a persistent workspace root.
Its `files.rs` routes previews/pinned opens, background loads and visibility
storage; `file_editor.rs` binds shared documents and checked atomic saves. The
titlebar Files button exposes this flow. See the application/filetree contracts
for preview safety, source reconciliation, storage and platform limits.

`bedtime-editor2` implements the first plain editor slice: persistent indexed
UTF-8 text, viewport-only line/long-line layout, selection/navigation/clipboard,
shared history, IME drafts, line numbers and bed1 caret motion. Everyday editing
adds indentation-aware soft rows, smart whitespace, selection indent/outdent and
literal Find/Replace. Cancellable workers prepare wrapping/search/bulk edits;
revision checks gate acceptance. Optional Tree-sitter contributions add syntax
colors and per-view folds; bEd2's code-only Structure targets public revision-bound
editor navigation, including zero-based `scroll_to_line`. The grammar bundle
contains 30 languages plus Markdown inline, without per-language Cargo crates.
One managed analysis session serves a viewed document, with revision-checked
semantic actions during refresh. Editor presentation retains previous colors
mapped through known edits, and Structure retains its last same-source outline
until fresh analysis arrives; fold/Structure navigation still requires current
analysis.
See the analysis/Structure READMEs for limits and source
selection (automatic eligible focus after closure counts). Its Workbench
adapter uses canonical document transactions and a 128-commit change journal;
default builds and the standalone example require no Workbench or language tools.
The editor locks each wheel input to its dominant axis without a timed gesture
lock, retains revision-valid wrapping during resize, reserves fold-gutter space and animates folds with bed1 smoothstep timing. Files/Structure selections
and editor caret/active numbers share its RGB cycle; disclosure arrows share UI2
artwork. bEd2 editor actions live in the canvas context menu; the top control bar
is removed. Titlebar tools use theme text ink, animate hover backgrounds and retain tooltips.
The standalone and opt-in syntax examples provide two in-memory editor views.
bEd2's file-backed views detect the initial language, retain explicit choices,
and share analysis/structure through canonical documents. Opening an empty Code
panel creates no sample document; Code is no longer a View creation choice.
See the editor README for contracts, limits and later editing/tooling milestones.

## Verification recipes

For v2 library/application work, start with the affected package or test filter:

```sh
cargo test --offline --locked -p bedtime-ui2 -p bedtime-workbench2 -p bedtime-terminal2 -p bedtime-filetree2 -p bedtime-clipboard2 -p bed2
cargo check --offline --locked -p bedtime-workbench2 --no-default-features
cargo test --offline --locked -p bedtime-treesitter2 -p bed-structure2
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter
cargo test --offline --locked -p bedtime-editor2 --features workbench,treesitter million_line_ -- --ignored --nocapture
```

When changing feature boundaries, also check the relevant `ui` and `native`
combinations with default features disabled. For v1, use the focused commands and
broader suites documented in the [root README](README.md).

For UI changes in a desktop session, the gallery smoke covers widgets/rendering:

```sh
cargo run --offline --locked -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-egui-smoke
```

The v2 application automation smoke builds with its opt-in feature and drives the
real executable through the stdio client. Give each scenario a fresh, isolated
`XDG_CONFIG_HOME` under `target` so appearance/preference tests cannot modify your
personal configuration; the application now persists settings.

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
python3 bedtime2/tools/smoke_cube.py
python3 bedtime2/tools/smoke_tasks.py
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_code.py
python3 bedtime2/tools/smoke_editor.py
python3 bedtime2/tools/smoke_filetree.py
python3 bedtime2/tools/smoke_workspace_policy.py
```

Editor presentation verification on 2026-10-21: editor (45 tests with
Workbench/Tree-sitter; 33 standalone), Structure (7), UI2 (27), filetree (100)
and bEd2 (86) passed; the two editor performance tests remain opt-in. Native
Code/Structure smoke passed ten captures, including collapse and titlebar
hover/tooltip. Native editor smoke passed the 112 MB million-line fixture
(29 rendered lines, 616 laid-out bytes). Native filename fixtures required
running filetree tests outside the filesystem sandbox.

These are recipes, not a claim of a passing run on the current worktree.
Earlier workspace/menu verification (2026-10-19, including the unresolved Files
GPU-capture/focus observations) is recorded in
[application composition](MIGRATION/application-composition.md#follow-up-verification--2026-10-19).
Rerun relevant checks for a new code change. Offline commands require cached
dependencies. Documentation-only changes need link/content checks, not Rust builds.
