# bEd patches to bevy_winit 0.20.0

Upstream: https://github.com/bevyengine/bevy (published crate 0.20.0).
Original MIT and Apache 2.0 licenses are retained alongside this file.

**Relaunching an App in the same window.** bEd builds one App per workspace.
Switching workspace drops the App and builds the next one, but winit allows one
event loop per process and the window belongs to it, so the loop and native
windows outlive the App:

- `relaunch(world, make)` records the next App and writes `AppExit`, so the
  current App's exit systems (store saves) run in this update. At the runner's
  exit check (`WinitAppRunnerState::redraw_requested`) a pending relaunch drops
  the App, renderer included, then builds the next one with `make` and creates
  its windows.
- `WinitPlugin::build` reuses the running event loop's proxy and display handle
  (thread-local `EVENT_LOOP`) instead of building a second loop, and doesn't
  set a runner or a second Ctrl+C handler. `relaunching()` reports this, so the
  app can leave out process-wide setup such as `LogPlugin`.
- `despawn_windows` doesn't close the native windows on the exit a relaunch
  requested.
- `create_windows` adopts a native window no entity owns (left by the previous
  App) instead of creating one, taking its size, scale and position, and
  setting the new title.
- AccessKit adapters are dropped on relaunch; adopted windows have none yet.
- macOS: wgpu draws into a `CAMetalLayer` that `raw-window-metal` adds as a
  sublayer of the window's view and never removes. After dropping the old App,
  relaunch removes the view's `CAMetalLayer` sublayers, or the old renderer's
  last frame stays in the window (visible through transparency and blur).
  This is the one `unsafe` block, so `#![forbid(unsafe_code)]` became `deny`.
  Adds macOS-only `objc2`, `objc2-app-kit`, `objc2-quartz-core` and
  `raw-window-handle` dependencies.

Measured on an M4 Pro (2026-10-27): the next App draws about 0.08 s after the
request; memory stays flat over repeated relaunches. `tests/automation/relaunch.py`
checks the window keeps its size, the workspace layout comes back and no errors
are reported. Run it when upgrading this fork.
