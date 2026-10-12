# bed-desktop

Bevy owns the native window, device, surface, submission and presentation. A
single window camera runs `DesktopGraph` after offscreen preview cameras. Dear
ImGui `Context` remains on the UI thread; only owned `FrameSnapshot` values,
geometry observations and GPU handles cross extraction.

```rust,no_run
use bed_desktop::{bevy::prelude::*, dear_imgui_rs::Ui, DesktopPlugins, DesktopUi};
fn draw(world: &mut World, ui: &Ui) { ui.text("A desktop without editor modules"); }
fn main() {
    App::new().add_plugins(DesktopPlugins::default())
        .insert_resource(DesktopUi(draw)).run();
}
```

The ordered `PreUpdate` sets are `Gpu → InjectInput → InterceptInput → Input →
Configure → Prepare → Ui → AfterUi → RouteInput`, followed by Bevy `InputSystems` and gameplay. Native
interceptors are ordinary systems in `InterceptInput`, borrowing the installed
`DesktopContext`, `DesktopInputEvents` and `DesktopInjectedInputEvents`. Events
can prevent ImGui delivery (`consumed`) or just gameplay delivery
(`gameplay_consumed`). Routing uses the current frame's keyboard/pointer capture
for typed and aggregate Bevy messages; releases and focus changes remain ordered.

`DesktopInputRequests::submit(Vec<DesktopInputStep>)` queues explicit frame
steps. Each step holds semantic mouse, key, modifier, text and line-scroll
events. Pointer positions are logical window coordinates. Keys carry physical
and logical identities and do not generate text: use `Text` once for characters.
Held synthetic keys/buttons/modifiers are released in a final UI frame before
`DesktopInputResults::take(id)` returns a receipt with `last_ui_frame`. Cleanup
preserves concurrently held native inputs. Focus loss/cancellation returns an
error after cleanup. Sequences pause while a UI frame cannot be produced.

Applications project configuration into `DesktopAppearance` and
`DesktopNativePresentation` in `DesktopSystems::Configure`. Preparation applies
appearance and native presentation before the frame opens, while
`DesktopContext` stays installed. `DesktopFonts` is an alias for the portable
main-thread `bed_ui::font::UiFonts`; it owns font handles, resolved sources,
warnings and atlas generation. Palette changes preserve the atlas. Font changes
rebuild it and advance the generation before feature preparation. Paper Mono is
embedded, so the default base does not discover bEd configuration or assets.
`DesktopFrontend` can own the coherent prepare/draw transaction instead; it must
close its frame into `DesktopFrame.snapshot`. The `Ui` and frame token are never
resources. The context is temporarily scoped out only for this UI transaction.
After drawing, `DesktopFrameInfo` records its monotonically increasing frame ID,
device generation, physical size, display origin, scale and geometry.

Hosts publish observed body geometry with `DesktopFrame::publish_panel`, using
an entity as the target token. Bounds and ancestor clips use logical ImGui
coordinates. The host owns layout; the desktop owns an immutable copy alongside
the snapshot. Geometry resets before each frontend draw.

`DesktopCaptureRequests::submit(targets, deadline, after_frame)` captures all
window/panel targets from one rendered frame. `after_frame` is an exclusive lower
bound. `DesktopCaptureResults::status(id)` returns pending/complete or `None`
for an unknown/consumed ID; `take(id)` returns an error-valued completed batch.
Each image contains its physical crop rectangle and packed `OwnedRgba` pixels,
with PNG and PPM exporters. Panel crops intersect content, ancestor clips and
the window. They include overlays/effects visible in those final pixels; hidden
panels produce an explicit error. Native title bars and the OS cursor are outside
this composition. Panel capture does not focus or reveal a hidden panel.

A copyable final composition texture supplies both native presentation and
readback. The copy precedes presentation; mapping starts after Bevy submits the
commands. Completion means CPU pixels have arrived, rather than merely a frame
being recorded. If an occluded native surface cannot present, requested captures
still compose current producers into the owned final texture; this does not
advance the presented-frame counter. Missing window dimensions or configured
formats retain requests until their deadlines. Device
replacement invalidates old attempts and retries on a fresh frame; old callbacks
cannot complete a replacement attempt. Mapping failures remain values and retry
until the deadline. `DesktopScreenshotRequest`/`DesktopScreenshotResult` retain
the legacy path/PPM interface through this same capture path.

`DesktopGpu` appears after `RenderStartup`; its generation changes on recovery.
Rebuild GPU caches and register new texture views on generation changes.
Pipelined rendering is disabled. `DesktopComposition` can add application effects;
it writes the supplied final output and appends commands to Bevy's
`PendingCommandBuffers`. Unrendered snapshots reconcile in render cleanup.

`DesktopPlugins` installs the default neutral `DesktopCanvasPlugin`. It consumes
`bed_ui::gpu::{CanvasOutput, CanvasRenderer, CanvasVisible}`, owns target
textures, and publishes `CanvasTextures`. Preparation/retirement run after
`DesktopSystems::Prepare` and before UI; visible registered renderers run after
UI. Applications order their producer preparation before
`DesktopCanvasSystems::Prepare`. Resize retains texture IDs, and old IDs retire
only after the previous snapshot is consumed. Renderer panics are quarantined
on that presentation's `CanvasFailure`; siblings continue. `DesktopCanvasErrors`
provides errors for the application to display. Optional `bed_ui::icons::Icons`
are borrowed in place and changed images upload on the same device.

Install the base on a fresh `App`, after any plugins that register early asset
sources and before application feature plugins. It installs Bevy's window,
renderer and runner; adding another `DefaultPlugins` group is unsupported.
Application resources supplied before installation are preserved. The group
contains focused appearance, native and canvas plugins that can be configured
or disabled through Bevy's plugin-group API.

`DesktopNativePresentation` contains owned menus, toolbar and Dock command
models. Native objects initialize after the real primary window exists.
Native chrome preserves Bevy's window visibility, focus, decorations and
resizability. Borderless windows have no native toolbar or titlebar inset.
`DesktopNativeActions` supplies command IDs and click/keyboard/modifier metadata
before input interception. `DesktopContentInsets` reports chrome geometry;
`DesktopNativeCapabilities` reports platform support and clipboard availability.
ImGui receives a native text clipboard backend. Cloneable main-thread
`DesktopFileClipboard` exposes path operations independently of editor APIs.
Native integration never replaces Winit's view or delegate.
Window visibility, focus, decorations and resizability remain under Bevy/Winit
control. Borderless windows have no native titlebar controls or content inset.

The default close policy accepts native Quit and window-close requests. Set
`close_when_requested: false` to let application systems resolve unsaved state
and emit `AppExit` themselves. Native mechanisms do not bypass that policy.
Menus/toolbars/Dock integration currently follow macOS support; Linux retains
its native drag-pointer helpers and clipboard integration. The base emits native
and appearance failures as resource values for the application to report.

Native proofs (run sequentially on a desktop):

- `cargo run -p bed-desktop --example native_smoke`: canvas sampling/gamma, PPM,
  skipped frames, resize and two actual device replacements.
- `cargo run -p bed-desktop --example automation_smoke`: real injected UI events,
  held-input cleanup, stamped window/panel pixels, clipping/scale, PNG, recovery
  and fresh captures without an acquired native surface.
- `cargo run -p bed-desktop --example canvas_smoke`:
  registered targets/icons, resize, isolated panic, retirement and recovery.
- `cargo run -p bed-desktop --example window_config_smoke`: hidden startup,
  non-resizable/borderless windows and runtime decoration changes.
- `cargo run -p bed-desktop --example window_config_smoke`: hidden/unfocused,
  non-resizable and borderless window configuration, native presentation, and
  decoration/visibility changes on the same window.

`cargo test -p bed-desktop --lib --all-features` covers input routing, focus,
capture cropping, stale callbacks, deadlines and export validation without a GPU.
