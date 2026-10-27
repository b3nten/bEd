# Project context

Last updated: 2026-10-27. Keep this file short and current. Update the step log
and open questions as work proceeds; delete anything that turns out to be wrong.

## Goal

Rebuild **bEd** (the app in `bed/`) from scratch, using `bed_old/` only as a
reference, one feature at a time. After each step we review the code together
and decide whether Bedtime/Workbench should change so the integration is cleaner.

End state: `bed` is small and easy to follow. Complexity that any desktop app
would need lives in `bedtime`, while `bedtime` stays flexible enough for apps
other than an IDE.

## Responsibilities

| Layer | Owns | Does not own |
| --- | --- | --- |
| **Workbench** (`bedtime-workbench`) | Panel layout (layouts, areas, tabs), documents (load/save/history/conflicts), commands, menus, managed background work, shutdown | Which panel opens a file, where data is stored, app-specific UI |
| **Bedtime plugins** (`bedtime-*`) | Optional features, usually by contributing panels (file tree, editor, terminal, hex, image, model, LSP, git...) | Policy: cwd, viewer choice, storage paths |
| **Application** (`bed`) | Policy and coordination: what to open and in which panel, project root, where/when to serialize, defaults, keybindings, branding | Reusable mechanisms |

A test for where code belongs: *would a different desktop app built on Bedtime
need to write this again?* If yes, it probably belongs in Bedtime.

## Repository map

- `bed/` — the new app: window, workbench, settings (`settings.rs`), workspaces
  (`workspace.rs`), panel opening policy (`panels.rs`). Run `bed [folder]`.
- `bed_old/` — the previous app, kept as reference only. Not in the workspace.
- `bedtime/crates/` — the maintained library: `bedtime-workbench`, `bedtime-ui`,
  `bedtime-document`, `bedtime-clipboard`.
- `bedtime_old/` — the other plugins (editor, filesystem, filetree, terminal, hex,
  image, model, LSP, git, tree-sitter, grammars, mascots, automation) plus the old
  smoke-test tools and fixtures. Reference only, not in the workspace, and they
  won't build against the new workbench. Each plugin is brought back into
  `bedtime/crates/` when bed needs it, cleaned up against the new workbench APIs.
- `vendor/` — patched `bevy_render`, `egui`, `rio-vt`, `arboard` (see root `Cargo.toml`;
  the `rio-vt` patch is commented out until the terminal returns).
- `references/zed/` — local Zed checkout for reference; gitignored.

The old guidance docs (AGENTS.md, ENGINEERING.md, `.skills/`, `MIGRATION/`) were
removed in commit `0e763c8`; read them with `git show c804b15:<path>` if useful.
The engineering principles in the old AGENTS.md are still a good baseline
("minimize conceptual complexity, not code size"; clear ownership; visible
execution; no abstraction just to organize code).

## Verification

```sh
cargo check -p bed
cargo run -p bed
cargo test -p bedtime-workbench --all-features   # or whichever crate changed
```

Use `--all-features`: many integration tests (editor, hex, LSP workbench) are
behind features and silently don't run otherwise.

## bed_old feature inventory

Approximate line counts, to show where the weight was.

| Feature | bed_old modules | Lines |
| --- | --- | --- |
| Files: project root, file tab, opening policy, viewer switching | `files.rs`, `opening.rs`, `file_editor.rs`, `images.rs`, `models.rs` | ~4,900 |
| Git panel + diff view | `git.rs`, `git/*` | ~4,800 |
| Persistence: project session, global prefs, layout slots | `session.rs`, `storage.rs`, `persistence.rs`, `layouts.rs`, `project.rs` | ~2,700 |
| Settings panel + applying prefs to plugins | `settings.rs`, `fonts.rs`, `keybindings.rs` | ~1,700 |
| Menus, toolbar, panel registry | `menus.rs`, `main.rs` | ~1,400 |
| LSP integration + diagnostics panel | `lsp.rs`, `lsp/*`, `lsp_panel.rs` | ~1,600 |
| Quit / close with unsaved docs | `close.rs` | ~870 |
| Status bar | `status_bar.rs` | ~600 |
| Terminal shell commands (`bed open` from the shell, duck) | `terminal.rs` | ~530 |
| Post-processing effects | `effects.rs` | ~530 |
| Autosave | `autosave.rs` | ~340 |
| Structure (tree-sitter outline) | `code.rs`, `crates/bed-structure` | ~830 |
| Mascots | `mascots.rs` | ~70 |

## Initial architecture observations

From a first read on 2026-10-26. These are leads to check during the relevant
step, not decisions.

1. **`FilePanel` is a catch-all panel.** `files.rs` (4.1k lines) has one panel type
   that switches between editor, image, hex, model and metadata views. Bedtime
   already has `ImagePanel`, `HexPanel` and `ModelPanel`, but no editor panel, so
   the app built its own wrapper. Possible direction: each viewer crate provides
   a real panel; the app's opening policy only decides which panel to spawn, and
   "switch viewer" means replacing the panel.
2. **Persistence boilerplate repeats in each feature.** `code`, `files`, `git`,
   `lsp_panel`, `mascots` and `terminal` each have `register_persistence`,
   `project_persistence`, `clear_persistence` and sometimes `activate_*` /
   `validate_*`. The app also defines its own `GlobalPersist`/`ProjectPersist`
   markers and around 1.5k lines of debounced atomic file I/O. Workbench
   persistence may be too low-level. A Bedtime-owned store, with the app choosing
   only paths and scopes, could remove most of this.
3. **Panel catalog in the app.** Workbench deliberately has no panel factory
   registry. The app then rebuilt one (`menus::register_panel`, `PanelMenu`,
   `open_or_focus`) to drive the View menu and titlebar toolbar. Most apps will
   want "open or focus panel X".
4. **Quit flow.** `close.rs` (prompt about unsaved docs, save all, wait for work,
   exit) is generic for any document app. It is a candidate for Workbench, with
   the app supplying only the dialog wording or style.
5. **Applying settings.** `settings.rs` pushes preferences into plugins through
   `Applied*` diffing resources. A simpler pattern: plugins expose a config
   resource and react to changes themselves.
6. **Git.** `bedtime-git` is backend only. The panel, diff and presentation code
   (~4k lines) lives in the app. It should probably be a Bedtime panel plugin.
7. **Structure panel** lives in `bed_old/crates/bed-structure` but is a generic
   tree-sitter outline, so it is a candidate for `bedtime-treesitter`.
8. **Many error channels.** Panel `error` fields, `editor_error`, `ErrorNotice`,
   `request_error_dialog`, and the command-failure reporters. Pick one approach.
9. **Automation hooks scattered** through app modules behind
   `#[cfg(feature = "automation")]`. Keep them out of the new app unless needed.
10. **Workbench API style is mixed.** Some operations are free functions
    `fn(&mut World, ..)` and others are methods on `Workbench<'_>`. Errors are
    mostly `String`. The workbench README is about 1,000 lines. Review this when
    we first rely on each part.

## Proposed step order

Each step should produce a usable app. We can reorder at any point.

0. ✅ Window, workbench surface, camera, close request → exit (current `bed/main.rs`).
1. ✅ Global settings: resource, config file, Settings panel.
2. ✅ Workbench document lifetime, autosave and close/quit flow (see Decisions).
3. ✅ Document handling cleanup (see step log).
4. ✅ Workbench cleanup A + B (see step log).
5. **Foundation** ([docs/foundation.md](docs/foundation.md)), in order:
   stores + simplified persistence → settings + layers → workspaces in bed →
   commands + keybindings → menus + toolbar.
6. Project root + file tree panel.
7. Open a file into an editor tab (opening policy; observation 1).
8. Other viewers: image / hex / model, chosen by file type; "Open with".
9. Terminal.
10. Structure panel (observation 7).
11. LSP + status bar.
12. Git panel + diff (observation 6).
13. Extras: fonts, mascots, effects, shell `bed open`, automation.

## Step log

- **Step 0** — minimal shell exists in `bed/main.rs` (`UiPlugin` + `WorkbenchPlugin`,
  close request exits immediately).
- **Step 1 (global settings)**, committed 2026-10-26 (`a7bcabe`).
  `bed/config.rs` mirrors any serde resource to a RON file: a synchronous read
  while the plugin builds (so there is no "not loaded yet" state), a write 500 ms
  after the last change and on `AppExit`, and an unreadable file is never
  overwritten. `bed/settings.rs` has appearance-only `Settings`, applies them to the
  theme and `WorkbenchSurface`, and draws the panel. The titlebar gear opens or
  focuses the panel. Config lives at `$BED_CONFIG_DIR`, else
  `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`. Deliberately dropped from bed_old:
  multi-instance three-way merge, lock file, periodic re-read, async I/O,
  "storage ready" gates.

- **Step 2 (document lifetime / close / quit)**, committed 2026-10-26 (`92136e6`).
  Unused plugins moved to `bedtime_old/` (`2a46ff2`).
  New `bedtime-workbench/src/closing.rs` (release of unviewed documents, close and
  quit flow, `close_prompt`/`resolve_close`), prompt UI in `dialogs.rs`.
  `DocumentView` became the public `DocumentReference` relationship
  (`DocumentReferences` on the document). Any entity may hold one; on a panel it's
  the document shown. Viewers are spawned as `(Viewer, DocumentReference::new(id),
  InArea(area))`; `bind_document_view`, `unbind_document_view`, `bind_text_editor`
  and `unbind_text_editor` were removed. Observers mark documents as referenced
  and clear undo groups and panel dirty flags when a reference goes. The editor's
  document id is now only a private cache key for its view state. Closing a panel
  also counts references held by its child entities (e.g. a diff panel's sides). Autosave is the `DocumentAutosave` resource
  plus a workbench system. Removed: `PanelCloseRequested`, `commit_close`, public
  `close_targets`, `begin_document_save_all`/`DocumentSaveAll`,
  `check_session_documents`, public `tick_document_autosave`. bed no longer
  handles closes or `CloseRequested`. `bed_old` was excluded from the workspace,
  since it can't build against the new API.
  Not done yet: a manual test of the prompt in the running app (bed can't open
  documents yet).

- **Step 3 (document cleanup)**, committed 2026-10-26 (`84ad11d`), then
  `bedtime-document` was reorganized: `model.rs`, `history.rs`, `source.rs`, and
  `text/` (`mod.rs` = `TextBuffer` API, `rope.rs` = internals, `edit.rs` = `BufferEdit`).
  - `bedtime-document`: `DocumentModel`, `History`, `TextBuffer`, and a smaller
    `DocumentSource` (`load`, `save(model, expected_version)`, optional
    `version`/`check_interval`/`writable`). Removed `BinaryDocument`,
    `ByteDocument`, `ByteSnapshot`, `String` as a model, `TextChange`, the
    `TextDocument` alias, and the file-specific `path`/`location_generation`/
    `write_blocked`. `MemorySource` gained `get`/`set`/`remove`.
  - Workbench `documents.rs` (API) + `documents/store.rs` (typed state machine) +
    `documents/autosave.rs`. One `Document` component holds a typed store:
    `Loading | Failed | Ready(session)` and at most one source task (read or
    save). Saves go straight to the source.
  - Removed: save bindings, destination keys and their lock table, source /
    binding / location generation counters, `refresh_required`, the `Unloaded`
    fake session, `open_shared_document`, `open_prepared_document`,
    `Workbench::open_document`, `offer_document_external_update`, byte views and
    the page cache, the `BinaryDocument` history special case,
    `saved_document_capture`, `preflight_session_close`, the `test-support`
    feature and `create_memory_snapshot`.
  - Non-test document code went from about 2,950 to about 1,590 lines. Tests were
    rewritten around behaviour with a `TestSource` that can hold or fail reads
    and saves.

- **Step 4 (workbench cleanup A + B)**, committed 2026-10-26.
  - Removed: `native_path`, process management and `admit_operation` in
    `ManagedWork` (tasks only now), `create_panel`/`create_panel_in_area`/
    `spawn_panel`/`spawn_panel_in_area` (spawn `(MyPanel, InArea(area))`),
    `Workbench::for_entity`/`require_owned`/`work()`/`requests()`/`commands()`,
    `PanelInstance.key`/`Panel::KEY`, the `libc` dependency.
  - Stamps and revisions are gone (`LayoutStamp`, `LayoutRevision`,
    `LayoutSelectionRevision`, the per-frame topology diff). Gestures stay deferred:
    each records the layout it was made in, and is dropped if that layout isn't
    active, a named entity left it, or (border resize) the border is gone. The
    frontend cancels an in-progress gesture by comparing last frame's topology.
  - `WorkbenchRequests`/`WorkbenchFinished` replaced by a private `PendingEdits`
    queue, drained once (before keyboard routing).
  - `reconcile_workbench` → crate-private `validate_workbench`, run once per frame
    in debug builds only (and by persistence). `PanelInstance.dirty` removed; tabs
    ask `holder_unsaved` when drawn.
  - Deferred: command system (C, with menus), frontend split (D), surface/shell
    merge (D), persistence (E, with workspaces).

- **Step 5.1 (stores + persistence)**, committed 2026-10-27.
  See "Built so far" in [docs/foundation.md](docs/foundation.md). `persistence.rs`
  (capture/restore on Bevy's `DynamicWorld`) + `persistence/stores.rs` (`Store`,
  `WorkbenchStore`, `StoreBackend`, `DirectoryStore`, `add_store`, `persist_in`,
  load in `PreStartup`, save ~1 s when changed and on exit). Non-test persistence
  code went from ~1,490 to ~560 lines including the new stores. bed saves its
  layout to `<config>/world.ron`; restore verified in the running app.

- **Step 5.2 (settings + layers)**, committed 2026-10-27.
  `bedtime-workbench/src/settings.rs` (`Setting`, `setting!`, `Settings`,
  `set_setting`/`clear_setting`, layers in stores). bed's `config.rs` replaced by
  registered settings feeding an `Appearance` resource. See "Step 2" in
  [docs/foundation.md](docs/foundation.md).

- **Step 5.3 (workspaces in bed)**, committed 2026-10-27.
  `bed/workspace.rs`: `bed <folder>`, store under `<config>/workspaces/<hash>/`
  with a lock, workspace settings layer, Global/Workspace selector in the
  Settings panel. See "Step 3" in [docs/foundation.md](docs/foundation.md).

## Decisions

- 2026-10-26: `config.rs` may move to bedtime later; it stays in bed for now.
  (Superseded 2026-10-27: stores replace it.)
- 2026-10-26: Settings *composition* is per-app, not per-plugin. Plugins may later
  expose widgets for their own settings. Deferred.
- 2026-10-26: Unknown fields in config files are ignored and dropped on the next save.
- 2026-10-26: Plugins hand user actions (e.g. "opened an item in the file tree")
  to **app-provided callbacks**, like `bedtime-filetree`'s `FileTreeOnOpen`, rather
  than messages or a workbench-side handler.
- 2026-10-26: Opening panels separates three concerns. The **caller** supplies how
  to build the panel (`PanelFactory`). The **app** (`bed/panels.rs`, `open_panel` +
  `PanelPolicy`) decides whether an equivalent panel (same type + `PanelPolicy::Key`)
  in the *active layout* is focused instead, and which area a new one goes in.
  **Drag-and-drop always creates a new panel** where it's dropped, even a duplicate
  in the same area (e.g. two `main.rs` views scrolled to different places), so the
  same factory serves both paths and workbench needs no change.

- 2026-10-26: **Workbench owns document lifetime, autosave and close/quit prompts.**
  Apps don't implement any of it; extension points come later if an app needs them.
  - A document lives while something holds a `DocumentReference` to it (panels,
    their child entities, or any plugin/app entity). Unreferenced documents are
    released at the end of the frame. LSP/Structure/Git only *observe* documents.
  - Closing panels (tab, split, merge): if no dirty document loses its last
    reference, close now. Dirty with autosave on: save, then close once it succeeds.
    Autosave off: workbench prompts Save / Discard / Cancel. A failed save never
    closes; it prompts with the error (Retry / Discard / Cancel). Closing one of
    several viewers never prompts.
  - Quit: workbench handles `CloseRequested` with the same flow for all dirty
    documents (Save all / Discard all / Cancel), finishes background work, then
    sends `AppExit`. Apps hook `AppExit` for their own flushing.
  - Decision logic is in workbench core (headless-testable via a resolve call);
    the dialog is drawn by the `ui` feature.
  - Autosave policy is a workbench resource the app sets from its settings.

- 2026-10-27: **Foundation design agreed** (details in
  [docs/foundation.md](docs/foundation.md)): stores (workbench defines only
  `WorkbenchStore`; apps add and bind others, with pluggable byte-level backends),
  one `Persist` marker with per-app redirects, settings resolved through layers that
  live in stores (`apply` on resolved change; the setting is the unit of override,
  so per-entry things are split into separate settings). Keybindings are one
  setting per command (`Keybinding<C>`); the command system owns a flat
  command → chords table fed by those settings. Bindings have no panel scopes:
  a chord shared by several commands runs the one with a handler in the most
  specific scope (focused panel, then global). One
  workspace per process; workspace data under the config directory as in bed_old;
  settings and stores live in workbench; last writer wins across instances.

## Open questions

- Should viewer switching ("Open with") replace the panel, or should one panel
  host several viewers? (Decide in step 2/3.)
- Foundation details still open: see the list at the end of
  [docs/foundation.md](docs/foundation.md).
