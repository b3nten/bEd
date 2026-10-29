# Architecture: the bedtime crates

Status (2026-10-29): **migration steps 1–6 built** (persistence, settings, actions and work, menus, layouts and chrome, the framework); step 7 (opening) is next; the
rest still lives in `bedtime-workbench`, and the [migration](#migration) below
moves it one crate at a time. Update this file as decisions change.

## Goal

bedtime is for building desktop apps with Bevy, not only bed. Each layer is
useful without the ones above it:

- **A UI toolkit.** `bedtime-ui` alone gives an app a good-looking egui UI in
  Bevy: theme, fonts, icons, widgets, docking (layouts, areas, panels) and
  window chrome. No documents, no workspaces, no opinions about files.
- **Building blocks.** Persistence, settings, actions and menus are separate
  crates. An app can take the UI plus settings and a settings panel without
  anything else.
- **A framework.** `bedtime-workbench` is the opinionated layer: a
  document-based desktop app with workspaces, opening, and the unsaved-work
  flow, wired together so an app gets them by adding one plugin group. An app
  that wants different policies composes the building blocks itself.
- **Plugins** (file tree, editor, terminal, language servers, tree-sitter,
  structure) build on the framework.

bed is one app on top: policy only.

## Crates

| Crate | Depends on | What it is |
| --- | --- | --- |
| `bedtime-document` | — | Document models, history, sources. No Bevy. |
| `bedtime-persistence` | bevy | Stores and their backends; saving and restoring reflected world data |
| `bedtime-settings` | persistence, (ui) | Settings resolved through layers; with `ui`, the settings panel pieces |
| `bedtime-actions` | (settings) | Actions, handlers, keybindings, shortcuts |
| `bedtime-menus` | actions, (ui), (native) | The menu model, drawn with egui or as the native menu bar |
| `bedtime-ui` | (persistence) | Toolkit, docking, window chrome |
| `bedtime-work` | bevy | Background work owned by entities, with cancellation |
| `bedtime-clipboard` | — | Clipboard (exists) |
| `bedtime-filesystem` | document | File sources and change watching (ported from bedtime_old) |
| `bedtime-workbench` | all of the above | The framework |
| `bedtime-automation` | ui | Driving an app from scripts (exists) |
| `bedtime` | — | Re-exports the others behind features, as `bevy` does |

Parenthesised dependencies are optional, behind a feature of the same name.

### Dependency rules

- Nothing below workbench depends on workbench.
- `bedtime-ui` depends on no other bedtime crate except optionally
  persistence. Menus, settings and actions know about ui, not the other way.
- Lower crates report failures as **messages** (a store that couldn't be
  saved, a setting value that couldn't be used), never as notifications or
  dialogs. Workbench, or the app, decides how to show them.
- Every crate works with its optional features off.
- A small example app uses `bedtime-ui` docking without workbench. If it ever
  needs something from workbench, the boundary is in the wrong place.

## `bedtime-persistence`

From workbench's `persistence.rs` and `persistence/`. Three concepts:

- **Stores** are places, each with a backend: `Store`, `StoreId`,
  `add_store::<S>(backend)`, `StoreBackend`, `DirectoryStore`. A store holds
  named entries; `DirectoryStore` writes each as `<dir>/<entry>.ron`.
- **The world snapshot.** Components and resources marked
  `#[reflect(Persist)]`, entities included, are saved together as one
  reflected snapshot in the store the app picks:
  `app.persist_world::<WorkspaceStore>()` writes `world.ron` there. Only one
  store gets it. Entity references are mapped on restore, both sides of every
  relationship are written back, and a snapshot that can't be restored blocks
  its store so it isn't saved over.
- **Persisted resources.** `app.persist::<T, S>()` saves one serde resource
  as its own entry in store `S`: loaded at startup, written when it changes,
  failing on its own. The entry's name comes from a key on the type, so
  renaming the type doesn't orphan the file:

  ```rust
  #[derive(Resource, Serialize, Deserialize, Persisted)]
  #[persisted(key = "layout_defaults")]
  struct LayoutDefaults(/* … */);

  app.persist::<LayoutDefaults, GlobalStore>(); // → global/layout_defaults.ron
  ```

Settings layers sit on top (`settings.rs` in `bedtime-settings`), as one more
entry in a store. Also here:

- `config_directory(app)`: an override variable (bed's `BED_CONFIG_DIR`), else
  `$XDG_CONFIG_HOME/<app>`, else `~/.config/<app>`.
- Failures (a store that couldn't be read, restored or written) are a
  `StoreError` message.
- `world.persistence()` / `world.persistence_mut()` for capturing and
  restoring the snapshot directly (layout templates and tests use it).

There's no routing: today's `persist_in` and the rule that only
`WorkbenchStore` collects entities go away. Data that belongs somewhere other
than the world snapshot is a persisted resource.

`WorkbenchStore` stays in workbench, which calls `persist_world::<WorkbenchStore>()`;
with workspaces it's the workspace's store.

There's no separate config crate. "Config" so far is the config directory and
the convention of a global store, which fit here. Revisit if something bigger
appears.

## `bedtime-settings`

From workbench's `settings.rs`:

- `Setting`, `settings!`, layers, `Settings` (`get`, `get_at`, `inherited`,
  `overridden_above`), `SettingsAppExt`, `read_setting`, `apply`.
- Setting and clearing values. Today these are `Workbench` methods. Here they
  become methods on a settings view (`world.settings_mut().set::<S>(layer, v)`),
  the same pattern as `world.workbench_mut()`.
- With `ui`: the row helpers that are in bed today (`columns`, `slider`,
  `combo`, `reset_button`, `overridden_marker`, layer tabs), and a settings
  panel that draws registered sections.
- **Settings are declared with `settings!`** (renamed from `setting!`). Each
  setting is a block of entries, like a struct literal, and settings can be
  grouped into sections:

  ```rust
  settings! {
      "Appearance" {
          pub ColorTheme: String {
              default: "everforest-dark-medium".into(),
              key: "appearance.theme",
              ui: custom(themes::row),
          }
          pub FontSize: f32 {
              default: 15.5,
              key: "appearance.font_size",
              ui: slider(8.0..=72.0),
          }
      }
      "Startup" {
          pub ReopenLastWorkspace: bool {
              default: false,
              key: "startup.reopen_last",
              label: "Reopen the last workspace",
              only_in: [GlobalStore],
          }
      }
      // Outside a section: not shown in the panel.
      pub LastLayout: u8 { default: 0, key: "layouts.last" }
  }
  ```

  - Each entry becomes the `Setting` trait item of the same name, in any
    order. `key` and `default` are required; leaving one out is a compile
    error naming it, and so is a misspelt entry.
  - Optional entries fall back to the trait's defaults:
    - `ui`: from the value's type where it's obvious: `bool` a toggle, a
      fieldless enum a combo box (variants from `Reflect`), `String` a text
      field, a number a drag value. `slider(range)` and `custom(fn)` (a row of
      its own, like Theme's folder button and error icon) are in scope inside
      the entry.
    - `label`: the name split into words (`FontSize` is "Font size").
    - `only_in`: every layer.
  - The section is the enclosing string. A setting outside any section isn't
    shown, so internal settings stay out of the panel.
  - A hand-written `impl Setting` works too, providing only `key` and
    `default`.
- **The panel draws what's registered**: sections in the order they're
  declared, across calls in registration order (or the order the app gives
  with `app.settings_sections([...])`), each setting's
  row with the layer tabs, override markers and reset buttons bed has today.
  Keybindings are a section of their own, one row per action.
- **The registry is public.** A `SettingsUi` resource lists each shown
  setting's key, label, section and editor, and can draw, reset or clear it
  for a layer. An app that wants search, another layout or descriptions builds
  its own panel from it instead of using the basic one.

## `bedtime-actions`

From workbench's `actions.rs` and `shortcuts.rs`:

- `Action`, `ActionId`, the registry, handlers, `ActionRequests`,
  `ActionFinished`, `ActionAppExt`.
- `KeyChord`, keybindings, shortcut dispatch, conflicts.
- With `settings`: `Keybinding<A>` settings and per-layer keys (`keys_at`,
  `set_keys_in`). Without it, bindings are just the defaults.
- A failing handler's `Err` stays in `ActionFinished`; workbench turns it
  into a notification.
- **Actions don't know about panels.** They know a focused entity, from an
  `ActionFocus(Option<Entity>)` resource. Handlers are global, or registered
  for a component. An action runs the handler for a component on the focused
  entity if there is one, else the global handler; shortcut conflicts resolve
  the same way. Layouts keep `ActionFocus` on the focused panel. An app
  without panels registers global handlers and leaves it empty, or sets it to
  its own idea of focus.

Document actions (Save, Undo, Redo) stay in workbench.

## `bedtime-menus`

From workbench's `menus.rs`, `ui.rs` and the menu half of `native.rs`:

- The menu model: items, separators, submenus, enabled and checked state, each
  item tied to an action; snapshots for drawing.
- `ui` feature: the egui menu bar and popup menus (`draw_menu_bar`,
  `draw_popup_menu`), drawn with bedtime-ui widgets.
- `native` feature: the native menu bar (muda), and knowing when it's present
  so the app can leave out its egui menu bar.

## `bedtime-ui`

Today's toolkit, plus docking and window chrome from workbench (`layout.rs`,
`geometry.rs`, `membership.rs`, `composition.rs`, `frontend.rs`, `surface.rs`,
the window half of `native.rs`, `dialogs.rs`).

### Docking

- **Layouts → areas → panels.** `Layout`, `ActiveLayout`, `Area`, `InLayout`,
  `LayoutAreas`, `InArea`, `AreaPanels`, `Panel`, `PanelInstance`,
  `PanelFactory`, tab bars, splitting, joining, drag and drop, animation,
  `TabBarVisibility`, `AreaBackground`.
- **Operations** keep the view pattern: `world.layouts()` /
  `world.layouts_mut()`, the view over every layout and its areas and panels,
  with today's `split_area`, `focus_panel`, `move_tab`, and so on
  (`world.layouts().active()`, `world.layouts_mut().split_area(…)`).
- **Docking is placed, not in charge.** It fills the space it's given. The app
  lays out its window with ordinary egui (titlebar, a status bar, anything
  else) and puts the docking area in what's left. Workbench's status bar slot
  (`register_status_bar`) goes away; a status bar is app code.
- **Closing is a request.** Closing a panel, area or layout triggers a
  close-request event carrying the planned edit. Observers may stop it. If
  none does, it closes. Workbench's observer stops a close that would lose
  unsaved work, asks, and applies the edit again as confirmed (see
  [unsaved work](#documents-and-unsaved-work)). Without workbench nothing stops
  it, unless the app adds its own observer. This replaces docking calling into
  documents (`check_panel_drafts`).
- **Persistence**, with the `persistence` feature: docking's types are
  `Persist`, and layout templates (`capture_layout`, `instantiate_layout`)
  live here. Which layouts an app keeps (bed's five slots) is app policy.

### Window chrome

- Transparency, `WindowBlur`, `WindowBackground`, the titlebar geometry.
- **The titlebar is a place for content.** bedtime-ui provides the region
  (height, space for the native window buttons, dragging, double-click to
  zoom); the app fills it with whatever it likes: a menu bar from
  `bedtime-menus`, tools, a search field. Where content can't be drawn into
  the titlebar, or the app turns that off, the same content draws as a row
  under the native titlebar. One feature with a fallback, not two.

### Dialogs

bedtime-ui owns a **modal layer**, drawn after the app's root view on every
window, so nothing draws on top of a dialog and apps can't forget to draw
them. `ErrorDialog` and its queue move here; workbench's unsaved-changes
prompt goes through the same layer.

- Each modal belongs to a window: by default the one the request came from (a
  panel's or an action's), else the focused window. It's drawn on that window
  and blocks only that window.
- If its window closes, it moves to another, as error dialogs do today.
- App-wide questions (the exit prompt) go to the focused window.

**The app supplies each window's root view**: its titlebar content, its own
bars, and `layouts::show(ui, world)` for the space that's left.

## `bedtime-work`

From workbench's `work.rs`: `ManagedWork`, `ManagedTask`, `Cancellation`, work
owned by an entity and cancelled when it goes, and an orderly shutdown that
waits for work to finish. Depends only on Bevy, so plugins that don't need
documents (a terminal, the file tree) can use it.

## `bedtime-workbench`

What's left, plus what bed does today that any document app needs:

- **Documents in the world**: document entities, `DocumentReference`, loading
  and saving on workers, autosave, drafts.
- **Unsaved work**: the close observer, the prompt, `request_exit`, quit.
- **Workspaces**, optional (see [below](#workspaces-are-optional)): a root
  folder, the per-workspace store, the lock, the recent list, the default
  workspace, and switching by rebuilding the App (`bevy::winit::relaunch`).
  Moves from bed's `workspace.rs`; bed keeps choices like "reopen the last
  workspace" as a setting it passes in.
- **Opening**: open a target at a location, viewers (panels bound to a
  document), navigation history, restoring opened panels. Designed after the
  move; see [open questions](#open-questions).
- **Notifications**, and turning lower crates' failure messages into
  notifications.

### `WorkbenchPlugins`

Workbench is a plugin group, like Bevy's `DefaultPlugins`: the building
blocks (persistence, settings, actions, menus, layouts, work) and the
framework's own plugins (documents, unsaved work, opening, workspaces). An app
adds the group, replaces a part with `.set()`, or drops one with `.disable()`:

```rust
app.add_plugins(
    WorkbenchPlugins
        .build()
        .set(SettingsPlugin::with_layers(...))
        .disable::<WorkbenchUiPlugin>(),
);
```

An app that only wants some parts adds those plugins on their own.

**Plugins depend on parts, not on all of workbench.** The editor needs
documents and layouts; the terminal needs layouts and work. Each checks at
startup that what it needs is installed and fails with a clear message
("EditorPlugin needs DocumentsPlugin, part of WorkbenchPlugins"). Plugins don't
install missing parts themselves: Bevy panics when a plugin is added twice and
doesn't allow adding plugins after the build phase, so "add it if missing"
depends on the order plugins are added, and hides what's installed.

### Workspaces are optional

`WorkspacesPlugin` is a cargo feature (`workspaces`). It isn't in the default
group: it's built before the App (the window title needs the workspace) and is
given the App's constructor, for switching.
Without the feature nothing needs the bevy_winit fork, because relaunching
only exists to switch workspace.

- **`WorkbenchStore` is the app's store.** Without workspaces it lives where
  the app puts it (usually under its config directory). `WorkspacesPlugin`
  points it at the open workspace's store directory and adds the workspace
  settings layer.
- **A `Root` resource** (optional) is the folder plugins work in: the file
  tree, language servers, the terminal's working directory, opening relative
  paths. `WorkspacesPlugin` sets it to the workspace's folder; an app without
  workspaces sets its own or leaves it unset. Plugins never mention
  workspaces.
- **Switching and the recent list** exist only with the plugin. The exit flow
  (`request_exit`) is core; switching is one use of it.

### Documents and unsaved work

How workbench makes sure nothing is lost, without plugins writing any of it.

**In the world:**

- **A document is an entity.** Its `Document` holds the source, history,
  revision, and dirty, saving and conflict state. Loading and saving run as
  managed work; finishing sends a message. Opening is deduplicated by source,
  so two views of one file share one document.
- **References.** `DocumentReference` is a Bevy relationship from whatever is
  using a document: a panel, a child entity of a panel, or a plugin's own
  entity. The document's `DocumentReferences` lists them.
- **Drafts.** `DocumentDraft` marks input that hasn't reached the document yet
  (text mid-composition). Saving, undo and exiting are refused while a draft
  exists: workbench can't ask about what the document doesn't have.

**The rule:** a dirty or conflicted document is at risk when something would
remove its last reference. Three things can:

1. **Closing** panels, areas or layouts. References on entities inside a
   closing panel close with it.
2. **Exiting** (`request_exit`: quitting, switching workspace). Every dirty
   document is at risk.
3. **Releasing a document directly.** `close_document` refuses a dirty one;
   `discard_document` drops it on purpose.

**The flow** is one pending close at a time:

```text
close or exit requested
   │  find at-risk documents
   ├─ none ───────────────────────────────► do it now
   ├─ autosave on ─► Saving ─ all saved ──► do it
   │                    └─ a save failed ─┐
   └─ autosave off ───────────────────────┴► Asking (prompt lists documents and errors)
                                              ├─ Save    ─► Saving ─► …
                                              ├─ Discard ─► mark discarded, do it
                                              └─ Cancel  ─► nothing happens
do it (close) ─► apply the layout edit
do it (exit)  ─► wait for saves and managed work, then run the exit's last step
```

Closing reaches the flow through the [close request](#docking): workbench's
observer stops a close with at-risk documents and starts the flow; after the
answer, workbench applies the edit again as confirmed and the observer lets it
through. Exits call it directly.

At the end of each frame, documents with no references left are released,
unless they're unsaved and not discarded: those stay until saved or
discarded, so nothing disappears silently. Replacing a reference within one
frame (switching viewer) keeps the document.

A plugin gets all of this by holding a `DocumentReference`. It writes no
prompt and no close logic.

**Things at risk that aren't documents** (a terminal running a build, say) are
left out for now. If one is needed later, the flow already only needs "what's
at risk and what can be done about it", so it can take a second kind of item:
a component a plugin puts on any entity, with a title and Discard/Cancel
(and Save only if the plugin supplies one), never autosaved. Closing and exits
would find it the same way they find documents. A dummy document is the wrong
way to do this: it would have a meaningless Save, autosave would close it, and
it would show up wherever documents are listed.

## What stays in bed

Which settings exist, the themes it ships, its menus and titlebar content, the
status bar, screen effects, its five layout slots, which viewer opens which
file, and its startup choices.

## Forks

- **`vendor/bevy_winit`** (relaunching an App in the same window) is required
  by workbench's `workspaces` feature for now. Apps using that feature need the
  same `[patch.crates-io]` entry; the umbrella crate documents it. Apps without
  workspaces build against plain Bevy. Later: try to get
  `relaunch` upstream, or spike a workbench launcher that owns the window and
  renders each workspace App into a texture on a shared wgpu device. The
  launcher would have to forward input, IME, scale, cursor, drops, clipboard
  and accessibility itself, so it isn't obviously less code. Copying frames
  through the CPU, as Bevy's `headless_renderer` example does, is too slow
  for an editor.
- **`vendor/bevy_render`** (screenshots on frames without a drawable) is a bug
  fix; upstream it. A launcher rendering into textures would also make it
  unnecessary.
- **`vendor/egui`** (widget transitions) and **`vendor/arboard`** are
  unaffected.

## Migration

One crate per step, bottom up. After each step every test and automation
script passes, workbench re-exports what moved so bed changes as little as
possible, and the step is reviewed before the next.

1. **Persistence.** Move stores, backends and the world snapshot into
   `bedtime-persistence`, with `persist_world`, `persist::<T, S>` and the
   `Persisted` derive, and `config_directory`. Store failures become
   messages. bed's `LayoutDefaults` and recent list become persisted
   resources.
2. **Settings.** Move settings into `bedtime-settings`; bed's row helpers move
   into its `ui` feature.
3. **Actions and work.** Move actions, keybindings and shortcuts into
   `bedtime-actions`, and managed work into `bedtime-work`.
4. **Menus.** Move the menu model and both renderers into `bedtime-menus`.
5. **Docking and chrome.** Move docking, window chrome and error dialogs into
   `bedtime-ui`. Add the close request; workbench's unsaved-work check becomes
   an observer. Docking fills the space it's given; bed draws its own status
   bar. The biggest step: most of it is moving code, about 6,000 lines.
6. **The framework.** `WorkbenchPlugins`; workspaces move from bed into
   workbench as an optional plugin and feature, with `Root`. Add the umbrella
   crate and the docking-only example.
7. **Opening.** Design and build opening and viewers, with
   `bedtime-filesystem` and the file tree as the first user.

Then the plugins: editor and tree-sitter, structure, language servers,
terminal.

## Open questions

- **Names** of what moves into `bedtime-ui`: `WorkbenchSurface`,
  `WorkbenchCamera` and friends lose the `Workbench` prefix there.
- **The panel drawing contract.** Panels take `&mut World` today. Drawing
  from read-only access and sending intents (actions, layout edits) is the
  preferred direction; do it after step 5, as its own step, so the move stays
  mechanical.
- **Opening:** whether targets are URIs (`file:`, `untitled:`, plugin
  schemes), whether navigation history belongs to it, who draws a picker for
  several results (references, definitions), and whether preview tabs are a
  docking feature that apps turn on.
