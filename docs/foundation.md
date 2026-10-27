# Foundation: stores, persistence, settings, workspaces, commands

Status (2026-10-27): **step 1 (stores and persistence) built**; the rest is the
agreed design, not built yet. Update this file as decisions change during the build.

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

- **One workspace per process.** Opening another folder starts a new instance.
- A workspace is a folder plus `WorkbenchStore` bound to
  `~/.config/bed/workspaces/<hash of path>/` (as bed_old did, so repos aren't
  touched). Opening one: bind the store, add its settings layer, restore the
  session world.
- Built in bed first; moved to an optional bedtime plugin once its shape settles,
  if it turns out to be reusable.
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
struct Keybinding<C: ContextCommand>(PhantomData<C>);
impl<C: ContextCommand> Setting for Keybinding<C> {
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

## Open questions

- How a setting restricts its layers (const list of store types vs a method).
- App-level defaults: letting the app change a setting's default (including a
  plugin command's default chords) without writing it into the user's global
  layer, e.g. `app.set_default::<Keybinding<C>>(chords)`.
- Format versioning: how stored world data and settings handle renamed types or
  fields (reflected type paths are part of the format today).
- Whether bed needs a lock so the same folder can't be open in two instances
  (bed_old had an OS lock).
- Where errors from stores and settings are reported (error dialog, a settings
  panel status line, or both).
