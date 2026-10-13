# bed-plugin-idle

`IdlePlugin` supplies optional desktop idle behavior as ordinary Bevy systems.
It observes native and injected input before normal desktop input routing.
The gesture that wakes the window is consumed through its release, including
IME composition; focus changes clear held wake gestures.

Configure `IdleSettings` for the delay and enablement. The application's UI sets
`eligible` to its focused viewport ID and calls `draw_ui(world, ui)` for the idle
overlay. With the `mascots` feature and `DesktopCanvasPlugin`, the overlay uses a
registered mascot canvas from the same app and GPU; the default feature set needs
no mascot renderer.
