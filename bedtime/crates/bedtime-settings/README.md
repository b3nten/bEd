# bedtime-settings

Typed settings resolved through layers, each layer kept in a
[`bedtime-persistence`](../bedtime-persistence/README.md) store. With the `ui`
feature, settings declared in a section describe how they're edited, and the
crate draws them.

```rust,ignore
use bedtime_settings::*;

settings! {
    "Files" {
        /// File tree indentation, in points.
        pub FileTreeIndent: f32 {
            default: 16.0,
            key: "filetree.indent",
            ui: slider(0.0..=40.0).decimals(0),
            apply: |world, indent| world.resource_mut::<FileTreeConfig>().indent = *indent,
        }
    }
    pub ReopenLast: bool { default: false, key: "startup.reopen_last", only_in: [GlobalStore] }
}

app.add_plugins(SettingsPlugin)
   .add_settings_layer::<GlobalStore>()      // lower precedence
   .add_settings_layer::<WorkspaceStore>()   // overrides global
   .label_settings_layer::<WorkspaceStore>("Workspace")
   .register_setting::<FileTreeIndent>()
   .register_setting::<ReopenLast>();

let indent = world.settings().get::<FileTreeIndent>();
world.settings_mut().set::<FileTreeIndent>(StoreId::of::<GlobalStore>(), 18.0)?;
```

## Settings and layers

- **Layers** stack in the order the app adds them; later layers win. A layer
  holds only what was set in it, so a missing value inherits from the layers
  below, then the setting's default. The crate attaches no meaning to layers.
- **`apply`** runs when a setting's resolved value changes: after `set` or
  `clear`, once for every registered setting when layers load at startup, and
  when a layer's file changes on disk. Plugins don't need to know about
  settings: they expose config resources, and the app's settings write to them.
- **Reading.** `world.settings()` is the `Settings` resource: `get` (resolved),
  `source` (which layer it comes from), `get_in` (one layer's own value),
  `get_at` (a layer and those below: what an editor for that layer shows),
  `inherited` (what a layer would have without its own), and
  `overridden_above` (the higher layer whose different value is used instead).
- **Changing.** `world.settings_mut().set::<S>(layer, value)` and
  `.clear::<S>(layer)`. Setting the value a layer would inherit anyway clears
  the layer instead, so it keeps following the layers below. `only_in` limits
  which layers may hold a setting.
- **Storage.** A layer is its store's `settings` entry, a RON map from key to
  value (`settings.ron` in a `DirectoryStore`). Values are kept as raw RON, so
  settings that aren't registered (from a plugin that isn't loaded) are
  preserved. A layer without a backend lives in memory only. Layers load before
  the world snapshot (`SettingsSystems::Load`), so restored entities see final
  values. Changed layers save about once a second and on exit; a layer whose
  file changes on disk is reloaded, and the last writer wins.
- **Problems.** A value that doesn't decode falls through to lower layers and is
  reported (`SettingsError`), not deleted. A layer file that can't be read or
  saved is a `StoreError` and is never written over.
- **The unit of override is the setting.** Anything overridable per entry is
  split into one setting per entry with a derived key (`Setting::key()` is a
  function, so generic settings can compute it).
- `read_setting::<S>(&backend)` reads one setting straight from a store's
  backend, for decisions made before the app is built.

## `settings!`

Each setting is a block of entries; `key` and `default` are required, and a
missing or misspelt entry is a compile error. Settings inside a quoted section
are shown in the settings panel; others aren't.

| Entry | Meaning |
| --- | --- |
| `default`, `key` | The value without any layer, and its name in a layer's file |
| `apply: \|world, value\| …` | Runs when the resolved value changes |
| `only_in: [StoreA, StoreB]` | The layers that may hold it |
| `label`, `description` | Its label (default: the name in words, `FontSize` is "Font size") and hover text |
| `ui` | `toggle()`, `text()`, `drag()`, `slider(range).suffix(" pt").decimals(1)`, `choice()`, or `custom(row)` |
| `visible: \|world, layer\| …` | Whether its row is shown (only while another setting is on, say) |

Without `ui`, the editor comes from the value's type: a toggle for `bool`, a
text field for `String`, a drag value for numbers, a combo box for a fieldless
enum that derives `Reflect`. A hand-written `impl Setting` needs only `key` and
`default`.

## The settings panel (`ui` feature)

`ui::layer_tabs` shows a tab per layer (when there's more than one) and returns
the one being edited; `ui::sections` draws every section with a row shown for
it. Sections come in the order they're declared, or the order given to
`app.settings_sections([...])`. Each row shows the layer's own value, a warning
when a higher layer overrides it, and a button that clears the layer's value.
An app wanting something else (search, another layout) reads the `SettingsUi`
resource and draws the rows itself, using `ui::row`, `ui::columns` and
`ui::Marks`.

```sh
cargo test -p bedtime-settings --features ui
```
