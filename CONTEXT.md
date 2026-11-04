# Context

Working notes for the current direction of bed/bedtime. Keep this current;
delete what's done and no longer useful.

## Goal: compose apps from plugins

Building a bedtime app should mostly mean adding plugins and setting a few
options, instead of writing glue. Installing a viewer plugin should be enough
for its files to open, and adding a panel plugin should make that panel
openable and placeable. Apps can still override any default.

Principle: **plugins provide the pieces; the app decides the arrangement.**

- Plugins provide panels, action handlers, settings (with defaults),
  notifications and status widgets.
- The app decides keybindings, menus, the right side of the status bar, and
  the settings panel's layout.
- A defaults plugin group gives a working arrangement out of the box. Opt out
  of any part with `.disable::<…>()`, as with Bevy's `DefaultPlugins`.

Keep ordering knobs to a minimum. Following plugin order is fine, as long as
the result is deterministic.

## Crate structure

Three tiers:

1. **bedtime-ui**: egui, theming, fonts, icons, layouts, areas, panels.
2. **bedtime-workbench**: desktop-app features (documents, actions, keys,
   menus, settings, background work, persistence wiring, workspaces,
   clipboard, native integration). Apps should only need `UiPlugin` and
   `WorkbenchPlugins`.
3. **Plugins**: code, terminal, file tree, viewers, git, search, etc.

Some crates stay separate, hidden behind workbench and re-exported by it:

- `bedtime-document` and `bedtime-filesystem`: they don't depend on Bevy.
  That keeps ECS out of the models and their tests fast.
- `bedtime-persistence`: it sits below bedtime-ui, which uses it for layouts.
- `bedtime-clipboard`: adapted from Bevy 0.20's `bevy_clipboard`, so it can
  be swapped for the upstream crate later.

The `bedtime` crate re-exports these and will provide the defaults group.
Cargo features cover compile cost; runtime opt-outs work like Bevy's plugin
groups. A plugin that can be turned off must leave the others working: they
check for its resources instead of assuming them.

## Decisions

- **Panel placement:** focus an existing panel with the same key in the
  active layout; otherwise use an area that already has a panel of that type;
  otherwise use the largest area. The registration API still needs designing.
  It should generate the "Open X" actions and replace bed's `PanelPolicy` and
  `open_panel`. Workbench needs its own way to ensure a layout is active,
  because `open_panel` currently depends on bed's layout slots.
- **Document viewers:** the contract already exists (`Viewer<M>`, `Opener`,
  `Fit`, `open_path`). What's missing is placement. `Target::New { area: None }`
  should prefer an area that already shows viewers, not whichever area has
  focus (the file tree, for example). Replace the separate "beside" logic in
  bed's `preview.rs` and git's `diff.rs` with one `Target::Beside(panel)`.
- **Settings:** each plugin declares its settings, namespaced, with defaults.
  An app overrides a default by registering the same setting again, identified
  by type. A `settings_panel!` macro lays out the panel: sections, grouping,
  order. Leaving settings out of the panel is fine (an app may hard-code some).
  Keep saved keys explicit (namespace and name), never derived from type names.
  Settings move to whoever owns the resource they write: appearance settings
  to bedtime-ui/workbench, editor settings to bedtime-code.
- **Status bar:** the left side is for notifications, which any plugin can
  post, including ongoing work such as "indexing…". The right side belongs to
  the app; plugins can export widgets for it.
- **Keybindings:** plugins provide handlers but don't bind app-level keys. The
  app binds them with a macro that works with settings. Keys inside a panel
  (an editor's cursor movement, paste) stay with the panel. Plugins may export
  a suggested keymap that the app opts into.
- **New:** a workbench action that goes to the focused panel's handler first,
  then to the app's default.
- **Moves:** the LSP panel goes to bedtime-code; the theme editor to
  bedtime-ui; the dock menu becomes `set_dock_menu([actions])` in workbench's
  native module; `+open` goes to the defaults/shell layer. Workspaces stay
  required (they provide the working folder), with a single-store mode for
  apps that don't need a separate workspace settings layer.

## Plan

1. **Merge crates into workbench:** actions, menus, settings and work become
   modules of bedtime-workbench, with no behaviour change.
2. Panel registration and placement.
3. Settings declared by the crates that own them; the generic settings panel.
4. Status bar contributions.
5. Route `New` through the focused panel.
6. Move the LSP panel and theme editor.
7. Single-store workspaces.
8. Dock menu and menus.

Do one step at a time, then review it before starting the next.

## Log

- 2026-11-03: step 1 done (branch `worktree-workbench-consolidation`). actions,
  menus, settings and work are modules of bedtime-workbench, re-exported at the
  root as before (`settings::settings!` still works). actions' `settings`
  feature became workbench's `persistence`. Their READMEs are in
  `bedtime-workbench/docs/`. The `bedtime` facade dropped the `actions`, `menus`,
  `settings` and `work` features (now part of `workbench`). bed needed no changes.
  Pre-existing and left alone: `--no-default-features --features persistence`
  fails to build workbench tests (a test panel has no `draw` without `ui`), and
  `cargo fmt --check` is not clean repo-wide.
