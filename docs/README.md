# bEd and bedtime, in pictures

Diagrams in bEd's own format: open them in bEd (they're editable).

| File | Shows |
|---|---|
| `structure.diagram` | The layers as built: the apps, bed (the library), bedtime's plugins, the framework's three tiers, the foundations, and the `bedtime` facade. |
| `app.diagram` | How an app is put together: what it writes, and the plugin groups that makes up (`AppPlugins`, `AppWindowPlugins`, `AppCorePlugins`), with what it can add. |
| `plugin.diagram` | What a plugin adds with one call each (a panel, a viewer, an action, a setting, status, something saved), and where each shows. |
| `saved.diagram` | What is saved and where: the two stores, with and without workspaces, and the files under the config folder. |
| `window.diagram` | A window: the titlebar, areas and tabs, panels, and the status bar's four parts, and who draws each. |

`concerns.md` is the inventory behind them: every kind of contribution, the
decisions taken, and what moved from bEd into bedtime.
