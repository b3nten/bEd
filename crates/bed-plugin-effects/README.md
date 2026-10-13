# bed-plugin-effects

`EffectsPlugin` supplies optional desktop composition effects. With its `desktop`
feature it registers and applies preferences under `bed.effects`; applications
choose their own defaults through the shared settings service. The plugin owns
numeric validation and the Off, Sharp, Legacy and Custom preset interpretation.
Custom values remain available when selecting another preset. Legacy enables the
configured values, while Sharp uses the subtle preset.

Without desktop features, `settings::ShaderSettings` and `settings::EffectPreset`
remain renderer-independent values for applications and settings frontends. The
`gpu` feature exposes the shader renderer without requiring a workbench.
