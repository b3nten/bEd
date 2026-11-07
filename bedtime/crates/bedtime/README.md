# bedtime

Feature-gated re-exports for Bevy desktop applications.

| Feature | Crate | What |
| --- | --- | --- |
| (always) | `ui` (bedtime-ui) | UI toolkit, layouts of panels, window chrome |
| `persistence` | bedtime-persistence | Stores, the world snapshot, persisted resources |
| `documents` | bedtime-document | Document models, history, sources |
| `workbench` (default) | bedtime-workbench | The framework: documents, unsaved work, actions and keys, menus, settings, background work, `WorkbenchPlugins`. Also re-exported as `actions`, `menus`, `settings`, `work` |
| `workspaces` | bedtime-workbench | Workspaces |
| `native` | | macOS window chrome and menu bar |

