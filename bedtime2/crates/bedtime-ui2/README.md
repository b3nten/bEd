# bedtime-ui2

ImGui lifecycle, Bevy rendering, reusable controls, themes and motion. Shared
application state, surfaces and theme selection live in ECS. The private runtime
owns ImGui, frame lifetime and control animations. There is no dependency on the v1 framework or
on workbench.

Run the standalone gallery:

```sh
cargo run -p bedtime-ui2 --example gallery
```

`cargo run -p bed2` presents the same gallery with native menus and typed command
buttons, plus a spinning-cube viewport. The standalone gallery needs no workbench
plugin or 3D renderer.

## Host integration

Install Bevy's window, renderer, input, assets and Winit plugins before `UiPlugin`.
The application owns those plugins, its window, camera and close policy. UI uses
the existing GPU device and does not create another surface or event loop.

```rust,ignore
app.add_plugins((DesktopPlugin, UiPlugin));

let dark = world.spawn(themes::dark()).id();
world.entity_mut(window).insert((UiSurface, UsesTheme(dark)));
world.spawn((Camera2d, CameraRenderGraph::new(UiGraph)));

let view = register_view(world, window, draw_form);
world.entity_mut(view).insert(FormState::default());
```

Drawing handlers are ordinary Bevy systems with borrowed `UiFrame` input:

```rust,ignore
fn draw_form(frame: UiFrame, mut forms: Query<&mut FormState>) {
    let mut state = forms.get_mut(frame.view).unwrap();
    frame.ui.window("My form").build(|| {
        let controls = frame.controls();
        controls.row("Name", || { controls.input("##name", &mut state.name); });
        controls.checkbox("Enabled", &mut state.enabled);
    });
}
```

The frame supplies its view/surface entities, borrowed ImGui access and immutable
prepared theme. It cannot outlive the active frame. Queries, resources and
deferred commands work normally. Systems execute sequentially in `SurfaceViews`
order; Bevy's relationship APIs support reordering and reparenting.

State used only by a drawing system can use ordinary Bevy `Local<T>` or a local
struct. Controls accept borrowed values; using a checkbox or animation never
requires spawning a control entity. Use components when other systems need to
read or modify that state, as the gallery does for its externally controlled form.

`OnSurface` owns views and `InView` owns application-defined parts or action placements.
Removing a surface recursively removes these entities. `UsesTheme` references a
theme without owning it; several surfaces can reference the same theme data,
although stage 2 renders only one active surface/context at a time.

`UiCapture` and `UiDiagnostics` live on the surface. Capture reports keyboard,
pointer and text input needs; consumers must honor it when admitting application
input. It does not clear Bevy's global input state. Capture produced during drawing
is available after `UiSystems::Draw`; earlier systems see the preceding frame.

## Themes and controls

`themes::light()` and `themes::dark()` return ordinary `Theme` values based on
Everforest Medium. Applications can use, modify or replace them. UI assigns no
stable keys and manages no theme-selection storage. Motion policy is independent
of the selected theme. Attribution is in `assets/everforest-LICENSE.txt`.

The runtime validates requested theme values before preparing presentation.
Resolved colors and dimensions are applied only when the selected values change.
Ordinary controls inherit this base style; variants push only their differences.
Changing colors or spacing does not rebuild fonts. The bundled Paper Mono regular
face supplies the initial typography and uses ImGui's dynamic DPI rasterization;
font-size changes rebuild the atlas. Its OFL license is included with the font.

Built-in helpers cover buttons with automatic hover motion, primary/disabled variants, SVG icon
buttons, checkboxes, animated toggles, text fields, sliders, selectors, form rows,
headings, feedback, tooltips, context menus, dialogs, disclosures and clipped lists.
ImGui remains available for specialized drawing and layout.

Context menus inherit the prepared base palette and use v1's file-tree popup
metrics: 8×6 padding, 6×4 item spacing, seven-point rounding and a one-point border.
Tooltips use 12×10 padding. Selectors leave a four-point gap and open above their
control when necessary. These dimensions are theme metrics. Native window backing
color follows the theme, filling transparent macOS title bars.
Closed menus do not construct a style scope or invoke their contents closure. `Controls::list` clips fixed-height
rows before invoking the caller's row painter. Rows must have a consistent height.
Custom icon/toggle painters honor the current style alpha, including disabled and
fading scopes.

Motion is private UI-local state keyed by drawing view, ImGui control identity and
animation kind. Use stable ImGui labels/ID scopes when reordering repeated controls.
The cache follows submitted controls, drops retired views and absent entries, and
never allocates popup state for closed row menus. Helpers own interpolation:

```rust,ignore
controls.icon_button("add", BuiltinIcon::Plus);
controls.tooltip("Add an item");
controls.toggle("Enabled", &mut state.enabled);
controls.disclosure("Advanced", &mut state.advanced_open, || {
    controls.text("The body is measured automatically.");
});
```

The surface's `MotionPolicy` controls duration and reduced motion. Motion uses
Bevy's real time independently of paused/scaled simulation time and requests
redraws while visible animations are active. Retargeting starts at the displayed
value; disabled motion settles immediately. Hidden controls stop requesting redraws.
Disclosures measure their body and stop accepting input while closing.

Tooltips grow from the source control's anchor and context menus from their
opening anchor, using the same 0.96 starting scale as v1. Both fade in; dialogs
fade without scaling. One explicit frame-owner pass transforms final vertices
and clip rectangles, including child windows and custom painting. Layout and hit
testing remain at their final coordinates. Interaction settles popup scale so
visual motion does not interfere with choosing a menu item. No native pointers
are stored in the cache and no native animation hook maintains a parallel window map.

## Icons and runtime packs

`Icon` is a validated monochrome SVG asset. `IconSet` is an asset containing partial
overrides of the fixed `BuiltinIcon` vocabulary. The bundled set contains 129
icons, including v1's Lucide and Simple Icons artwork, with retained attribution
and licenses in `assets/icons`. The default set is embedded and works offline.

Applications select a pack by attaching `UiIcons(Handle<IconSet>)` to a UI surface.
Removing that component restores the bundled set. Pack selection is independent
of the color theme. Unspecified slots inherit bundled artwork; overrides never
accumulate from previously selected packs.

For example, load `assets/my-pack.icons.json` through Bevy's configured asset source:

```json
{
  "folder": "folder.svg",
  "plus": "plus.svg"
}
```

Paths resolve relative to the manifest, including for custom Bevy asset sources.
Unknown keys and malformed SVGs fail loading. The application can switch packs
at runtime through ordinary ECS operations:

```rust,ignore
let pack: Handle<IconSet> = asset_server.load("my-pack.icons.json");
commands.entity(surface).insert(UiIcons(pack));

// Restore the embedded artwork later:
commands.entity(surface).remove::<UiIcons>();
```

The frame owner waits for the entire requested pack and its dependencies and
prepares the replacement before committing it. Pending or failed loads preserve
the current artwork. `UiIconStatus` on the surface reports the active pack,
pending state and errors; new failures also emit `UiError`. `active: None` denotes
the bundled set. Packs can also be constructed directly with `Assets<IconSet>`;
its `overrides` map contains `BuiltinIcon` keys and strong `Handle<Icon>` values.

```rust,ignore
controls.icon(BuiltinIcon::Folder);
controls.icon_button("save", BuiltinIcon::Save);
controls.icon_sized(BuiltinIcon::Rust, 32.0);

let custom: Handle<Icon> = asset_server.load("custom.svg");
controls.icon(&custom); // Explicit artwork bypasses pack overrides.
```

`Icon::from_svg(bytes)` supports embedded or programmatically supplied artwork.
`IconSource` can be stored as a component on an existing application entity and
passed to a control by reference. Immediate-mode icon instances need no entities.
Explicit custom handles reserve layout while loading and draw when available.

The private context cache prepares normal-size rasters once per loaded asset and
reuses them across controls and rows. Larger sizes initially use that raster and
request an exact physical-pixel raster for the next frame. Cached requests are
resolved against a replacement pack before it becomes active. Artwork revisions,
size and DPI distinguish cache entries; theme tinting and disabled/popup alpha
are applied when drawing. Theme changes do not rasterize or upload icons again.
Old revisions, abandoned sizes and surface retirement release managed textures
through ImGui's snapshot-aware retirement path. No GPU texture IDs are stored in
application components. `UiDiagnostics` exposes texture and rasterization counts.

Use **Browse icons** in either gallery host to filter the catalog, vary preview
size and switch between bundled artwork and a sample partial pack. The explicit
custom-asset preview demonstrates that it bypasses pack selection.

## Scheduling and command integration

`PostUpdate` runs `UiSystems::Draw`. One frame
owner prepares the context, invokes drawing systems, finishes the frame and hands
an owned draw snapshot to Bevy's render world. Native pointers remain on the UI
thread. `UiSystems::PrepareTargets` follows drawing and resizes visible viewport
images before Bevy's camera update. Render snapshots work with Bevy's normal
pipelined renderer. `UiGraph` runs `UiRenderSystems::Draw` into a transparent UI
layer, then `UiRenderSystems::Composite` into the window. The compositor honors
the UI camera's clear policy, preserves earlier scene cameras when clearing is
disabled, and blends premultiplied UI with proper sRGB decoding. Game render
schedules are unchanged. The separate UI stages leave editor effects independent
of game-camera effects.

UI has no workbench dependency. Applications can emit `DesktopAction::Activated`
with a placement/source and target entity. `bed2` demonstrates placements using
the existing `Activates` relationship and shared `ActionEnabled` availability.
Order drawing before the workbench's ingress:

```rust,ignore
app.configure_sets(PostUpdate, UiSystems::Draw.before(CommandSystems::Ingress));
```

Workbench collects native and UI actions and dispatches them after the frame
borrow ends. UI systems should update presentation state and enqueue application
intentions; consequential application work belongs to its controlled owner.

## Scene viewports and full-window overlays

Game cameras and model viewers render into ordinary Bevy `Image` assets. UI
displays that completed output; it owns no scene pipeline, materials or gameplay.
Create a compatible target and use the same handle for the camera and viewport:

```rust,ignore
let scene = images.add(viewport_image(UVec2::new(640, 400)));
// On an application-owned scene camera, retaining its normal render schedule:
commands.entity(scene_camera).insert((
    RenderTarget::Image(scene.clone().into()),
    Camera { order: -10, ..default() },
));
world.entity_mut(view).insert(UiViewport::new(scene));
```

The drawing system queries its `UiViewport` component:

```rust,ignore
fn draw_scene(frame: UiFrame, mut views: Query<&mut UiViewport>) {
    let mut viewport = views.get_mut(frame.view).unwrap();
    frame.ui.window("Scene").build(|| {
        let available = frame.ui.content_region_avail();
        frame.controls().viewport("scene", &mut viewport, available);
    });
}
```

Use positive logical dimensions. The control records its logical `rect`, physical
pixel size, `hovered` and `visible` observations. The frame resets visibility for
ECS viewport components before drawing. `PrepareTargets` updates visible targets
and requests projection refresh only for cameras using a resized image. Each
independently sized viewport needs its own output. Applications decide when a
hidden camera or simulation should pause and how viewport input is admitted.

`viewport_image` creates a resolved RGBA8 output with an sRGB camera view and an
encoded-color UI sampling view. Cameras can use HDR, MSAA and custom shaders in
their own pipelines, resolving and tone-mapping before this final image. Other
outputs must provide the equivalent single-layer, sampleable RGBA/BGRA8 contract;
raw HDR and depth textures are not directly displayed by this control.

The render world resolves each image's current GPU view before drawing UI. Resize
and asset replacement therefore use the current target, rather than a stale view
captured on the main thread. Private registration leases retain assets and texture
bindings until referencing snapshots have retired. Repeated rows/frames reuse
registrations; hidden outputs retire after a short grace period. GPU texture IDs
never appear in application components.

For UI over a scene rendering directly to the window, give the UI camera a later
order and disable its clear:

```rust,ignore
commands.spawn((
    Camera2d,
    Camera { order: 100, clear_color: ClearColorConfig::None, ..default() },
    CameraRenderGraph::new(UiGraph),
));
```

Draw floating controls, a window with `NO_BACKGROUND`, or use ImGui's foreground
draw list for a HUD anywhere in the window. An opaque full-window UI background
still covers the scene by design. An ordinary cleared UI camera continues to
support desktop applications without a scene camera.

`bed2` demonstrates both arrangements using the same game camera. **Full-window
scene** switches from the panel target to the window and leaves controls/HUD over
it; **Panel** switches back. Closing the demo returns to the gallery, and **Open
3D viewport** reopens it. The game scene and its own fullscreen color-grade shader
live in `bed2-demo-game`, which has no Bedtime dependency:

```sh
cargo run -p bed2
cargo run -p bed2-demo-game --example standalone
```

## Verification and current scope

```sh
cargo test --locked -p bedtime-ui2 --lib
cargo run --locked -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-smoke
cargo run --locked -p bed2 -- --viewport-smoke-dir target/ui2-viewport-smoke
cargo run --locked -p bed2-demo-game --example standalone -- --smoke-dir target/ui2-standalone-smoke
```

The example-only smoke path feeds native-shaped pointer events through the real
adapter, checks that clicks select light and a dropdown option, changes motion
state, opens a context menu, resizes the window, swaps icon packs, replaces artwork
at the same asset handle, restores the bundled set and captures eleven frames using
Bevy's screenshot path and exits normally.
It moves no OS pointer and does not interact with other applications.

The viewport smoke path checks actual cube pixels and visible rotation, drags an
ImGui panel resize through the native input adapter, compares panel/window scene
colors, verifies translucent UI blending, closes/reopens the viewport and captures
five frames. The standalone target checks the same scene independently of Bedtime.

Stage 2 provides one active surface, one bundled font family, asset-backed SVG icons
with replaceable packs, automatic control motion and anchored popup opening motion. Detached OS viewports, custom window chrome,
font-family discovery/fallbacks, general image/canvas APIs, editor GPU effects,
panels/docking policy and document widgets remain later work. Native behavior has
been exercised on macOS; Linux interaction and changes between physical DPI
environments still need validation.
