# Project context

Last updated: 2026-10-26. Keep this file short and current. Update the step log
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

- `bed/` — the new app. Currently a minimal window + empty workbench (`bed/main.rs`).
- `bed_old/` — the previous app, kept as reference only. Do not extend it.
  `bed_old/crates/bed-structure` is still built because it is a workspace member.
- `bedtime/crates/` — the library crates. `bedtime/tools/` holds Python smoke
  tests that drive the old app via `bedtime-automation`.
- `vendor/` — patched `bevy_render`, `egui`, `rio-vt`, `arboard` (see root `Cargo.toml`).
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
cargo test -p bedtime-workbench   # or whichever crate changed
```

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
1. 🔄 Global settings: resource, config file, Settings panel (chosen first as the
   foundation for workspaces).
1b. Project root + file tree panel.
2. Open a file into an editor tab (opening policy; observation 1).
3. Other viewers: image / hex / model, chosen by file type; "Open with".
4. Save, dirty state, quit with unsaved documents (observation 4).
5. Menus, toolbar, commands, keybindings (observation 3).
6. Layouts + project session persistence (observation 2).
7. Settings panel + global preferences (observation 5).
8. Terminal.
9. Structure panel (observation 7).
10. LSP + status bar.
11. Git panel + diff (observation 6).
12. Extras: autosave, fonts, mascots, effects, shell `bed open`, automation.

## Step log

- **Step 0** — minimal shell exists in `bed/main.rs` (`UiPlugin` + `WorkbenchPlugin`,
  close request exits immediately).
- **Step 1 (global settings)**, implemented 2026-10-26 and awaiting review.
  `bed/config.rs` mirrors any serde resource to a RON file: a synchronous read
  while the plugin builds (so there is no "not loaded yet" state), a write 500 ms
  after the last change and on `AppExit`, and an unreadable file is never
  overwritten. `bed/settings.rs` has appearance-only `Settings`, applies them to the
  theme and `WorkbenchSurface`, and draws the panel. The titlebar gear opens or
  focuses the panel. Config lives at `$BED_CONFIG_DIR`, else
  `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`. Deliberately dropped from bed_old:
  multi-instance three-way merge, lock file, periodic re-read, async I/O,
  "storage ready" gates.

## Decisions

- 2026-10-26: `config.rs` may move to bedtime later; it stays in bed for now.
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

## Open questions

- Workbench silently does nothing on tab/split close unless the app handles
  `PanelCloseRequested` (bed: `panels::approve_closes`). That was an easy trap in
  step 1. Should workbench approve by default, with apps opting in to veto?

- Should viewer switching ("Open with") replace the panel, or should one panel
  host several viewers? (Decide in step 2/3.)
- Should persistence move into Workbench as a store, or stay a thin layer on top?
