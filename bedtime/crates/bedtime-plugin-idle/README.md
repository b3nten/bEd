# bedtime-plugin-idle

`IdlePlugin` supplies optional desktop idle behavior as ordinary Bevy systems.
It observes native and injected input before normal desktop input routing.
The gesture that wakes the window is consumed through its release, including
IME composition; focus changes clear held wake gestures.

The plugin registers `bedtime.idle.bedtime_enabled` and
`bedtime.idle.bedtime_delay_seconds` with the shared settings service and applies them
to `IdleSettings`. Applications can choose defaults during launch. Without a
settings service, applications may configure `IdleSettings` directly. The plugin
registers its own foreground workbench overlay and derives eligibility
from the focused desktop viewport. Applications do not need a drawing callback.
Custom frontends may still set `eligible` and call `draw_ui(world, ui)` directly.
With the `mascots` feature and `DesktopCanvasPlugin`, the overlay uses a
registered mascot canvas from the same app and GPU; the default feature set needs
no mascot renderer.
