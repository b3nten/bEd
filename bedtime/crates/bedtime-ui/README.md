# bedtime-ui

Bed themes, icons, motion and composable drawing functions on upstream
`bevy_egui` and [vendored egui](../../../vendor/egui/BED_PATCHES.md). Applications install their own Bevy window, renderer, camera and event
loop. UI has no workbench dependency.

```sh
cargo run -p bedtime-ui --example gallery
```

## Host integration

Install Bevy's window, Winit, input, asset and renderer plugins before `UiPlugin`.
Attach the UI surface to the window and `UiCamera(surface)` to an application
camera targeting that window:

```rust,ignore
app.add_plugins(UiPlugin);
// Theme::dark() and default Metrics unless given.
world.entity_mut(window).insert((UiSurface, Theme::light()));
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
and egui context settings that UI changed during configuration. The camera
remains application-owned; UI removes its `EguiContext` and releases its managed
textures so detached UI stops processing and rendering. Reattaching `UiCamera`
creates and configures a fresh egui session.

Application post-processing can run in Bevy's `Core2d`/`Core3d` render schedules
after `bevy_egui::render::egui_pass`, before `UiSystems::PrepareOutput` and before
Bevy's `upscaling`. `PrepareOutput` owns final platform color/alpha conversion
(currently macOS transparent-window output); effects must return linear,
premultiplied color before this phase. UI does not own application shaders.

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

`tree` owns the Files and Git trees' shared row geometry, filename/icon painter,
item-sized hover/selection backgrounds, rainbow selection text and rotating
disclosures. `RowMotion` supplies smoothstep movement/fading with continuous
retargeting and reduced-motion settling. Callers own their data, row identity,
viewport culling, scroll anchors and actions; painting performs no filesystem I/O.
The painter multiplies its fade with the parent's opacity, including icons.
`frame.scroll(id, corner_radius, draw)` insets the scrollbar's ends by the supplied
radius so its track clears rounded container corners, while content keeps the
complete viewport. Pass zero for square edges. Layout geometry is supplied by
the caller rather than stored in UI's frame or context.
Surface ownership retires views and their parts; independently referenced themes
remain alive. One active UI surface is currently supported.

UI uses the integration's manual-pass support and runs one pass per Bevy frame.
Stateful drawing functions are not replayed for egui layout retries. In PreUpdate,
`UiSystems::Draw` follows upstream input processing. In PostUpdate,
`UiSystems::SyncViewports` runs application camera routing, followed by target
resizing and Bevy's camera updates. Upstream owns GPU extraction/rendering,
managed texture lifetime, cursor, IME and repaint integration.

## Appearance and motion

`Theme` holds semantic colors and `Metrics` sizes, both components on the
surface. `Palette.background` colors the window, titlebar, status bar, menu bar
and panel gaps; `panel_background` colors panel bodies and tab bars. The built-in
themes keep their original panel colors, with a slightly lighter window background
in dark mode and a slightly darker one in light mode.
`Palette.terminal` owns the 16 normal/bright ANSI colors, in standard ANSI order.
The built-in themes use bEd's Everforest Medium terminal palettes. These are
independent of the default foreground/background and are validated alongside the
semantic colors; terminal consumers own contrast adjustment and OSC overrides.

Bedfont (bundled Nordwand Mono 1.3.3) supplies the UI typography, including the matching real bold monospace
face exposed by `fonts::monospace_bold(size)`. `fonts::definitions()` supplies both
faces and egui's Unicode fallbacks for hosts customizing their font definitions.
`fonts::DEFAULT_FAMILY` is the application-facing name, **Bedfont**. The original
font files are unmodified; [provenance and license](assets/fonts/UPSTREAM.md)
travel with the embedded assets.

A surface's colors are its `Theme` component (name, light or dark, and a
`Palette` including terminal and syntax colors); its sizes are its `Metrics`
component (font size and family, padding, spacing, corners). `UiSurface`
requires both, defaulting to `Theme::dark()` (Everforest Dark Medium, bundled
with `Theme::light()`) and default metrics; an app replaces either by
inserting a new value. A theme file is a `Theme` in RON (`Theme::from_ron`,
`to_ron`), colors written `"#rrggbb"`. `theme::ThemeFolder`, inserted as a
resource, reads every `<id>.ron` in a folder and is kept up to date (about once
a second) as files change; files that don't parse are listed in `errors()`.

Every standard control (button, combo box, slider, toggle) is one height,
`theme::control_height`: a line of text plus `Metrics::padding`. The theme sets
egui's `interact_size.y` to it, because egui starts each horizontal row at that
height and centres widgets on it; with taller controls, widgets that nest a row
(combo boxes, sliders) sat below the centre of their row.

The theme chooses the font: `Metrics::font_family` names an installed family,
`None` is Bedfont. The runtime applies fonts itself when a context first draws
and when the family changes; hosts don't call `set_fonts`. A named family's
regular and bold faces go in front of Bedfont, so Bedfont and egui's Unicode
fallbacks cover what it lacks. With the `system-fonts` feature, initialising
the `fonts::InstalledFonts` resource discovers installed families on a
background thread (`loading()`, `error()`, `families()` with a `monospaced`
flag). While it's loading, a named family waits and the current fonts stay; a
family that isn't installed, or any family without discovery, falls back to
Bedfont. Which families to offer is the host's choice. Layout and panel configuration
belongs to `layouts::LayoutSurface`, independently of `Theme`.
The core has no layout configuration components (`layouts` adds them); ordinary egui widgets and layout
remain available directly.
`UiFrame` adds text edits, icons, tooltips, an
animated toggle and `primary_button`. The primary button scopes accent colors
over egui's inactive, hovered and active visuals, preserving native click/focus
and disabled behavior without restyling adjacent widgets.
Popup, menu and tooltip backgrounds scale outward from their source using bEd's
96% → 100% cubic ease-out and a matching fade, with the surface's motion
duration. The chosen placement supplies the pivot, including when a popup
flips near a window edge. Their contents fade at their final size and pixel
positions, avoiding the changing glyph sampling of scaled bitmap text. Layout
and hit bounds keep their final coordinates. Interaction settles background
growth while fading continues.
`tooltip_ui` accepts custom tooltip content with the same theme, timing and view
identity as `tooltip`. Runtime tooltips have zero delay and do not require a
stationary pointer; feature controllers may impose their own delay (bEd LSP
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
`Context::animate_color_with_time`. UI icon buttons use native image buttons;
the custom toggle retains switch geometry and delegates its color fade to egui.

`bedtime_ui::ComboBox` now re-exports egui's ComboBox. Its popup keeps the existing
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
bedtime-actions collects eligible egui shortcuts after drawing. UI leaves Bevy input
messages and `ButtonInput` unchanged. Applications and panels reading that input
own their focus/visibility gating and any held-key cleanup when it is suspended.

The existing `bedtime-clipboard::Clipboard` is shared by UI and application
systems. An application-provided memory clipboard is preserved. UI bridges
Copy/Cut/Paste input and egui CopyText output; upstream OS clipboard management
is disabled. Paste reads the optional text representation: a file-only clipboard
does not emit an egui text-paste event or a spurious `UiError`, and the original
Primary+V key remains available to application file-list commands. Clipboard
initialization and real operation errors retain the service's normal semantics;
native access/conversion failures still produce `UiError` rather than being
silently treated as absent text.

## Floating controls

Content that fills its panel (an image, a model, a video) keeps its controls
in `Floating` bars: small rounded bars at a corner or the middle of an edge of
a rect, shown while the pointer is over it or one of their menus is open, and
faded away otherwise (`shown(true)` keeps them, as for a paused video).
`UiFrame::floating_button`, `floating_toggle` (underlined in the accent while
on) and `floating_label` are their pieces.

```rust,ignore
Floating::new(id, rect, egui::Align2::CENTER_BOTTOM).show(&mut frame, |f| {
    if f.floating_button(BuiltinIcon::Fit, "Fit").clicked() { /* ... */ }
    f.floating_label("100%");
});
```

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
use bedtime_ui::{Component as _, UiViewport, viewport_image};

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
phase; UI owns its ordering against resizing and Bevy camera updates:

```rust,ignore
app.add_systems(PostUpdate, sync_scene.in_set(UiSystems::SyncViewports));
```

This phase sees completed application updates and this frame's UI observations.
Routing cameras before resizing also lets a newly selected target refresh its
projection in the same frame. Panels handle direct Bevy input using the
viewport's visibility and hover observations. Applications retain ownership of the independent scene and can run it without
the Workbench presentation layer.

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
`bed/demo-game/examples/embedded.rs` for a complete child app filling a UI
surface, including normal plugin initialization and App::update driving.

## Layouts

`layouts` (with `LayoutsPlugin`) tiles a window with areas holding panels as
tabs, which users split, join, resize and rearrange by dragging. It knows
nothing about what panels hold; apps add rules through hooks
(`check_panel_close`, `on_panel_close`, `PanelModified`,
`add_layout_validation`). `world.layouts()` reads and `world.layouts_mut()`
changes them; changes made there are the app's own, while a user's goes
through `user_edit` (permissions, close checks and hooks). A window with
`LayoutSurface` draws its chrome (`Titlebar`, measured and kept native by
`ChromePlugin` with the `native` feature), then its body: the active layout,
or the app's own `SurfaceBody`, which calls `view::show` where the panels go
(with a status bar under them, say). Error dialogs, and modals an app adds
(`add_modal`), are drawn before everything else on the window. With
`persistence`, layouts are `Persist` and can be saved as templates
(`capture_layout`, `instantiate_layout`). `cargo run -p bedtime-ui --example
layouts` is a layouts-only app.

## Layouts: composition

Layouts use the application's World and renderer; the layout UI's state
between frames is the `LayoutsUi` resource.

`Layout` supplies its area collection. `Area`/`PanelInstance`
supply presentation components. A concrete panel requires
`PanelInstance::of::<Self>()` to supply metadata and drawing immediately; ordinary
composition requires no panel-type registration:

```rust,ignore
fn setup(mut commands: Commands) {
    let layout = commands.spawn((Layout { name: "Code".into(), ..default() }, ActiveLayout)).id();
    let area = commands.spawn((Area::default(), InLayout(layout))).id();
    commands.spawn((Notes { text: "Initial draft".into() }, InArea(area)));
}
```

At most one `ActiveLayout` exists in the World; zero is valid. `all()`
returns stable insertion order and `active()` returns an Option.
A bare layout creates no area. `Area::default()` covers the full layout; multiple
areas must form a complete nonoverlapping integer `0..LAYOUT_EXTENT` tiling.
Areas may be empty. Attachment selects a panel's tab without changing area focus;
`select_panel`, `focus_area` and `focus_panel` make that distinction explicit.
Hooks maintain order and nearby selection/focus fallbacks immediately when
relationships apply. Reconciliation validates the graph and stale gestures; it is
not deferred panel initialization.

`InLayout`/`LayoutAreas` and `InArea`/`AreaPanels`
are linked Bevy relationships. Retiring a layout retires its areas and panels.
`Layouts::relocate_panel` moves between layouts while retaining document links
and managed work. Closing the last viewer retains the canonical document.
Game entities need no layout components. Application-wide session data belongs
in resources; optional panel/document owners give work a shorter lifetime.

Where a new panel goes: `area_for(layout, fits)` is an area with a panel
that `fits` (the focused one, else the most recently focused, by
`AreaFocusedAt`), else the largest area (`destination_area`). Opening
documents and bEd's panels both use it, so a new panel lands beside its kind.

Files dragged in from outside the app (Finder) get the same drop preview as
a panel drag when the app inserts `FileDrops`: given the dropped paths, it
returns the panel factory to place there, or `None` when it dealt with them
itself. Content can take the drop first (`reserve_for_content`), as a file
tree does.

No selected layout is normal: shell/titlebar/menu/status remain available and
panel content is skipped. `world.layouts_mut()` works in that state. Operations
that need a destination return an error. Deleting the last layout is allowed.
Keep deferred writers/readers ordered normally; unapplied Commands are not visible.

## Layouts: drawing

Tag the window and explicitly supply its camera. The surface supplies a dark
theme, UI surface and shell:

~~~rust,ignore
world.entity_mut(window).insert(LayoutSurface::default());
world.spawn((Camera2d, LayoutCamera));
~~~

Configure panel appearance and interaction directly on `LayoutSurface`.
Supply `Theme`, `Metrics` or `SurfaceShell` on the same window to configure them
explicitly. Mark an existing application camera instead of
spawning a new one if appropriate:

~~~rust,ignore
world.entity_mut(camera).insert(LayoutCamera);
~~~

Exactly one camera must be marked `LayoutCamera`, including cameras that
already have `UiCamera(window)`. `LayoutCamera` requires `UiCamera` (and its
`EguiContext`) immediately. Setup resolves its initially unbound window link;
it never creates or selects an unmarked camera. The application owns the camera and its
render order. Add `InView(window)` if it should retire with the window. Supplied
themes remain application-owned; a generated default theme retires with the
window. Missing or duplicate marked cameras, duplicate layout surfaces and
conflicting render targets fail clearly. UI plugins still require the
application's Bevy renderer/window plugins.

Setup runs automatically in `PreUpdate`, after application `Startup` and before
`bedtime_ui::UiSystems::Configure`. Inserting components is sufficient; no setup
helper or binding resource is needed. Systems that need the completed UI link,
theme or view can run after `UiSystems::Configure`, or in `Update`. Query
`With<LayoutSurface>` for the window and `With<LayoutCamera>` for its camera.
`SurfaceShell`, `UiSurface` and a transparent `UiBackground` are required
defaults available immediately when the surface component is inserted. An
explicit `UiBackground` overrides that default.

Removing `LayoutSurface` immediately retires its generated root renderer and
default theme, restores its camera target and Quit shortcuts, and detaches a UI
camera binding the surface supplied. UI restores camera clear/output,
compositing and context settings and ends that egui session when the binding is
removed. The application
window and camera remain alive. Supplied `UiCamera` bindings, themes and unrelated
views are preserved. Required/default components on the window remain in place;
reinserting `LayoutSurface` recreates its wiring in the next configuration
pass. Despawning the window performs the same cleanup for its surviving camera.

Every layout content instance is a panel. `Panel` is the rendering/type
contract; `PanelInstance` is the ECS metadata attached to each instance:

~~~rust,ignore
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
struct Notes { text: String }

impl bedtime_ui::layouts::Panel for Notes {
    const LABEL: &'static str = "Notes";
    fn draw(ui: &mut egui::Ui, world: &mut World,
            context: &bedtime_ui::UiContext<'_>, panel: Entity) {
        let mut notes = world.get_mut::<Notes>(panel).unwrap();
        context.frame(ui).text_edit(egui::TextEdit::singleline(&mut notes.text));
    }
}
commands.spawn((Notes { text: "Draft".into() }, InArea(area)));
// Required components preserve explicitly supplied component overrides.
~~~

Panel draw methods are ordinary Rust calls with exclusive world access, not
registered Bevy systems. Panels compose `bedtime_ui::UiComponent` renderers with
arbitrary associated props. The shell, panels, menus, titlebars and status renderers all draw in the
application World during the same egui pass. Scheduled input,
commands and updates remain Bevy systems. `Panel` has no initializer. Bevy
required components supply only missing state, preserving explicit overrides in
the spawn. `PanelInstance::of::<P>()` carries `P`'s label and draw function on
that same entity. Spawning `(MyPanel, InArea(area))` is the one way to create a
panel. Entity identities keep widget state distinct. Moving tabs retains
panel and widget state.

`LayoutSurface` owns panel rounding, suggested padding, gaps and corner hit
targets, along with single-area edge-to-edge presentation, tab visibility, corner
dragging and user permissions. Configure it directly or change its fields from
ordinary systems:

~~~rust,ignore
world.entity_mut(window).insert(LayoutSurface {
    drag_corners: false,
    tab_bar: TabBarVisibility::Hidden,
    ..default()
});
~~~

`TabBarVisibility` shows an area's tab bar `Always`, while it has
`MultipleTabs`, while the layout has `MultipleAreas`, with
`MultipleTabsOrAreas` (either), or keeps it `Hidden`. `area_background`
(`AreaBackground::Always`, `MultipleAreas`, `Never`) says when areas paint the
theme's panel background. With `MultipleTabsOrAreas` and `MultipleAreas`, a
layout of one area with one tab is just that panel, drawn straight onto the
window like a native terminal; tabs return once there's a second tab or area to
move panels between. bEd uses both.

Disabling `drag_corners` removes corner hit targets and cancels any ongoing
corner gesture. Divider resizing and other layout operations remain available.
Hidden tab bars reserve no space; each area's selected panel continues drawing
and can be selected through application operations. Hiding the bars also ends
any ongoing tab drag. These settings apply across layout switches.

By default one tiled area has no border, rounding or gaps (even with several
tabs). For a fixed panel presentation, `LayoutSurface::single_panel()` also hides
the tab bar and disables user layout editing, creation and closure:

~~~rust,ignore
world.entity_mut(window).insert(LayoutSurface::single_panel());
~~~

The same panel identity, focus and input path remains active. Application APIs
can still create/select/rearrange panels. `TabBarVisibility` supports `Always`,
`MultipleTabs` and `Hidden`. `Layouts::user_edit` applies the surface's
permission policy to a user layout action; direct application operations bypass
it. Custom command handlers should use `user_edit` for user layout actions.
Content padding and scrolling remain opt-in. A panel can query
`world.get::<LayoutSurface>(context.surface)` for the surface's suggested
padding and rounding, then pass them explicitly to UI's `padded` and `scroll`
helpers. Layout configuration is not part of `UiFrame` or `UiContext`.

UiFrame carries borrowed theme, icon, texture and input services. Its surface,
view and panel entities belong to the application World. Panels can inspect game
entities and shared UI components directly. `UiViewport` displays a normal Bevy
image; no viewport proxies or scene duplication are required:

```rust,ignore
let image = world.resource_mut::<Assets<Image>>()
    .add(bedtime_ui::viewport_image(UVec2::new(640, 400)));
world.entity_mut(game_camera).insert(RenderTarget::Image(image.clone().into()));
world.entity_mut(panel).insert(bedtime_ui::UiViewport::new(image));
```

UI resizes visible image targets before camera updates. Application camera
routing belongs in `UiSystems::SyncViewports`. Applications decide which cameras
are active and how gameplay pauses or steps. Additional preview scenes can use
render layers with the same renderer. Layouts don't create, tick or own
child Apps/SubApps. A plugin that needs isolated simulation/rendering owns that
integration; UI's independent `EmbeddedRenderPlugin` remains available. See
[UI viewports](../bedtime-ui/README.md#icons-and-viewports).

`SurfaceShell` on the window configures bounds, minimum area size, panel
visibility and titlebar ownership. It remains stable across layout switches.
`SurfacePresentation` reports the titlebar and panel allocations, including
usable `titlebar_content`.

The body under the titlebar is the active layout, unless the window has a
`SurfaceBody`: the app's own drawing function, which lays out whatever it wants
(a status bar, a sidebar) and calls `view::show(ui, world, context)` for the
panels, in the space it gives them:

~~~rust,ignore
world.entity_mut(window).insert(SurfaceBody(body));

fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    egui::Panel::bottom("status").show(ui, |ui| ui.label("Ready"));
    view::show(ui, world, context);
}
~~~

Add `Titlebar` to the same window as `LayoutSurface` to provide titlebar
content. The component holds the requested height and drawing function directly:

~~~rust,ignore
world.entity_mut(window).insert((
    Titlebar {
        height: 20.0,
        draw: draw_titlebar,
    },
    ApplicationChrome::default(),
));

fn draw_titlebar(ui: &mut egui::Ui, _: &mut World, context: &bedtime_ui::UiContext<'_>) {
    let mut frame = context.frame(ui);
    frame.ui.label("My application");
    if frame.icon_button(bedtime_ui::BuiltinIcon::Plus).clicked() {
        // Submit the application's operation.
    }
}
~~~

`Titlebar` requires `LayoutSurface`, supplying its defaults if it is absent.
The shell reads the component each frame; no registration call or renderer
entity is needed. Drawing receives the window as both `UiContext.view` and
`UiContext.surface`, so custom state can be ordinary components on that window.
The renderer runs once per visible frame in the application World and owns the
complete layout inside a clipped child Ui. Egui identities follow the surface,
keeping titlebar widget state stable across layout switches.
The host reserves extra space beside native controls, six points at
the horizontal window edges and four points above the content before calling it.
Popups retain their ordinary overlay bounds. Change `Titlebar.height` or
`Titlebar.draw` through normal component mutation; the height must be finite and
positive. Removing `Titlebar` removes the custom content and requested height,
retaining native chrome and the layout surface. The titlebar belongs to the
window and continues drawing with no selected layout. Its callback can compose
or select a layout before the shell draws the body.
Application setup must create any state its drawing callbacks require.

`SurfaceShell.titlebar_ownership` explicitly chooses who manages the titlebar:

- `TitlebarOwnership::Surface` (the default) renders the optional slot, paints
  its background and handles background window dragging and native double-clicks.
- `TitlebarOwnership::Application` leaves titlebar painting, native appearance and
  gestures to the application. The `Titlebar` component remains in place while
  drawing is suspended; switching back resumes it. No slot height or padding is
  allocated in this mode. The measured native band is still reserved to keep the
  menu and panels below native controls and accessories.

An application installing its own native widgets can select ownership when it
inserts the surface components, before native/UI updates:

~~~rust,ignore
world.entity_mut(window).insert((
    LayoutSurface::default(),
    SurfaceShell {
        titlebar_ownership: TitlebarOwnership::Application,
        ..default()
    },
));
// Install platform widgets on the application's native window.
~~~

This option does not create native widgets or change Bevy's window configuration.
The application owns their layout, lifetime, event handling and any bridge for
dragging native items into panels. Window backing color/blur and close-request
forwarding remain available through the native adapter in either mode.

With surface ownership, on macOS the slot uses the full-size content view beside
the native traffic lights. Other platforms, ordinary content views and embedded
host bounds use the same renderer in a header inside the content area. Without a renderer, only
the intersecting native titlebar band is reserved. The host lays out titlebar,
menu, panels and status in that order; applications need no manual bounds edits.
Presses on unused titlebar background start an OS window drag. Widget presses,
including typed `panel_drag_source` payloads, remain owned by their widgets.
The macOS adapter disables AppKit's automatic dragging through the transparent
content view, so widget gestures cannot move the window independently of egui.
It applies a per-instance, storage-compatible view subclass overriding only
`mouseDownCanMoveWindow`; Winit's view identity, event handlers and Metal layer
are retained. Switching to application ownership restores the original view class
and native appearance; switching back reinstates surface behavior. With
surface ownership, background dragging remains active even without a registered
renderer. The native adapter also restores mouse-up after native drags
and respects the system double-click preference for minimize, zoom or no action.

Areas tile a normalized 0–10,000 integer plane. Area owns its rectangle and selected
panel; AreaPanels stores tab order. `Layouts` methods alone commit topology and
membership changes. Switching layouts retains state; closing retires instances.
Panel and layout closure reject bound viewers carrying `DocumentDraft`; unsaved
documents are handled as in [Closing and quitting](#closing-and-quitting).
Direct raw despawn bypasses these checks.

Panel content is clipped to its content rectangle and the area's rounded outline,
including viewport images and custom painting. The host enforces this boundary;
plugins do not need to match their own corner radii. Popups and tooltips retain
their separate overlay layers.
Panel content has no implicit padding or scroll area. Plugins opt into UI's
padded and scroll helpers. PanelPresentation and AreaPresentation report local
visibility, focus and geometry. LayoutSurface controls gaps, padding, rounding
and corner targets independently of the theme.

### Per-item tab text

`PanelTabStyle` is an optional component on a **panel instance**, not its type
registration or area. It is plain core data and does not require the `ui` feature:

```rust,ignore
world.entity_mut(panel).insert(PanelTabStyle {
    italic: true,
    text_opacity: 0.9,
});
// Return to automatic normal/preview styling:
world.entity_mut(panel).remove::<PanelTabStyle>();
```

Without an override, normal labels inherit the theme unchanged; preview labels
are italic at 90% of their themed text opacity, with no underline. An explicit
component replaces both defaults: `PanelTabStyle::default()` means upright text
at full themed opacity, even on a preview. Opacity multiplies the current themed
selection/hover text color, including its alpha; it does not change icons, dirty
markers, close controls or hover backgrounds. Finite opacity values are clamped
to `0.0..=1.0` for rendering; nonfinite values use `1.0`. Font metrics remain
theme-owned. Tab width and painting use the same shaped label, including italics.

`PanelInstance.preview` remains semantic state. Styling never sets or clears it;
keeping or moving a preview still clears that flag. Explicit styles stay on the
same item when it moves or becomes permanent. Style insertion, edits and removal
are observed on the next draw. This is
**runtime-only appearance configuration**. Applications own any serialization
and reapply overrides when composing their panels.

The frontend retains Blender-style interactions:

- Drag tabs to reorder, group or split. Moving a final tab fills its vacated area;
  closing a final tab leaves an empty area.
- Drag area corners inward to split, across a border to join, or farther into
  another area to relocate. Partial joins preserve target tails.
- Drag shared borders to resize connected spans, including T-junctions.
- Close with x, middle click or the tab menu. Double click keeps a preview tab.

Gestures cancel on Escape, right click, focus loss, activation or bounds changes.
Resize previews commit on release. Interactive cross-layout dragging and floating
native panels remain outside this implementation; checked `relocate_panel` is an
explicit programmatic transition, not a cross-layout drag implementation.

Layouts have no `PanelType`, panel registry/catalog or built-in panel picker.
The application owns creation discovery (for example, a View menu) and directly
spawns its concrete components. For creation drags, pass an application-owned
`PanelFactory::new(label, create)` to `panel_drag_source`, where
`create: impl Fn(&mut World, Entity) -> Entity + Send + Sync + 'static` creates
exactly one panel in the supplied area and returns it. Constructors can capture
source data, such as a file path. `PanelFactoryPayload::with_content` retains
typed widget data alongside the constructor so content targets can consume the
drag before the shell offers area/tab/split targets. Eligible content targets call
`PanelFactoryPayload::reserve_for_content` each draw pass to retain hover-feedback
priority until release. Validate application-specific inputs before offering this
infallible factory; layouts don't resolve descriptive keys to constructors.

Gestures are deferred: drawing records each edit with the layout it was made in,
and `LayoutSystems::Apply` applies them after drawing, before keyboard routing.
An edit is dropped if that layout is no longer active, if anything it names has
left the layout, or (for a border resize) if that border no longer exists.

## Shortcuts and direct panel input

The UI adapter reads only the explicit LayoutCamera's egui context, after
drawing and applying panel focus observations. It uses `consume_shortcut` for
the chords in `Keybindings`, retaining exact modifier matching and event order. Widgets can consume a shortcut first. Text editing reserves typing,
navigation and clipboard keys, including no-op edits and Enter ending focus;
unrelated chords such as Save remain available. Popups/modals and an unfocused
layout surface window suppress keyboard shortcuts.
The app's actions own Quit; the surface disables egui's built-in Quit shortcut
on its camera, preserving the application's command and shutdown policy.

The PreUpdate order is UI/shell drawing, focus observations, shortcut collection
(`ActionSystems::CollectInput`), binding resolution, Action
dispatch and queued layout edits. The headless core accepts
normalized `BindingInput` directly and has no egui dependency.

Panels can read Bevy input directly in their own systems or use egui input in
their drawing functions. They own gating using window focus, panel visibility,
`Layouts::has_focus`, viewport hover/egui responses and the surface's
`UiCapture` summary. Layouts don't filter Bevy events or provide a second
held-key state. Consuming an egui shortcut leaves Bevy input unchanged.

See [stage 4](../../../MIGRATION/application-composition.md) for migration and validation.

## Window transparency and macOS blur

`WindowBlur(true)` enables a passive AppKit material behind the Winit content
view. Removing/disabling it hides the material. Its lifetime follows the window;
resizing keeps it fitted without reparenting or replacing Winit's view. It does
not receive input. `WindowBackground` continues to select appearance and native
backing color; translucent GPU backgrounds use a clear native backing so alpha
is applied once. Window-wide opacity remains 1 so text stays opaque.

Applications configure Bevy window transparency/composite alpha mode and the
UI camera's scene-overlay routing. On macOS, enable `fullsize_content_view`
alongside `titlebar_transparent` to paint behind the native titlebar, as in the former application.
Set `titlebar_show_title: false` when the application supplies its own titlebar
content; keep `titlebar_shown` and `titlebar_show_buttons` enabled to retain native
controls. `WindowTitlebarGeometry` reports AppKit's reserved height and the union
of visible native button bounds before UI drawing, in logical content-view
coordinates with a top-left origin. Measurements are independent of tint and
blur. The surface converts them to egui points using the current zoom and reserves
only the portion intersecting its bounds. Native controls remain owned by the OS;
application renderers do not measure or coordinate them. Windows custom-frame
integration and Linux client-side decorations are not implemented.

Panel gaps and outer margins default to 5 logical points in multi-area layouts.
Single-area layouts are edge-to-edge by default. `LayoutSurface.panel_gap` sets the full spacing between panels
and between panels and the shell's window, menu, titlebar and status edges.
Tabs have transparent fills with a 12% text-color hover
tint, square lower corners and meet their border. Tab bars have no add button;
empty areas have no built-in type picker; creation UI belongs to the application.
Corner handles use a crosshair cursor and take egui input priority over panel contents. A tab's faint
hover background slides up from its bottom edge at twice the surface's motion
speed; its geometry and text stay fixed. Reduced motion
settles the highlight immediately. New areas appear at their final size.
Tabs fill the bar height up to the inside of the top border. Both top corners use
the panel rounding. Their icons, labels and close buttons are
centered vertically in the full bar, independently of the tab background.
The tab bar reaches both panel edges; only the contents inside each tab are padded.
Areas give up reassigned space immediately and animate only into newly available
space. A tab drop that splits an area's width while expanding its height snaps
the width and animates the height independently. Corner splits show both halves
immediately; direct border dragging follows the pointer.
While dragging a corner within its own area, the split line follows the pointer
without easing or minimum-size clamping. Releasing clamps the actual split to
the minimum panel size.
Tabs follow their area's geometry without separate interpolation. Panel contents
fade in when first shown or when their committed area or placement changes, using
the surface motion policy. Nested widgets, icons and viewport images inherit the
fade. Geometry interpolation does not restart it; reduced motion settles it
immediately. Fade timing starts at first display and uses elapsed egui time:
preceding idle time and repeated layout passes cannot consume the animation.
Pure area relocation snaps its geometry while fading its contents.

`LayoutSurface` also configures panel rounding, suggested opt-in content
padding, panel gaps, corner hit size and debug visibility, independently of `Theme`.
UI drawing helpers take explicit geometry and do not depend on this component.
Defaults are 7-point rounding, 14×12-point padding, 5-point gaps, 13-point hit targets and debug off:

```rust,ignore
world.entity_mut(window).insert(LayoutSurface {
    rounding: 7.0,
    padding: [14.0, 12.0],
    panel_gap: 5.0,
    hit_size: 24.0,
    debug: true,
    ..default()
});
```

Hit size is measured inward from each visible panel corner after gaps are applied,
so changing panel spacing moves the handles with the panels. Zero disables corner
handles, as does `LayoutSurface.drag_corners = false`. Targets are capped at
half the visible panel's shortest side to prevent overlapping corners. Debug
paints the rectangles registered with egui; egui's normal interaction radius also
allows nearby presses in the gutter. Hover cursors and drag startup use the same
egui response, with priority over panel content and below popups/modals.

The native corner regression scenario exercises all four corners, including a
four-area junction, at 0-, 5-, 16- and 32-point gaps. It splits and rejoins through
real pointer input, including presses just into the gutter:

```sh
cargo build --offline --locked -p bed --features automation
python3 bedtime_old/tools/smoke_corners.py  # drives bed_old; not maintained
```

## Error dialogs and the modal layer

Request user-facing errors independently of the panel that noticed
an operation fail:

```rust,ignore
world.request_error_dialog(ErrorDialog {
    title: "Could not open the file".into(),
    message: "The file is no longer available. Refresh Files and try again.".into(),
    details: Some(error.to_string()),
});
```

Callers own domain-specific titles, understandable messages and any recovery
advice. Raw I/O/debug diagnostics belong in optional `details`, not the main
message. The surface presents a nonblocking egui modal with an OK button;
Enter/Escape also dismiss it. Technical details start collapsed and use
selectable monospace text. There is no native alert, new window, logging service
or general notification framework.

`request_error_dialog` lazily creates the `ErrorDialogs` resource without any
plugin or window requirement. `len`, `is_empty` and `front` inspect
retained dialogs, including the active one. Requests survive a headless/no-UI
period and outlive their source panel. The surface root presents
one dialog before drawing shell/panels, even with hidden panels or no selected
layout. It retains the first available layout surface window until dismissal,
rehoming the dialog if that window/surface closes. The shell is disabled during
the first, active and closing frames; dismissal Enter/Escape events are removed
before underlying panel gestures run. Bevy input remains application-owned.

The queue holds at most 16 entries including the active dialog. Identical
active/pending requests are deduplicated. If full, its last pending error and
new requests become one additional-error count, without retaining their
identities or diagnostic details. Further requests coalesce into that summary
until it is dismissed; its count describes omitted requests, not distinct
errors. Earlier retained duplicates do not increase the count. A dismissed
error can be requested again, so callers must edge-detect persistent failures
rather than resubmitting them on every draw/poll. Errors requested during drawing
appear on the next root pass. The module/API are absent with `ui` disabled; the
standalone core remains renderer-independent.

## Verification

```sh
cargo test -p bedtime-ui
cargo run -p bedtime-ui --example gallery -- --smoke-dir target/ui2-egui-smoke
```

The gallery fixture uses observed widget rectangles to exercise native-shaped
Bevy input, theme selection, dropdowns, overlays, resizing, icon replacement and
stable raster reuse. The gallery smoke uses a memory clipboard.

For desktop translucency, set Bevy's `Window.transparent = true` and
`composite_alpha_mode = CompositeAlphaMode::PostMultiplied` on macOS. The theme's
background alpha controls window background opacity, and panel_background alpha
controls panel fills over that background; text and ordinary widgets retain their
own alpha. With a layout surface, use `UiBackground::Transparent` so the shell paints the
window background once before its panel fills. The optional
native window adapter supplies macOS blur via `WindowBlur`. For app-painted native
titlebars, use Bevy's `fullsize_content_view` with `titlebar_transparent` and
use the surface's titlebar slot or reserve the reported `WindowTitlebarGeometry`
when allocating application controls directly.

`rainbow_ink` supplies bEd's RGB sine cycle for editor carets/active line numbers
and selected tree labels. `paint_chevron` and `animate_expansion` share filled
right/down disclosure triangles and reversible 160 ms smoothstep timing.
`icon_button` animates its background and ink through hover, focus and press;
with `visuals.button_frame = false` its resting background is transparent.
`icon_button_sized` fixes the artwork's point size independently of the font;
egui button padding still supplies the hit rectangle.
Callers attach descriptive tooltips through the existing themed tooltip helper.
All respect `MotionPolicy.enabled`.

Hyperlinks and editor modifier-hover underlines use the live theme accent.
