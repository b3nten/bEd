# bEd and bedtime, in pictures

Diagrams in bEd's own format: open them in bEd (they're editable).

| File | Shows |
|---|---|
| `structure.diagram` | The layers as built: the apps, bed (the library), bedtime's plugins, the framework's three tiers, the foundations, and the `bedtime` facade. |
| `app.diagram` | How an app is put together: what it writes, and the plugin groups that makes up (`AppPlugins`, `AppWindowPlugins`, `AppCorePlugins`), with what it can add. |
| `plugin.diagram` | What a plugin adds with one call each (a panel, a viewer, an action, a setting, status, something saved), and where each shows. |
| `saved.diagram` | What is saved and where: the two stores, with and without workspaces, and the files under the config folder. |
| `window.diagram` | A window: the titlebar, areas and tabs, panels, and the status bar's four parts, and who draws each. |

How it works, at run time:

| File | Shows |
|---|---|
| `frame.diagram` | One frame, in order: input, keys a panel claims, the one egui pass that draws everything, layout edits, the rest of the keys and the action queue, then plugins' systems, rendering and saving. |
| `actions.diagram` | Actions: from a key (its two passes), a menu, the palette, a script or code, through one queue and dispatch, to a panel's handler or the app's; and what a plugin declares (keys, menu items). |
| `panels.diagram` | Panels and layouts as entities (layout ← area ← panel), placing a panel, gestures becoming layout edits, and what "focused" means. |
| `documents.diagram` | Documents: their parts (source, document entity, history, panels), then opening, editing, saving, changes on disk, and closing. |
| `settings.diagram` | Settings: declared, layered, resolved, applied; edited in the Settings panel, saved, and reloaded when changed on disk. |
| `session.diagram` | A session: starting, switching to another folder, and quitting, step by step. |

`concerns.md` is the inventory behind them: every kind of contribution, the
decisions taken, and what moved from bEd into bedtime.
