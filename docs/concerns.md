# Concerns: what a plugin or app can contribute

An inventory of bed and bedtime as of 2026-11-22, to decide what the
framework should offer and how. `structure.diagram` beside this file has the
target layers. Each kind below has: what exists, how it is contributed today,
and the decision to make.

## Decided (2026-11-22)

- Settings lay themselves out from their declared sections. No app-side
  layout; an app that wants its own panel builds one from the settings.
  Done: `settings_panel!` and `set_settings_layout` are gone, bed only
  orders the sections.
- Documents and workspaces are opt-in tiers. Foundations plus the app tier
  (shell and services) is where a new app starts. Done for documents, as
  plugins: `AppCorePlugins` / `AppWindowPlugins` / `AppPlugins` are the app
  tier, `DocumentsPlugins` adds documents, and `WorkbenchPlugins` is both.
  Quitting is the app tier's (`exit`), and documents hold an exit through an
  `ExitGate`. They are Cargo features too: bedtime-workbench's `documents`
  and `workspaces`, and the same on `bedtime`, whose default is the app
  tier alone. Every app has a global store with the Global settings layer,
  and a session store (`<config>/session`); workspaces gives the session
  store a place per folder and adds the workspace's layer.
- bedtime's operations sit on one wrapper over `World`, not on extension
  traits of `World`. Done: `world.bedtime()` and `world.bedtime_mut()`
  (`BedtimeWorldExt`), with the parts on it by name (`layouts()`,
  `actions()`, `menus()`, `settings()`, `persistence()`) and opening panels,
  links and error dialogs. `&mut World` stays in every signature. bedtime-ui
  and bedtime-persistence used on their own keep their own traits: the
  wrapper is the workbench's, where nearly all the operations are.
- The cat and the frame-rate item become bedtime crates.
- bed becomes a library with a thin `src/bed.rs` binary, so an engine can
  build on it. Done: `bed::BedPlugins`, `bed::default_plugins()`,
  `bed::runtime`.

Step 6 (contributions), as decided:

- A panel the app opens by name is one call, `app.add_panel::<P>()`, with
  `impl ShowPanel for P { const SHOW: &str = "notes.show"; }`. It gives
  `Show<P>` (in the palette, with its keys) and an item in the View menu's
  one list of panels. It always opens a new panel.
- Action keys for opening panels are one scheme, `<panel>.show`
  (`settings.show`, `terminal.show`, `git_history.show`, `structure.show`,
  `todos.show`, `language_servers.show`, `theme_editor.show`, `bed.show`,
  `workspaces.show`). Custom keybindings saved under the old keys are
  dropped.
- Plugins add their own global status items; they show in the order plugins
  are added, as menu items do. Menu order stays plugin order.
- Titlebar tools are the app's: bed lists them (`tool::<GitPanel>(…)`).
- The status bar names the focused panel at its right, while there's room.

## The kinds, at a glance

| Kind | Count | Contributed with | Who arranges it today |
|---|---|---|---|
| Panels | 32 | `#[panel]` + `impl Panel` + `register_type` | plugin |
| Viewers (panel for a document type) | 12 | `register_viewer::<Model, Panel>(label).formats(..)` | plugin |
| Actions | 84 | `impl Action` + `add_action_handler` | plugin |
| Keys | 3 keymaps | `Action::DEFAULT_KEYS`, `add_keymap` | plugin defaults, app keymaps |
| Menu items | 76 | `add_menu_item::<A>(menu, group)` | plugin; order = plugin order |
| Titlebar tools | 5 | hand-written in `bed/titlebar.rs` | app |
| Dock menu | 1 | `add_dock_menu_item` | app |
| Settings | 36 | `settings!` + `register_setting` | plugin declares, app re-lists all |
| Global status items | 4 | `add_global_status(fn)` | app (plugins export the fn) |
| Panel status items | 5 | `add_panel_status::<P>(fn)` | plugin |
| Shell commands | 3 | `add_shell_command` | app |
| Persisted data | 4 resources + world snapshot | `persist::<T, Store>`, `reflect(Persist)` | plugin |
| Stores | 2 | `add_store` | workbench (workspaces) |
| App hooks | 9 | fn-pointer resources, `workbench_mut().set_*` | app |
| Modals and dialogs | 3 | `add_modal`, `request_error_dialog` | workbench |
| Lifecycle | 3 schedules + quit | `OnWorkspaceOpen/Closing/Close`, `CloseRequested` | workbench |
| Themes | 20 shipped | `ThemesPlugin { shipped, default }` | app |
| Notifications, background work | - | `notify`, `ManagedWork` | workbench |

## Panels (32)

| Crate | Panels | Opened by |
|---|---|---|
| workbench | Settings, Theme Editor | action + menu |
| code | Editor, Structure, TODOs, Language Servers, References | viewer; action + menu; References from Find References only |
| git | Git, History, Diff, Commit Diff | action + menu; the diffs from the Git panels |
| file-explorer | Files | action + menu + tool |
| terminal | Terminal | action + menu + tool |
| search | Search | action + menu + tool |
| notes, web, http-client, schedules | one each | action + menu |
| mascots | Bed, Duck | Bed: action + menu. Duck: `+ducky` only |
| viewers | Hex, Image, Audio, Video, Model, Font, Table, Markdown, Tree, Sqlite, Diagram | opening a file |
| bed | Workspaces | action + menu |

Every "show" panel repeats four calls: `register_type`, an `Action`, a handler
calling `open_new::<P>`, and a menu item. Five panels also have a titlebar
tool, which only bed can add.

**Decide:**
- One call (`add_panel::<P>()`) that makes the action, the menu item, the
  palette entry and an optional tool, or keep them separate.
- One place in the View menu for panels. Today they are split between
  "panels" (Git, History, Notes, Web, HTTP Client, Settings, TODOs,
  Structure) and "tools" (Terminal, Workspaces, Schedules, Theme Editor,
  Bed, Language Servers), with no rule behind the split.
- Whether titlebar tools are a plugin contribution the app orders, or stay
  the app's.
- One naming scheme. Today: `ShowGit` / `OpenSchedules` / `NewTerminal`;
  keys `git.show`, `code.show_structure`, `app.open_workspaces`,
  `settings.open`.

## Viewers (12)

Text Editor (text), Hex (any file), Image, Audio, Video, 3D Model, Font,
Table (CSV), Markdown Preview, Tree View (JSON, TOML), Database (SQLite),
Diagram. A viewer is a panel bound to a document; several may fit one file
(Open to the Side shows the other).

**Decide:** whether "document + viewers" is its own tier (apps without files
skip it), and whether a document type declares its own default extension and
"New" entry (today bed's Save As special-cases diagrams, and bed adds
New Diagram to the File menu).

## Actions (84)

| Owner | Actions |
|---|---|
| workbench (12) | New, Save, Undo, Redo, Close Panel, Toggle Maximize, Zoom In/Out/Reset, Open Settings, Theme Editor, Command Palette |
| code: editor (28) | 26 text commands (lines, carets, selection, comments, folding, brackets), Word Wrap, Format and Save |
| code: language (10) | Go to Definition, Find References, Rename, Code Actions, Format Document, Go to Symbol, Restart Language Server, show Structure / TODOs / Language Servers |
| file-explorer (6) | Files, Show Hidden, Show Ignored, view as Tree / Columns / by Panel Shape |
| search (3) | Search Workspace, Next / Previous Result |
| diagram (4) | New Diagram, Copy as Image, Export PNG, Export SVG |
| git (2), terminal, web, notes, http-client, schedules, mascots (1 each) | show the panel |
| bed (13) | Quit, New Window, Open Folder, Open Recent, Close Folder, Workspaces, Layout 1-5, Save Layout as Default, Reset Layout |

Keys: actions carry `DEFAULT_KEYS` (about 20 do); bed adds bedtime-code's VS Code
keymap and its own two chords for the palette; keybindings are also settings.

**Decide:**
- Quit, New Window, Open Folder / Recent / Close Folder and the layout
  slots are generic; they move into the framework (app tier and workspaces).
- Some toggles exist twice, as a setting and as an action (hidden files,
  ignored files, word wrap). A setting could offer its toggle action itself.
- Whether default keys live on the action, in a keymap, or both (today
  both).

## Menus (76 items)

Standard menus and groups: App (about, settings, quit), File (new, open,
save, close), Edit (history, clipboard, find, code), View (panels, editor,
zoom, tools, commands), Window, Help. Custom: Selection (code, shown only
for an editor, 19 items), Layout (bed, 7 items). bedtime-code alone adds 37
items. Items appear in the order their plugins were added.

**Decide:**
- Whether order comes from plugin order (today), a declared priority, or an
  app-side arrangement like the settings layout.
- Whether the Layout menu and the File menu's folder items come with the
  workspaces tier.

## Settings (36)

| Owner | Settings | Declared section |
|---|---|---|
| workbench | theme, font, font size, rainbow caret | Appearance |
| workbench | window opacity, window blur | Window |
| workbench | panel opacity, corner radius, gap | Panels |
| workbench | autosave (key `editor.autosave`) | Documents |
| workbench | reopen last folder | Startup |
| code | wrap, line numbers, highlight occurrences, format on save, trim trailing whitespace, final newline, breadcrumbs, auto-close brackets | Editor |
| code | language servers (edited in its own panel) | - |
| file-explorer | show hidden, show ignored | Files |
| web | draw web pages | Web |
| bed | effects on + 8 intensities | Appearance |
| bed | backdrop, cat, cat colour, frame rate | Window |

Settings show in their declared sections; a setting declared outside a
section (language servers) isn't in the panel. bed orders the sections with
`settings_sections`.

## Status bar

Global items (bed orders them): workspace name, git branch, language
servers, frame rate. Panel items (for the focused panel): editor position
and language, the file's language server, terminal, SQLite, HTTP client.
The bar itself is `status::body`, which bed sets as the window's body.

**Decide:** plugins add their global item themselves (app may reorder or
hide), as panel items already work; the status bar comes with the shell.

## Shell commands (3)

`+open` / `+o` (files, or a folder as the workspace), `+web`, `+ducky`. All
three are in bed because each joins two plugins (terminal with workbench,
web, mascots).

**Decide:** stay in bed's glue, or `+open` moves beside the terminal plugin
as the generic "open in this app".

## Persistence

| What | Where | Mechanism |
|---|---|---|
| Layouts, areas, panels and their state | workspace store, `world.ron` | world snapshot of `reflect(Persist)` types |
| Notes, HTTP requests, workspace note | workspace store | persisted resource |
| Recent folders, layout defaults | global store | persisted resource |
| Settings | global and workspace layers | settings layers |
| Themes, backdrops | folders under the config directory | files |

Saved types are named by type path, so moving a type breaks saved layouts
unless pinned.

**Decide:**
- Done: two scopes always exist, `GlobalStore` (app-wide) and
  `WorkbenchStore` (the session's, which workspaces repoint), so a plugin
  says which it means without knowing about workspaces.
- A stable name per saved type, chosen once (not the module path).
- For the engine: snapshot by scope (a marker, like `InWorkspace`) as well
  as by type, for play / stop.

## App hooks (9)

| Hook | Question it answers | Set by bed to |
|---|---|---|
| `FileDrops` | what dropped files do | open files; a folder becomes the workspace |
| `SaveAs` | where an untitled document is saved | the system picker |
| `set_new_document` | what New makes | a text document |
| `LayoutFallback` | which layout a panel goes to when none is open | layout 1 |
| `ExternalOpener` | what a link opens in | the web panel, else the system |
| `GitHost` | open folder / open terminal from the Git panel | switch workspace / a terminal there |
| `ActionFocus`, `PanelModified`, `SettingsLocation` | framework internals | (workbench sets these) |

Also `register_document_handlers()`, which the app must remember to call for
Save, Undo and Redo to work.

**Decide:**
- Which of these get a framework default so an app sets nothing (the save
  picker, file drops, document handlers).
- Shared requests in the framework ("open this folder", "open a terminal
  here", "open this URL") that plugins send and other plugins answer, in
  place of `GitHost` and `ExternalOpener`.
- One style for the rest. Single-slot fn pointers cannot be layered when the
  engine builds on bed.

## The second app

`bedtime/examples/hello` is an app that depends on `bedtime` and Bevy only:
one panel of its own (a counter), a setting, a status item, an action and a
menu item, with the settings panel, the command palette, menus, a status bar
and saving between runs. `tests/automation/hello.py` drives it.

It is 111 lines, all its own feature: `AppPlugins::new("Hello")` is the app
from its name (Bevy's plugins with its window, the UI, the workbench in its
config folder, the settings panel, the palette, Quit, and the window's
panels with a status bar), and `CounterPlugin` is the counter, which also
says what a new window starts with.

What was by hand, and where it went:

| What the app did by hand | Now |
|---|---|
| Configured Bevy's window so the workbench decides when it closes | `AppPlugins` |
| Gave its name for the menu bar, the config folder and the window title | once: `AppInfo` |
| Put the layout surface, the status bar and a camera on the window | `ShellPlugin` |
| Added the settings panel and the command palette beside the workbench | in the group |
| Opened its window unfocused for scripts | `AutomationPlugin` does it |
| Added a store and made it a settings layer | every app has `GlobalStore` (with the Global settings layer) and a session store |
| Defined Quit: the action, its key, its handler, its menu item | `Quit`, in `ShellPlugin` |
| Listed bedtime's `native` feature | a default of `bedtime` |

Still the app's: adding `AutomationPlugin` (automation depends on the
workbench, so the group can't have it), and listing Bevy's features.

Documents are the app's choice: `AppPlugins` has none, and an app whose
panels show documents adds `DocumentsPlugins` (as bed does). hello has no
Documents section in Settings and no New, Save, Undo or Redo. On platforms
without a native menu bar it has no menus at all: drawing them is
`Titlebar`'s, which bed sets up itself.

Closed while writing it: `#[panel]` and `#[derive(Persisted)]` named
`bedtime_ui` and `bedtime_persistence`, so they failed in an app with only
`bedtime` (they now use `bedtime::ui` and `bedtime::persistence` there);
`bedtime::automation` is re-exported; the workbench built with warnings
without `workspaces`.

The target, for the same app:

```rust
fn main() -> AppExit {
    App::new()
        .add_plugins((bedtime::AppPlugins::new("Hello"), CounterPlugin))
        .run()
}
```

with `CounterPlugin` saying what a new window starts with.

## In bed today, by where it should end up

| bed module | Lines | Goes to |
|---|---|---|
| `lib.rs`, `window.rs`, `workbench.rs` | 190 | `BedPlugins` stays (done: bed is a library, `src/bed.rs` the app); the window setup goes to a shell plugin in the app tier |
| `runtime.rs` | 80 | app tier, opt in, set by the binary |
| `menus.rs` | 210 | app tier (Quit, New Window), workspaces (folders, recent) |
| `plugins/documents.rs` | 85 | documents tier (save picker) |
| `plugins/files.rs` | 65 | documents tier (drops, handlers); keymap stays |
| `workspace/layouts.rs` | 205 | workspaces tier; bed keeps its default arrangement |
| `workspace/panel.rs` | 340 | workspaces tier |
| `titlebar.rs` | 120 | shell (tools become contributions) |
| `perf.rs` | 360 | a small bedtime plugin, or stays |
| `themes.rs` + 20 theme files | 75 | stays (bed's), or a shared theme pack |
| `settings.rs` | 40 | mostly disappears with declared sections |
| `plugins/{git,search,mascots,terminal}` | 440 | stays: glue between plugins |
| `visuals/` | 6,400 | stays: bed's look (effects and shader backdrops could move later) |
