# Settings

Typed, layered settings backed by
[`bedtime-persistence`](../../bedtime-persistence/README.md) stores. The `ui`
feature renders editors for settings declared in sections.

```rust,ignore
use bedtime_workbench::settings::*;

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

## Whose settings

Each setting belongs to the crate whose behaviour it changes, which declares
and registers it: workbench's appearance (`appearance`, in `AppearancePlugin`)
and `Autosave` (in `DocumentSettingsPlugin`), the editor's in bedtime-code,
the file tree's in bedtime-filetree. An app adds its own the same way.

An app changes a plugin's default with
`app.set_setting_default::<FontSize>(13.0)`: the setting is identified by type,
the latest call wins, and it works before or after the setting is registered.
Stored values still win over it, and clearing one goes back to it.

## The settings panel (`ui` feature)

`SettingsPanelPlugin` adds `SettingsPanel` and `OpenSettings`
(`settings.open`, Cmd/Ctrl+,): layer tabs, where the chosen layer is saved
(`SettingsLocation`, which workspaces sets), its errors, every setting, then
the keyboard shortcuts.

Without a layout, settings show in the sections they were declared in, in the
order given to `app.settings_sections([...])`, then the order their first
setting was registered. An app lays the panel out itself with
`settings_panel!`, listing settings by type from any plugin; a setting it
leaves out isn't shown (an app may set some itself):

```rust,ignore
app.set_settings_layout(settings_panel! {
    "Appearance" { ColorTheme, FontFamily, FontSize }
    "Editor" { bedtime_code::WrapLines, Autosave }
});
```

`ui::layer_tabs` shows a tab per layer (when there's more than one) and returns
the one being edited; `ui::sections` draws the sections with a row shown for
it. Each row shows the layer's own value, a warning when a higher layer
overrides it, and a button that clears the layer's value. An app wanting
something else (search) reads the `SettingsUi` resource and draws the rows
itself, using `ui::row`, `ui::columns` and `ui::Marks`.

```sh
cargo test -p bedtime-workbench --features persistence settings::
```
