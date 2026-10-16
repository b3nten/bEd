# bedtime-ui2

Bed themes, icons, motion and borrowed Bevy drawing systems on upstream
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

fn draw_form(mut frame: UiFrame, mut forms: Query<&mut FormState>) {
    let mut form = forms.get_mut(frame.view).unwrap();
    frame.ui.heading("My form");
    frame.text_edit(egui::TextEdit::singleline(&mut form.name));
    frame.ui.checkbox(&mut form.enabled, "Enabled");
}
```

Drawing systems run sequentially in `SurfaceViews` order. Panel/view identity
scopes egui IDs. `UiFrame` supplies a borrowed mutable egui Ui, prepared theme and
motion policy. `scope`, `scroll` and `padded` reborrow the same services into
nested UIs. Panel hosts supply no content padding or automatic scrolling.
Panels can opt into `frame.padded(frame.panel_settings.padding, draw)`;
the default panel settings use 14 horizontal and 12 vertical points, matching bed1.
`scroll` insets the scrollbar's ends by `frame.panel_settings.rounding` so its track clears
rounded container corners, while content keeps the complete viewport.
Surface ownership retires views and their parts; independently referenced themes
remain alive. One active UI surface is currently supported.

UI2 uses the integration's manual-pass support and runs one pass per Bevy frame.
Stateful drawing systems are not replayed for egui layout retries. In PreUpdate,
`UiSystems::Draw` follows upstream input processing; `UiSystems::PublishInput`
commits remaining input before application Update. Viewport target resizing stays
in PostUpdate before camera updates. Upstream owns GPU extraction/rendering,
managed texture lifetime, cursor, IME and repaint integration.

## Appearance and motion

`Theme`, `UsesTheme` and Everforest light/dark values retain semantic colors and
metrics. PaperMono supplies the UI typography. Optional surface `PanelSettings`
separately owns `rounding`, `padding`, `panel_gap`, `hit_size` and `debug`; `UiFrame.panel_settings`
contains the resolved settings for each draw. Theme changes and resets leave them
alone. The defaults are 8.5-point rounding, 14×12-point padding, 7.2-point gaps, 15-point corner
targets, and debug off. Use ordinary egui widgets and layout directly.
`UiFrame` adds text ownership, icons, viewports, tooltips, an
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
identity as `tooltip`.

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
all render infrastructure remains upstream. For a scene underneath UI, clear the
UI camera's intermediate target to transparent each frame with
`ClearColorConfig::Custom(Color::NONE)`. Its output preserves the earlier window
camera. `UiBackground::Theme` paints the theme background. Select
`UiBackground::Transparent` when the application/workbench paints its own surfaces.
UI cameras clear their intermediate and window targets to transparent by default;
an overlay over an earlier scene camera must set its output clear to `None`.

## Status bars

A host creates a status bar with `create_status_bar(world, owner)` and draws it
inside an allocated child Ui using `draw_status_bar`. The bar has no independent
surface view or window-relative position. Workbench creates and lays out its bar
automatically; obtain it from `WorkbenchHost::status_bar()`. Plugins register
ordinary Bevy drawing systems on the supplied bar, attach their own data to each
returned item entity, and optionally register an expanded tooltip system:

```rust
use bedtime_ui2::{
    BuiltinIcon, StatusItem, StatusSide, UiFrame,
    register_status_item, register_status_tooltip,
};

// `bar` is supplied by the host.
let item = register_status_item(
    world, bar,
    StatusItem::new(StatusSide::Left, BuiltinIcon::CloudDownload, "Connection", 120.0),
    draw_connection,
);
register_status_tooltip(world, item, draw_connection_details);

fn draw_connection(frame: UiFrame) {
    frame.ui.label("Connected");
}
fn draw_connection_details(frame: UiFrame) {
    frame.ui.heading("Connection");
    frame.ui.label("Ready to synchronize changes.");
}
```

Standalone UI hosts allocate `status_bar_height(ui)` with an egui bottom Panel,
paint its background and call `draw_status_bar(&mut frame, world, bar)` inside it.
The bar respects that allocation, including an inset host rectangle. Its
`InView(owner)` relationship retires the bar and its contributions with the host.
Pass the bar entity to each plugin's registration function. UI2 has no workbench
dependency. `StatusSide::{Left, Right}` anchor contributions at the
respective edge; registration orders each side from the outside toward the
center. Each item must provide an `IconSource`, label and inline content width
in logical points. The bar places the icon before the content. Producers can
update `StatusItem.width` with their data or set `visible = false` to withdraw it.

When space runs out, lower-priority items collapse to their icons first. Higher
`priority` values retain text longer; ties collapse newer items first. Widening
the bar restores inline content. If even the icons cannot fit, the row scrolls
horizontally without a scrollbar. Optional tooltip systems remain available
while collapsed; otherwise the accessible label is shown as a tooltip.

Inline and tooltip systems both receive `UiFrame.view == item`. Inline systems
run once while their content is shown, and tooltip systems run only while the
tooltip is open. Keep ongoing plugin updates in ordinary Bevy systems. Declared
widths let UI2 allocate space before drawing, without replaying stateful renderers
for measurement. Content is clipped to its allocation. `StatusItemPresentation`
reports slot/icon/content geometry, collapse and tooltip visibility. Items belong
to their bar; tooltip systems belong to their item, so despawning an item, bar or
surface retires the corresponding renderers.

## Input and clipboard

Interactive egui widgets claim pointer input before panel systems; viewport image
regions explicitly delegate it. `UiFrame::text_edit` reserves typing, navigation, editing, clipboard and IME
input, including no-op edits and Enter that ends focus. Unrelated command chords
remain available. Unannotated focused text widgets conservatively reserve
keyboard input. `claim_key` and `claim_pointer` let custom UI gestures claim their
input explicitly. Source events and claims are retained by `input::InputRouter`;
egui's event list alone is not a complete consumption report.

`input::RoutedInput` contains remaining events, admitted held keys/buttons,
pointer position and wheel input. Consumers run in Update or after
`UiSystems::PublishInput`; raw Bevy input remains available to platform systems.
Workbench adds panel ownership and command resolution between Draw and Publish.

The existing `bedtime-clipboard2::Clipboard` is shared by UI and application
systems. An application-provided memory clipboard is preserved. UI2 bridges
Copy/Cut/Paste input and egui CopyText output; upstream OS clipboard management
is disabled. Clipboard initialization and operation errors retain the service's
normal semantics.

## Icons and viewports

Icons remain Bevy `Icon` and partial `IconSet` assets. A surface's `UiIcons` handle
selects a pack; missing slots inherit bundled artwork. Pending/failed replacement
packs preserve current artwork and report status through `UiIconStatus`.
Rasterization is cached by artwork revision and physical size; egui texture
handles own retirement. `BuiltinIcon`, explicit handles and `IconSource` remain
available. Theme changes affect tint, not SVG rasterization.

`UiViewport` embeds a standard Bevy image target. The scene retains its normal
cameras and post-processing. UI reports its content rectangle, visibility,
hover state and requested physical size; the application decides camera routing
and simulation policy. Viewport image regions can delegate input to a panel
input system. The independent scene still runs without Bedtime:

```sh
cargo run -p bed2-demo-game --example standalone
```

## Verification

```sh
cargo test -p bedtime-ui2
cargo run -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-egui-smoke
cargo run -p bed2 -- --workbench-smoke-dir target/workbench2-egui-smoke
```

The gallery fixture uses observed widget rectangles to exercise native-shaped
Bevy input, theme selection, dropdowns, overlays, resizing, icon replacement and
stable raster reuse. Native workbench smoke checks shell gestures, viewport
colors and linear alpha composition. Smoke uses a memory clipboard.

For desktop translucency, set Bevy's `Window.transparent = true` and
`composite_alpha_mode = CompositeAlphaMode::PostMultiplied` on macOS. The theme's
background alpha controls painted background opacity; text and ordinary widgets
retain their own alpha. With workbench, use `UiBackground::Transparent` so the
shell supplies that tint once across panels and gaps. The optional workbench
native window adapter supplies macOS blur via `WindowBlur`. For app-painted native
titlebars, use Bevy's `fullsize_content_view` with `titlebar_transparent` and
reserve the reported `WindowTitlebarInset` above application controls.
