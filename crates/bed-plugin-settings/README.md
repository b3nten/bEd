# bed-plugin-settings

An optional generic frontend for the core-owned settings service. Install
`SettingsPlugin` in the owned application callback. It contributes
`bed.settings.panel`, commands and a toolbar entry and the default Primary+Comma
shortcut. It requires settings contracts and a workbench, without document or
bEd configuration dependencies.

The panel renders registered scalar preferences with Reset controls, plus
plugin-provided `SettingsSection` callbacks for structured values. Changes are
tracked transactional batches sent to the settings owner; receipts report
validation or persistence failures. The panel never writes settings files.

Its keybindings editor covers global commands, common focused actions and panel
actions from the registry. Overrides use the ordinary
`bed.workbench.keybindings` preference. An empty action list unbinds it;
removing the override restores defaults. Missing plugins' overrides remain
available when those plugins return. Conflicts are displayed, and recording
suppresses normal dispatch until a combination is captured or cancelled.

Applications may supply a custom settings frontend while continuing to use the
same definitions, snapshots, request ordering and persistent owner.

Custom Settings frontends can reuse `keybindings::KeybindingEditor`. It accepts
registered actions, panel labels and a caller-owned overrides draft, then returns
changes, errors and shortcut-input suppression. It performs no ECS mutation or
persistence; the frontend submits changes through its settings owner. Recording
ignores the opening frame and uses Escape to cancel unless explicitly assigned.
