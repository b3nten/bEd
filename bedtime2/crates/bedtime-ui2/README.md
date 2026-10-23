# bedtime-ui2

Bed themes, icons, motion and composable drawing functions on upstream
`bevy_egui` and [vendored egui](../../../vendor/egui/BED_PATCHES.md). Applications install their own Bevy window, renderer, camera and event
loop. UI2 has no workbench dependency.

```sh
cargo run -p bedtime-ui2 --example gallery
```

## Host integration

Install Bevy's window, Winit, input, asset and renderer plugins before `UiPlugin`.
Attach the UI surface to the window and `UiCamera(surface)` to an application
camera targeting that window:

```rust,ignore
app.add_plugins(UiPlugin);
let dark = world.spawn(themes::dark()).id();
world.entity_mut(window).insert((UiSurface, UsesTheme(dark)));
world.spawn((Camera2d, UiCamera(window)));
let view = register_view(world, window, draw_form);
world.entity_mut(view).insert(FormState::default());

fn draw_form(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let mut frame = context.frame(ui);
    let mut form = world.get_mut::<FormState>(context.view).unwrap();
    frame.ui.heading("My form");
    frame.text_edit(egui::TextEdit::singleline(&mut form.name));
    frame.ui.checkbox(&mut form.enabled, "Enabled");
}
```

Root renderers run sequentially in `SurfaceViews` order. They are ordinary
function pointers, not registered Bevy systems. Surface membership determines
root execution and ownership; it does not position or nest a renderer. Parents
allocate space with ordinary egui layout and call child components directly.

Removing `UiCamera` restores the camera clear/output modes, compositing space
and egui context settings that UI2 changed during configuration. The camera
remains application-owned; UI2 removes its `EguiContext` and releases its managed
textures so detached UI stops processing and rendering. Reattaching `UiCamera`
creates and configures a fresh egui session.

Application post-processing can run in Bevy's `Core2d`/`Core3d` render schedules
after `bevy_egui::render::egui_pass`, before `UiSystems::PrepareOutput` and before
Bevy's `upscaling`. `PrepareOutput` owns final platform color/alpha conversion
(currently macOS transparent-window output); effects must return linear,
premultiplied color before this phase. UI2 does not own application shaders.

`Component` has associated borrowed `Props<'a>` and a static
`draw(ui, world, context, props)` method. Props can contain child props, borrowed
labels, callbacks or entity IDs. Use `()` when no props are needed.
`register_component::<C>` mounts a component with unit props as a surface root;
`register_view` accepts an ordinary root function. Persistent application state
belongs to ECS data; local presentation state can use scoped egui IDs. Do not
hold a component/resource borrow while passing the same world mutably to a child.

`UiContext` supplies instance identity, prepared theme, motion and drawing
services separately from `&mut egui::Ui`. That allows normal composition:

```rust,ignore
ui.horizontal(|ui| Details::draw(ui, world, context, props.details));
```

Panel/view identity scopes egui IDs. `context.frame(ui)` optionally exposes
existing `UiFrame` icon, clipping and text helpers. It is an ordinary
borrowed convenience wrapper, with no Bevy SystemInput or scheduling machinery. `scope`, `scroll` and `padded` reborrow the same services into
nested UIs. Panel hosts supply no content padding or automatic scrolling.
`paint_drop_overlay` supplies the shared translucent fill/two-point border used
by panel and folder drag destinations. Callers own eligibility, clipping, bounds
and motion; this painter neither accepts drops nor highlights invalid targets.
`clip_rounded` enforces a rounded paint boundary for images, text and custom
meshes, preserving UVs, colors and transparent backgrounds. Workbench applies it
to every panel automatically. Child scissors cannot widen that boundary;
popups on separate layers keep their own bounds. Direct GPU paint callbacks
crossing a rounded boundary must render into a `UiViewport` image.
Content can opt into `frame.padded([14.0, 12.0], draw)`.
`frame.scroll(id, corner_radius, draw)` insets the scrollbar's ends by the supplied
radius so its track clears rounded container corners, while content keeps the
complete viewport. Pass zero for square edges. Layout geometry is supplied by
the caller rather than stored in UI2's frame or context.
Surface ownership retires views and their parts; independently referenced themes
remain alive. One active UI surface is currently supported.

UI2 uses the integration's manual-pass support and runs one pass per Bevy frame.
Stateful drawing functions are not replayed for egui layout retries. In PreUpdate,
`UiSystems::Draw` follows upstream input processing. In PostUpdate,
`UiSystems::SyncViewports` runs application camera routing, followed by target
resizing and Bevy's camera updates. Upstream owns GPU extraction/rendering,
managed texture lifetime, cursor, IME and repaint integration.

## Appearance and motion

`Theme`, `UsesTheme` and Everforest light/dark values retain semantic colors and
metrics. `Palette.background` colors the window, titlebar, status bar, menu bar
and panel gaps; `panel_background` colors panel bodies and tab bars. The built-in
themes keep their original panel colors, with a slightly lighter window background
in dark mode and a slightly darker one in light mode.
`Palette.terminal` owns the 16 normal/bright ANSI colors, in standard ANSI order.
The built-in themes use bed1's Everforest Medium terminal palettes. These are
independent of the default foreground/background and are validated alongside the
semantic colors; terminal consumers own contrast adjustment and OSC overrides.

Bedfont (bundled Nordwand Mono 1.3.3) supplies the UI typography, including the matching real bold monospace
face exposed by `fonts::monospace_bold(size)`. `fonts::definitions()` supplies both
faces and egui's Unicode fallbacks for hosts customizing their font definitions.
`fonts::DEFAULT_FAMILY` is the application-facing name, **Bedfont**. The original
font files are unmodified; [provenance and license](assets/fonts/UPSTREAM.md)
travel with the embedded assets. System font discovery and selection belong to
the host application.
Hosts replacing the definitions must retain that named bold family for terminal
rendering. Workbench layout and panel configuration
belongs to the workbench crate's `WorkbenchSurface`, independently of `Theme`.
UI2 has no workbench configuration components; ordinary egui widgets and layout
remain available directly.
`UiFrame` adds text edits, icons, tooltips, an
animated toggle and `primary_button`. The primary button scopes accent colors
over egui's inactive, hovered and active visuals, preserving native click/focus
and disabled behavior without restyling adjacent widgets.
Popup, menu and tooltip backgrounds scale outward from their source using bed1's
96% → 100% cubic ease-out and a matching fade, with the surface's motion
duration. The chosen placement supplies the pivot, including when a popup
flips near a window edge. Their contents fade at their final size and pixel
positions, avoiding the changing glyph sampling of scaled bitmap text. Layout
and hit bounds keep their final coordinates. Interaction settles background
growth while fading continues.
`tooltip_ui` accepts custom tooltip content with the same theme, timing and view
identity as `tooltip`. Runtime tooltips have zero delay and do not require a
stationary pointer; feature controllers may impose their own delay (bEd2 LSP
hover uses 200 ms).

`MotionPolicy` is independent of theme. Default duration is 160 ms. Toggle motion
uses cubic ease-out with continuous retargeting; disabling motion settles
immediately and disables popup growth/fading. Stable egui IDs retain identity.
`UiFrame::animate_bool(id, target)` supplies the same transition for custom
shell geometry. The vendored egui crate fades widget interaction/selection colors
internally, including primary buttons, menu items, checkboxes, radio buttons,
sliders, dropdowns, text-field focus borders and scrollbars. Existing calls and
scoped styles keep working; input and target layout update immediately.
The surface motion policy supplies egui's animation duration, including zero for
reduced motion. Custom painters can use `Style::interact_animated` or
`Context::animate_color_with_time`. UI2 icon buttons use native image buttons;
the custom toggle retains switch geometry and delegates its color fade to egui.

`bedtime_ui2::ComboBox` now re-exports egui's ComboBox. Its popup keeps the existing
half-item-spacing gap (4 points by default) and hidden scrollbar. Wheel scrolling
and height limits still work. [Vendor provenance and patch notes](../../../vendor/egui/BED_PATCHES.md)
document the shared animation implementation and upgrade checks.

UI cameras use linear compositing and premultiplied output. A small shader asset
override corrects upstream premultiplied sRGB colors for linear-light blending;
the upstream integration owns normal rendering. On macOS, an LDR window UI camera
that clears its output and targets `CompositeAlphaMode::PostMultiplied` also runs
a final conversion after egui and before Bevy's output blit. AppKit expects RGB
premultiplied in display space, whereas Bevy blends premultiplied linear RGB.
The conversion unmultiples the completed image, encodes its color, and multiplies
by alpha in display space. Applying this per widget would corrupt overlapping
translucent surfaces. It prevents bright fringes around antialiased icons/text
and keeps background opacity consistent with the compositor. Image viewports,
HDR targets and scene overlays whose output clear is `None` skip this conversion.
For a scene underneath UI, clear the
UI camera's intermediate target to transparent each frame with
`ClearColorConfig::Custom(Color::NONE)`. Its output preserves the earlier window
camera. `UiBackground::Theme` paints the theme background. Select
`UiBackground::Transparent` when the application/workbench paints its own surfaces.
UI cameras clear their intermediate and window targets to transparent by default;
an overlay over an earlier scene camera must set its output clear to `None`.

## Input and clipboard

egui owns widget focus, text editing and pointer interaction. Custom widgets can
use ordinary egui responses and `input_mut` consumption. `UiFrame::text_edit` is
a convenience for `TextEdit::show`; it adds no event-ownership layer.

`UiCapture` on the surface reports keyboard/pointer interest, text editing and
popup/modal activity. Text and modal flags include activity earlier in the same
frame, so closing a text edit or popup cannot leak its final key into a shortcut.
Custom text canvases call `egui::Context::register_text_input(id)` when drawing;
the ordinary focused widget ID then participates in the same text capture policy
without creating a `TextEdit` buffer or private history.
Workbench consumes eligible egui shortcuts after drawing. UI2 leaves Bevy input
messages and `ButtonInput` unchanged. Applications and panels reading that input
own their focus/visibility gating and any held-key cleanup when it is suspended.

The existing `bedtime-clipboard2::Clipboard` is shared by UI and application
systems. An application-provided memory clipboard is preserved. UI2 bridges
Copy/Cut/Paste input and egui CopyText output; upstream OS clipboard management
is disabled. Paste reads the optional text representation: a file-only clipboard
does not emit an egui text-paste event or a spurious `UiError`, and the original
Primary+V key remains available to application file-list commands. Clipboard
initialization and real operation errors retain the service's normal semantics;
native access/conversion failures still produce `UiError` rather than being
silently treated as absent text.

## Icons and viewports

Icons remain Bevy `Icon` and partial `IconSet` assets. A surface's `UiIcons` handle
selects a pack; missing slots inherit bundled artwork. Pending/failed replacement
packs preserve current artwork and report status through `UiIconStatus`.
Rasterization is cached by artwork revision and physical size; egui texture
handles own retirement. `BuiltinIcon`, explicit handles and `IconSource` remain
available. Theme changes affect tint, not SVG rasterization.

`UiViewport` stores a standard Bevy image target and implements the drawing
`Component` trait. Attach it to an entity and draw it into an ordinary egui
allocation:

```rust,ignore
use bedtime_ui2::{Component as _, UiViewport, viewport_image};

let image = world.resource_mut::<Assets<Image>>()
    .add(viewport_image(UVec2::new(640, 400)));
world.entity_mut(camera).insert(RenderTarget::Image(image.clone().into()));
world.entity_mut(panel).insert(UiViewport::new(image));

// In the panel's draw function; fills the supplied UI allocation.
UiViewport::draw(ui, world, context, panel);
```

Drawing registers the image directly with `bevy_egui::EguiUserTextures`, which
caches the image-to-texture ID mapping. The component and camera own strong image
handles; egui uses a weak registration and retires it when the asset is removed.
Resizing preserves the image asset and texture ID. No `UiFrame` is needed.
Use `UiViewport::show(ui, world, entity)` when the caller needs its
`egui::Response`, for example to implement orbit controls. It performs the same
drawing and observation updates as the `Component` implementation, while
preserving egui's drag ownership outside the image until release.

The scene retains its normal
cameras and post-processing. UI reports its content rectangle, visibility,
hover state and requested physical size; the application decides camera routing
and simulation policy. For scenes owned by the host, register synchronization in the public viewport
phase; UI2 owns its ordering against resizing and Bevy camera updates:

```rust,ignore
app.add_systems(PostUpdate, sync_scene.in_set(UiSystems::SyncViewports));
```

This phase sees completed application updates and this frame's UI observations.
Routing cameras before resizing also lets a newly selected target refresh its
projection in the same frame. Panels handle direct Bevy input using the
viewport's visibility and hover observations. The independent scene still runs without Bedtime:

```sh
cargo run -p bed2-demo-game --example standalone
```

For scenes owned by another app, `ViewportHostPlugin` (included in UiPlugin)
publishes GPU images without copying scene entities or reading pixels back to
the CPU. Create a `ViewportOutput` during host Startup, then install
`EmbeddedRenderPlugin<C>::new(output.clone())` before the child app's scene/game
plugins. C marks the scene camera(s) that produce the output. Display
`UiViewport::new(output.image.clone())` in the host UI or a workspace panel.

The child plugin installs standard Bevy plugins with a windowless renderer,
shared host GPU resources, and sequential child rendering. It preserves an
already installed TimePlugin. The child retains its own asset stores, scene
entities, normal extraction, render schedules and custom shaders. The host
image handle is host-local; do not query it in the child's Assets<Image>.

UI observations control visibility and physical target size. Resizing replaces
the shared texture and refreshes the child camera projection in the same frame.
The renderer stops drawing hidden outputs; the application's simulation/update
policy remains independent. One output is supported per child app. Dropping its
ViewportOutput owners releases the export and normal image/GPU ownership.

This support has no Workbench dependency. See
`bed2/demo-game/examples/embedded.rs` for a complete child app filling a UI
surface, including normal plugin initialization and App::update driving.

## Verification

```sh
cargo test -p bedtime-ui2
cargo run -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-egui-smoke
```

The gallery fixture uses observed widget rectangles to exercise native-shaped
Bevy input, theme selection, dropdowns, overlays, resizing, icon replacement and
stable raster reuse. The gallery smoke uses a memory clipboard.

For desktop translucency, set Bevy's `Window.transparent = true` and
`composite_alpha_mode = CompositeAlphaMode::PostMultiplied` on macOS. The theme's
background alpha controls window background opacity, and panel_background alpha
controls panel fills over that background; text and ordinary widgets retain their
own alpha. With workbench, use `UiBackground::Transparent` so the shell paints the
window background once before its panel fills. The optional workbench
native window adapter supplies macOS blur via `WindowBlur`. For app-painted native
titlebars, use Bevy's `fullsize_content_view` with `titlebar_transparent` and
use workbench's titlebar slot or reserve the reported `WindowTitlebarGeometry`
when allocating application controls directly.

`rainbow_ink` supplies bed1's RGB sine cycle for editor carets/active line numbers
and selected tree labels. `paint_chevron` and `animate_expansion` share filled
right/down disclosure triangles and reversible 160 ms smoothstep timing.
`icon_button` animates its background and ink through hover, focus and press;
with `visuals.button_frame = false` its resting background is transparent.
`icon_button_sized` fixes the artwork's point size independently of the font;
egui button padding still supplies the hit rectangle.
Callers attach descriptive tooltips through the existing themed tooltip helper.
All respect `MotionPolicy.enabled`.

Hyperlinks and editor modifier-hover underlines use the live theme accent.
