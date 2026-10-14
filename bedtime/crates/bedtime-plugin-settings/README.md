# bedtime-plugin-settings

An optional settings frontend for the core-owned preference service. Install
`SettingsPlugin` in the application composition. It contributes the Settings
panel, commands, toolbar entry, and Primary+Comma shortcut.

Each provider registers its own named section through
`FeatureRegistrar::settings_ui` or `settings_page` and renders its own controls.
The panel sorts sections alphabetically by display name, shows sidebar navigation,
and fades the selected section according to the interface animation preference.
It does not generate controls from definitions or combine providers' sections.
`bedtime_ui::settings` supplies consistent, responsive label/control rows.

Draft callbacks return edited values; page callbacks receive the panel entity,
shortcut input and settings snapshot and return explicit `SettingsChange` values.
The panel submits tracked transactional batches and displays validation or
persistence errors. It never writes preference files. There are no reset buttons.

Core publishes Appearance, Documents, and Panels and Keybindings sections under
their respective preference owners. The reusable shortcut editor lives in
`bedtime_workbench::keybindings`; its provider manages recording and shortcut
suppression. Viewer associations preserve unavailable viewer IDs.

Applications can supply a global header through `SettingsPanelHeader` and their
own section callbacks. bEd publishes its theme controls and window blur in its
own section, plus Open Settings JSON in the header. `SettingsPanelLayout` remains
available to restrict which registered sections are shown.
