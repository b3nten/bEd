# Foundation: stores, persistence, settings, workspaces, commands

Status (2026-10-27): **steps 1–7 built** (stores, persistence, settings,
workspaces, commands and keybindings, menus and toolbar, fonts, automation). Update this file as decisions change during the build.

The goal is a foundation solid enough that everything after it is "bring in a
plugin": file tree, editor, viewers, terminal. Those plugins should only need to
mark what they persist, expose config resources, and register commands.

## Overview

| Concept | What it is | Who defines it |
| --- | --- | --- |
| **Store** | A named place persisted data is saved: world data and, optionally, one settings layer | Workbench defines `WorkbenchStore`; apps add their own |
| **Store backend** | Where a store's bytes live (a directory, a SQLite row...) | Workbench ships `DirectoryStore`; apps can supply others |
| **Persisted world data** | Reflected components and resources marked `Persist` | Workbench, plugins and apps mark their types |
| **Setting** | A typed, named value resolved through layers, with an `apply` hook | The app (settings composition is per-app) |
| **Layer** | The setting values kept in one store; layers stack in precedence order | The app chooses which stores carry layers, and their order |
| **Workspace** | A project folder plus a store bound to it | bed (maybe a bedtime plugin later) |
| **Command / keybinding** | A named action with handlers; a flat table of command → chords | Workbench mechanism; plugins and apps define commands |

Workbench never hears the word "workspace". It knows about one store of its own,
the general store/layer machinery, and nothing about where things are saved.

## Stores

A store is identified by a type, so binding and redirects are compiler-checked.

```rust
// bedtime-workbench: the only store it defines.
pub struct WorkbenchStore;   // layouts, areas, panels, panel-local data, plugin resources

// bed:
struct GlobalStore;          // recent workspaces, layout templates, global settings

// bed at startup (one workspace per process, so this happens once):
app.add_store::<GlobalStore>(DirectoryStore::new(&config_dir))
   .add_store::<WorkbenchStore>(DirectoryStore::new(config_dir.join("workspaces").join(hash)))
   .add_settings_layer::<GlobalStore>()
   .add_settings_layer::<WorkbenchStore>();   // added later = higher precedence
```

A simple app that treats the workbench as global binds only `WorkbenchStore`
(to its config directory) and adds one settings layer. An app that doesn't bind
`WorkbenchStore` doesn't persist the workbench.

### Backends

Workbench owns the *format* (how settings and world data become bytes). A
backend only decides *where the bytes live*, as named entries:

```rust
pub trait StoreBackend: Send + Sync + 'static {
    /// Read a named entry ("settings", "world"). `None` if it doesn't exist yet.
    fn read(&self, entry: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&self, entry: &str, bytes: &[u8]) -> Result<(), StoreError>;
    /// Optional cheap change check, so outside edits are reloaded.
    fn version(&self, entry: &str) -> Result<Option<String>, StoreError> { Ok(None) }
}
```

- `DirectoryStore` ships with workbench: one file per entry (`settings.ron`,
  `world.ron`), atomic writes, modification time as the version.
- Other backends (SQLite, a remote service) need no knowledge of ECS, reflection
  or settings.
- Threading follows document sources: backends may block and are called on
  workers, except the **startup read, which is synchronous** so settings and
  restored state exist before plugins initialise.
- A structured backend (receiving the captured world and settings map, e.g. to
  write one table per component) is possible later; byte-level comes first.

### Saving and reloading

- Saves are **debounced (1–2 s) after changes, and on quit**. Change detection
  compares the newly captured bytes with what was last written, so persisted
  types need no change-tracking of their own.
- **Last writer wins** across instances. A store whose backend reports a new
  version for an entry reloads it: settings layers reload live; world data is
  only read at startup.
- An entry that can't be read or decoded is **never overwritten**; the error is
  reported and the store stops saving that entry until it's fixed (the rule
  bed's `config.rs` already follows).

## Persisted world data

Builds on the existing reflected world serialization, simplified.

- **One marker: `#[reflect(Persist)]`.** It replaces `WorkbenchPersist` and the
  app-defined partition markers. A marked type goes to `WorkbenchStore` by
  default. Entities (layouts, areas, panels and their marked components) always
  go to `WorkbenchStore`.
- **The app can redirect a resource** to another store without the plugin
  changing: `app.persist_in::<RecentFiles, GlobalStore>()`.
- **Plugins hook in by marking types.** A file tree's `show_hidden` lives on its
  panel component, marked `Persist`, and travels with the layout. Runtime state
  is rebuilt through required components and hooks, as now.
- **Restore once at startup**, into an empty world, before the first frame. The
  "restore window" / sealing phases go away. Replacing a single layout at
  runtime (templates, "reset layout") stays as a narrower operation.
- **Kept from the current implementation:** relationships restored in order (tab
  and area order), entity remapping, `validate_workbench` before capture and
  after restore, a size limit.
- **Removed:** multiple partition markers, the startup admission/sealing phases,
  most of the custom limits and per-relationship cycle checks.
- **Layout templates** (bed_old's five slots, "reset layout") become a resource
  in the global store holding captured layouts: same format, no separate code.
- bed's `config.rs` is replaced by stores.

## Settings

### Defining a setting

Settings are app-defined. Plugins don't know the settings system exists: they
expose config resources (e.g. `FileTreeConfig`), and the app's settings write to
them through `apply`. Plugins may provide egui widgets for editing their config.

```rust
pub trait Setting: 'static {
    type Value: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync;
    /// "filetree.indent". A function rather than a const so generic settings
    /// can derive it (see `Keybinding<C>`).
    fn key() -> Cow<'static, str>;
    fn default() -> Self::Value;
    /// Runs whenever the resolved value changes, including at startup.
    fn apply(_world: &mut World, _value: &Self::Value) {}
}

// A small macro covers the common case:
setting! {
    /// File tree indentation, in points.
    FileTreeIndent: f32 = 16.0, key = "filetree.indent",
    apply = |world, indent| world.resource_mut::<FileTreeConfig>().indent = *indent,
}

app.register_setting::<FileTreeIndent>();
```

A setting can restrict which layers may hold it (e.g. "restore last workspace on
launch" only in `GlobalStore`). The exact form (a const list of store types, or a
method) is decided when building.

### Layers and resolution

A layer is the settings stored in one store:

```rust
struct SettingsLayer {
    store: StoreId,
    values: BTreeMap<String, ron::Value>,   // key -> value, only what was set here
}
```

Layers stack in the order the app adds them; bed's stack is
`default < GlobalStore < WorkbenchStore`.

- **Get:** `settings.get::<S>()` walks from the top layer down and returns the
  first value that's present, allowed for that setting, and decodes. Otherwise
  `S::default()`. Resolved values are cached per setting.
- **Set:** `settings.set::<S>(layer, value)` writes to that layer and marks its
  store for saving. Setting a layer the setting doesn't allow is an error.
- **Clear:** `settings.clear::<S>(layer)` removes the value so the layer inherits.
- **Untyped storage keeps unknown keys.** Values for settings that aren't
  registered (e.g. a plugin that isn't loaded) are kept and written back.
- **Bad values fall through.** A value that doesn't decode is skipped for
  resolution and reported, but not deleted, since the user may fix the file.

### `apply`

Runs when a setting's **resolved** value changes, not on every write:

- Setting global to 18 while the workspace overrides it to 12: still 12, no call.
- Clearing the workspace override: becomes 18, `apply(18)` runs.
- A layer loading, being added or removed, or reloading from disk: every
  registered setting is re-resolved; `apply` runs for those that changed.
- At startup, once layers are loaded, `apply` runs once for every registered
  setting, so consumers start out correct.

### The unit of override is the setting

A layer overrides a whole setting value, never part of one. Anything that should
be overridable per entry is split into one setting per entry, with a derived key:
keybindings (`keys.<command>`), and later per-language server configs
(`lsp.server.rust`) or file associations (`files.open_with.png`). Ordinary
resolution then gives per-entry overrides, so there is no merge-by-key concept.

### Settings UI

For each setting the UI can show the resolved value, which layer it comes from
("inherited from Global"), and a reset button that clears the current layer.
That's bed_old's Global/Workspace toggle and ↶ button, from the general mechanism.

## Workspaces (bed)

- **There's always one workspace per App**: the folder bed was opened at, or
  the **default workspace**, rooted at the home folder, for using bed without
  a project (as a terminal). Both are saved the same way: `WorkbenchStore`
  bound to `<config>/workspaces/<hash of path>/` (`default/` for the default),
  plus its settings layer above the global one.
- **Switching replaces the workspace in the same window** by rebuilding the
  App (see "Switching workspace"), so no plugin state carries over. Open
  Folder, Open Recent, Close Folder (back to the default) and later a
  terminal's `+open` all switch; New Window starts another process.
- Startup: `bed <folder>` opens that folder; otherwise the default workspace,
  or with `startup.reopen_last` (global only, off by default) the most recent
  folder that isn't open in another window.
- Features that need a project can check `Workspace::default` and step back
  (a command that isn't available, no toolbar button).
- Later possibility: a shared, committable `<repo>/.bed/settings.ron` as another
  app-defined store and layer between Global and the workspace.

## Commands and keybindings

The deferred command cleanup (C) is part of this foundation.

### Commands and handlers

- **One handler mechanism.** Global handlers in a registry resource; panel
  handlers as components on the panel. Removed: handler-as-system entities,
  generation counters, metadata entities, duplicate registration paths.
- **Context lives in handlers, not bindings.** A panel says what it can do by
  having handlers; bindings have no panel scopes.

### The binding table

The command system owns a plain resource mapping each command to its chords. It
is what dispatch, menu hints and conflict notes read. It does not depend on the
settings system: an app without settings, or a test, fills it directly.

### Keybindings as settings

Each command's chords are an ordinary setting, so layering, persistence and
"reset to inherited" need nothing special:

```rust
struct Keybinding<C: Action>(PhantomData<C>);
impl<C: Action> Setting for Keybinding<C> {
    type Value = Vec<KeyChord>;
    fn key() -> Cow<'static, str> { format!("keys.{}", C::KEY).into() }
    fn default() -> Vec<KeyChord> { C::DEFAULT_KEYS.to_vec() }
    fn apply(world: &mut World, chords: &Vec<KeyChord>) { /* update the binding table */ }
}
```

```ron
// GlobalStore settings.ron
"keys.editor.duplicate_line": ["Ctrl+B"],
"keys.file.save": ["Ctrl+S"],

// WorkbenchStore settings.ron
"keys.editor.duplicate_line": ["Ctrl+D"],   // replaces global's, for this action only
```

- Registering a command registers its `Keybinding<C>`, so plugin commands are
  rebindable without the plugin doing anything.
- A higher layer **replaces** an action's chords: a workspace binding Ctrl+D
  replaces global's Ctrl+B; the action doesn't end up with both.
- Within one layer an action can have several chords (redo: Ctrl+Shift+Z, Ctrl+Y).
- An empty list unbinds the action in that layer.

### Dispatch: one chord, several commands

Several commands may share a chord. When it's pressed, dispatch takes every
command bound to it and runs the one with a handler in the **most specific
scope**: the focused panel first, then global.

Example: Ctrl+F bound to both `edit.find` and `model.toggle_backfaces`.

| Focus | Available handlers | Runs |
| --- | --- | --- |
| Editor | `edit.find` (panel) | Find |
| Model viewer | `model.toggle_backfaces` (panel) | Toggle backfaces |
| A panel handling neither | a global handler, if any | That global handler, else nothing |
| Model viewer, with a global `edit.find` handler | panel beats global | Toggle backfaces |

A **conflict** is two commands sharing a chord with handlers in the *same* scope
(one panel handling both). Dispatch reports it rather than guessing.

### Conflicts in the settings UI

Handlers are added when panels spawn, so the settings UI can't know in advance
which panels handle which commands. It notes sharing ("Ctrl+F is also bound to
Toggle backfaces") without calling it an error; real conflicts are reported when
the key is pressed. Panels don't declare their commands (no `Panel::COMMANDS`)
unless that turns out to be needed.

### Menus

Built afterwards on the same pieces: command labels plus resolved shortcuts.

## Fonts

bedtime-ui owns the mechanism; the app decides which families to offer.

- The theme carries the font: `Metrics::font_family: Option<String>` next to
  `font_size`. `None` is the bundled Bedfont.
- The UI runtime applies fonts itself, when a context first draws and whenever
  the theme's family changes. Apps never call `set_fonts`. A named family
  replaces the primary regular and bold faces; Bedfont's and egui's Unicode
  fallbacks stay behind it. A family that can't be found falls back to Bedfont.
- `InstalledFonts` (feature `system-fonts`, fontdb) scans installed fonts on a
  background thread. An app opts in by initialising the resource. It reports
  `loading()`, `error()` and `families()` (name, monospaced). While it's
  loading, a named family waits and the current fonts stay; once it's ready
  the family is applied or falls back.
- bed: an `appearance.font_family` setting (any layer) flows through
  `Appearance` into the theme like the size. Settings lists Bedfont plus the
  installed monospaced families (the editor and terminal need fixed-width
  text), and says when the saved family is missing.

## Switching workspace (spike, 2026-10-27)

Opening another folder (Open Folder, Open Recent, a terminal `+open`) should
replace the window's workspace rather than open a new window. Swapping stores
inside one App would leave plugin state (resources and entities that aren't
persisted) behind from the previous workspace, and plugins can't know about
workspaces. So each workspace gets a fresh App, and only the window survives:

- `vendor/bevy_winit` (see its `BED_PATCHES.md`): `bevy::winit::relaunch(world,
  make)` exits the App normally (stores save), drops it, builds the next with
  `make`, and the next App adopts the open native window.
- bed: `app(Start) -> App` builds the App for one workspace; `main` runs it.
  `workspace::switch(world, Start)` relaunches into another workspace, refusing
  a folder another window has open (`workspace::available` probes its lock).
- bedtime-automation keeps its HTTP server across relaunches (the server lives
  on Bevy's global task pool); the next App takes over its request mailbox.
- Result: the next App draws about 0.08 s after the request, the window keeps
  its size and position, the layout is restored from the workspace store, and
  memory stays flat over repeated relaunches. Native window pieces (blur view,
  menu) are removed by their `Drop` with the old App.
- Not yet: carrying state such as a terminal's shell into the next App (a
  handoff store, deferred), an effect over the switch if it ever feels slow.

## Built so far

### Step 1: stores and persistence

As designed, with these specifics and differences:

- Relationships are restored with Bevy's `DynamicWorld::write_to_world`, which
  writes both sides exactly as saved without running relationship hooks. That
  replaced the staged insertion, relationship plans and
  `register_persisted_relationship`.
- `PanelInstance` isn't saved. The concrete panel component supplies it through
  `#[require]`; restoring both could lose the panel's draw function depending on
  insertion order. Titles come from `Panel::LABEL` (or are set at runtime) and
  the preview flag isn't kept.
- A panel with nothing persisted is left out of the save (and out of its area's
  tab list) instead of failing the save.
- Saved files use runtime entity ids; Bevy remaps them on load. No canonical ids.
- Saves are synchronous on the main thread, about once a second when changed,
  and on `AppExit`. Restore happens in `PreStartup`.
- Removed for now: capturing or replacing a single layout at runtime
  (`CapturedLayout`). It returns with layout templates / reset layout.
- A type in the saved file that isn't registered (e.g. a plugin that's no longer
  loaded) fails the whole restore; the store is then blocked and its file kept.
- bed stores `WorkbenchStore` in its config directory until workspaces exist.

### Step 2: settings and layers

As designed (`bedtime-workbench/src/settings.rs`), with these specifics:

- Layer values are stored as raw RON (`ron::value::RawValue`), not `ron::Value`,
  which loses enum variant names. Unknown keys survive untouched.
- `get` decodes from the layers each call; no cache yet.
- Which layers a setting may use is `Setting::allowed_in(StoreId)`; the macro's
  `only_in = [Store, ...]` generates it. `StoreId::of::<S>()` names a store as a value.
- Settings layers load in `PreStartup` before world data (`StoreSystems`), then
  `apply` runs for every registered setting. Saves happen about once a second when
  a layer changed, and on exit. Layer files are checked for outside changes about
  once a second and reloaded.
- bed: `config.rs` is gone. Appearance settings are `setting!` definitions that
  feed an `Appearance` resource, applied to the theme/surface by one system. bed's
  stores live in `<config>/global/` (`GlobalStore`) and `<config>/workbench/`
  (`WorkbenchStore`, until workspaces exist). Files from earlier steps at the top
  of the config directory are no longer read.
- Not done: app-level defaults, a settings UI showing which layer a value comes
  from (bed's panel only edits the global layer), skipping an empty `world`
  entry for stores with nothing persisted (bed writes an empty `global/world.ron`).

### Step 3: workspaces (bed)

`bed/workspace.rs`, wired in `bed/main.rs` (`stores`):

- `bed <folder>` opens a workspace; a path that isn't a folder is reported and
  bed starts without one. Without a folder, the workbench is stored in
  `<config>/workbench` as before.
- The store is `<config>/workspaces/<FNV-1a hash of the canonical path>/` (stable
  across Rust versions), with `path.txt` naming the folder and a `lock` file.
- **Lock** (settles the open question): a second instance on the same folder
  can't claim the lock, so its `WorkbenchStore` isn't bound. It still runs, its
  workspace settings layer lives in memory only, and it shows a dialog saying
  its layout won't be saved. The OS releases the lock on exit or crash.
- The workspace settings layer (`WorkbenchStore`) sits above `GlobalStore`. The
  Settings panel has a Global / Workspace selector; rows show "inherited" when a
  workspace value comes from below, and ↶ clears the selected layer.
- `Workspace { root }` resource; the window title is "<folder> — bEd".
- Not yet: "Open Folder…" (launch another instance), "Open Recent", restoring
  the last workspace when started without a folder. These arrive with commands
  and menus (step 4/5).

### Step 4: commands and keybindings

As designed (`bedtime-workbench/src/commands.rs`), with these specifics:

- `Action` gained `DEFAULT_KEYS` and `REPEATS` (repeat is a property of
  the command now, not of each binding). `app.register_action::<C>()` makes a
  command known and registers its `Keybinding<C>` setting (`keys.<KEY>`);
  `app.add_action_handler::<C>(f)` also sets its global handler.
- One handler type, `fn(&mut World, ActionInvocation) -> Result`. Global
  handlers live in `ActionRegistry`; panel handlers are `ActionHandler<C>`
  components. The registry finds a panel's handler through a function stored at
  registration, so there's no handler index, no handler entities and no
  generation counters. A global handler is told the panel it runs for (Save
  uses it to find the focused document).
- Requests carry only command and context; handlers and targets are resolved
  when each request runs. The "admitted handler" revalidation is gone.
- Removed: `InPanel`/`PanelCommandParts` (only commands used them), `Binding`
  entities, `ActionEnabled`, `ContextCommandInfo` entities,
  `register_context_command`, `install_global_handler`/`install_panel_handler`,
  `Workbench::register_handler` and friends, panel-owned menu entries, and
  `register_document_shortcuts` (Save/Undo/Redo now have default keys;
  `register_document_handlers` still opts into their handlers). Their keys are
  `document.save`, `document.undo`, `document.redo`.
- Chords are stored as text: `"Primary+Shift+Z"`, `"Ctrl+Enter"`, `"F5"`.
  `Primary` is Cmd on macOS, Ctrl elsewhere; parsing accepts Cmd/Super/Meta,
  Control, Option, and is case-insensitive.
- `keys_in` / `set_keys_in` give settings UIs per-layer access by command id.
- bed: `OpenSettings` (`app.open_settings`, Primary+,) is the first app command.
  The titlebar gear invokes it and shows its shortcut in the tooltip. The
  Settings panel has a Keyboard Shortcuts section: click a command's keys and
  press new ones (recorded with Primary for the platform's main modifier),
  ↶/inherited per layer as for other settings, and "Also bound to …" notes.
- Not done: recording a chord can also trigger that chord's command (the
  shortcut collector still sees it); recording sets a single chord, replacing
  any others the command had.

### Step 5: menus and toolbar

Workbench:

- **One handler type.** `Handler` holds what a command does and, optionally,
  when it can run (`available_when(fn(&World, Option<Entity>) -> bool)`). Global
  handlers and `ActionHandler<C>` both hold one; the panel handler's `enabled`
  field is gone. Menus, the toolbar and dispatch all use `action_available`,
  so apps need no availability sync system. Save/Undo/Redo are available only
  when the focused panel's document can be saved / has history.
- `MenuItem.enabled` is gone; `MenuItem::command::<C>()` labels an item with
  its command.
- Invocations carry a `source` (the menu item or button), so one command can
  serve several items: each Open Recent item carries its path.
- `settings::read_setting::<S>(&backend)` reads a setting before the app
  exists.

bed (`bed/menus.rs`):

- Menus: **bEd** (Settings…, Quit), **File** (Open Folder…, Open Recent ▸,
  Save), **View** (Settings). Native on macOS, an egui bar in the titlebar
  elsewhere.
- Commands: `app.quit` (Primary+Q, sends `CloseRequested`), `app.open_folder`
  (Primary+O, folder dialog), `app.open_recent` (from a menu item, that folder;
  from a key, the most recent other folder). Opening a folder launches a new
  `bed <folder>` process; the folder already open here is ignored.
- Toolbar: `tool::<C>(icon, factory)`; tooltip, shortcut and enabled state come
  from the command; dragging creates a panel from `factory`.
- Recent folders are the `recent` entry of the global store's directory
  (`recent.ron`), written directly through the backend at startup so `main` can
  read it before the app exists. At most 10.
- Startup: with no argument and `startup.reopen_last` (global-only, default on),
  bed reopens the most recent folder that still exists. If it's open in another
  instance, bed starts without a folder instead of sharing it.
- Deferred: Edit menu (Cut/Copy/Paste and widget undo; with the editor plugin),
  layout slots and reset/save layout (need layout templates), panel-owned menu
  entries.

### Step 6: fonts

bedtime-ui:

- `Metrics::font_family: Option<String>` (serde default `None`, Bedfont).
- The runtime tracks each camera's applied family and calls `set_fonts` when
  the theme's family differs; new contexts start with Bedfont. While
  `InstalledFonts` is loading, a named family isn't applied (and isn't recorded,
  so it's retried each frame until discovery finishes).
- `fonts::InstalledFonts` (feature `system-fonts`): discovery thread, polled
  before drawing. A family is monospaced when all its faces are; names starting
  with `.` (platform-private) are hidden. `fonts::with_primary(regular, bold)`
  builds definitions with a family in front of Bedfont.
- `theme::FONT_SIZES` is the one font size range; bed's setting clamps to it.
- Removed from bed_old's `fonts.rs`: per-camera `AppliedFont` tracking and the
  ordering against UI's own `set_fonts`.

bed:

- `appearance.font_family` (`Option<String>`, any layer) → `Appearance` → the
  theme, beside the size.
- `main` initialises `InstalledFonts` at startup. The Font row lists Bedfont and
  the installed monospaced families, and notes when fonts are loading, when
  discovery failed, or when the chosen family isn't installed.

### Step 7: automation

Scripts drive the real app over the Bevy Remote Protocol, with no automation
code in the app or plugins beyond adding one plugin.

- `bedtime-automation`: `AutomationPlugin::from_env()`. With the crate's
  `enabled` feature and `BEDTIME_AUTOMATION_PORT` set, it adds Bevy's
  `RemotePlugin` (so `world.query`, `world.get_resources`... work on reflected
  types) and these methods: `bedtime.status` (frame, queued steps, window size
  and scale, finished screenshots, errors), `bedtime.input` (steps run one per
  frame: move, press/release, key_down/key_up by `KeyCode`, text, scroll,
  wait), `bedtime.ui` (the AccessKit tree: role, label, value, rect in logical
  pixels, state), `bedtime.screenshot` (the window as rendered; written then
  renamed, so a listed file is complete), `bedtime.invoke` (a command by key),
  `bedtime.exit`. Otherwise the plugin does nothing.
- Unfocused: `AutomationPlugin::active()` tells the app to open its window with
  `focused: false` (on macOS winit then doesn't activate the app). Each frame
  the plugin marks the window focused without change detection and restores it
  in `PostUpdate`, before winit applies window changes in `Last`, so shortcuts
  and egui behave as if focused while the user keeps working elsewhere.
- bedtime-ui `accessibility` feature: while `AccessibilityTree` exists, the
  runtime enables AccessKit and records each frame's tree.
- bed: `automation` feature (`bedtime-automation/enabled`); `main` adds the
  plugin and uses `active()` for the window's focus. Nothing else.
- `tools/bed.py` (stdlib only): `Bed(folder, settings={key: ron})` launches
  with a temporary config dir and free port; `click(label)`, `key(chord)`,
  `type`, `scroll`, `drag`, `invoke`, `find`/`exists`/`widgets`,
  `screenshot(name)`, `call(method, params)` for anything else; `close()`
  exits normally. `python3 tools/bed.py run [paths]` runs saved scripts in
  `tests/automation/` (alias `cargo test-automation`). Output in
  `target/automation/`.
- Rule: when a script can't see something, reflect it or label its widget;
  don't add app-specific methods.

### Step 8: the default workspace and switching in place

- `Workspace { root, default }` always exists. `workspace::open(Start, ..)`:
  `Start::CommandLine(path)` (reopen-last applies when there's no path),
  `Start::Folder(path)`, `Start::Default`. The default workspace's store is
  `<config>/workspaces/default/`; `<config>/workbench/` is no longer used.
- A second window on the default workspace runs unsaved without a dialog; a
  folder named explicitly that's open elsewhere opens unsaved with one;
  switching to such a folder is refused instead.
- File menu: New Window (Cmd+Shift+N), Open Folder…, Open Recent ▸, Close
  Folder, Save. Recent folders exclude the default workspace.
- Settings: the layer selector reads "Default workspace" there.
- `tests/automation/workspaces.py` switches to the default workspace and back,
  checking each workspace's layout is its own and the window keeps its size.

### Step 9: layout templates and five layouts

Workbench (`persistence/templates.rs`): `capture_layout`, `instantiate_layout`,
`LayoutTemplate`. Capture and restore share filtering, serialization, parsing
and writing with whole-store saves. Panels referring outside the layout are
left out, so file-bound panels don't end up in defaults without a marker.

bed (`bed/layouts.rs`):

- `LayoutSlot(0..5)` on each numbered layout, saved with the workspace.
  Layout 1…5 (Cmd+1…5) switches, making the slot's layout the first time from
  its default. At startup the saved active layout stays; otherwise layout 1 is
  opened (a workbench saved before slots gets its active layout as layout 1).
- `LayoutDefaults([Option<LayoutTemplate>; 5])`, persisted in the global
  store: Save Layout as Default sets the current slot's; Reset Layout rebuilds
  the current layout from it (built first, so a bad default leaves the layout
  alone). No default is a single empty area.
- Layout menu: Layout 1…5, Save Layout as Default, Reset Layout.
- `tests/automation/layouts.py`: slots keep their panels, and a saved default
  builds layout 1 in another workspace.

### Step 10: the Workspaces panel

The default workspace isn't treated as a workspace you manage: it has no
settings layer (Settings shows only Global there) and isn't listed. Its layout
is still saved.

`bed/workspace/panel.rs`: `WorkspacesPanel` (View → Workspaces,
`app.open_workspaces`), a list of folder workspaces: recent folders, then any
other folder with a saved store (its `path.txt`). Each row: name, path, then
"This window" / "Open in another window" / "Folder missing" or Open (switches
this window), and Delete. Delete asks to confirm, then `workspace::delete`
removes the folder's store (never the folder) while holding its lock, and
takes it off the recent list; it's unavailable for this window's workspace or
one open elsewhere. Looked up again about once a second while drawn. The
default workspace's layout 1, without a saved default, starts with this panel.

- bedtime-automation: `status` reports `launch`, which App of the process this
  is; `bed.relaunched()` waits for it to change (the frame count alone raced
  right after startup).

## Build order

Each step depends on the one before.

1. Stores, backends, and simplified persistence (`Persist`, restore at startup,
   debounced saving).
2. Settings and layers, on top of stores. Replace bed's `config.rs` and
   `Settings` with registered settings.
3. Workspaces in bed: folder, `WorkbenchStore` binding, settings layer, session
   restore.
4. Commands and keybindings (one `Keybinding<C>` setting per command; dispatch by
   handler scope).
5. Menus and the toolbar.
6. Fonts: the theme's family, applied by bedtime-ui; installed fonts.
7. Automation: scripts drive the real app and take screenshots.

## Open questions

- How a setting restricts its layers (const list of store types vs a method).
- App-level defaults: letting the app change a setting's default (including a
  plugin command's default chords) without writing it into the user's global
  layer, e.g. `app.set_default::<Keybinding<C>>(chords)`.
- Format versioning: how stored world data and settings handle renamed types or
  fields (reflected type paths are part of the format today).
- Where errors from stores and settings are reported (error dialog, a settings
  panel status line, or both).
