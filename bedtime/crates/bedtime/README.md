# bedtime

Feature-gated re-exports for Bevy desktop applications.

| Feature | Crate | What |
| --- | --- | --- |
| (always) | `ui` (bedtime-ui) | UI toolkit, layouts of panels, window chrome |
| `persistence` | bedtime-persistence | Stores, the world snapshot, persisted resources |
| `settings` | bedtime-settings | Settings in layers, and the settings panel |
| `actions` | bedtime-actions | Actions, keybindings, shortcuts |
| `menus` | bedtime-menus | Menus, drawn with egui or natively |
| `work` | bedtime-work | Background work with cancellation |
| `documents` | bedtime-document | Document models, history, sources |
| `workbench` (default) | bedtime-workbench | The framework: documents, unsaved work, `WorkbenchPlugins` |
| `workspaces` | bedtime-workbench | Workspaces (needs the bevy_winit fork) |
| `native` | | macOS window chrome and menu bar |

See [docs/architecture.md](../../../docs/architecture.md).
