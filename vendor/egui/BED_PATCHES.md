# Bed's egui patches

This is the egui **0.36.2** crate, copied from the crates.io package.
Upstream repository: https://github.com/emilk/egui
Upstream commit: `49682f8baa058bf49e011035cfbd6e825f88a5ef`, `crates/egui`.
Package SHA-256: `dc938cc27cd911415e1e4d151bc56ff9df063d8db081374e87aeb805ea74b2ba`.
The original manifest is retained as `Cargo.toml.orig`. The normalized manifest
uses local license paths. Source is licensed MIT OR Apache-2.0; both licenses
are included.

The published crate omits images required by its doctests. `assets/ferris.png`
and `assets/icon.png` are copied unmodified from this same upstream commit's
`crates/egui/assets/ferris.png` and `crates/eframe/data/icon.png`. Three atomic
layout documentation examples reference the local icon instead of reaching
outside the crate. The manifest includes these fixtures for standalone builds.

The root Cargo patch supplies the same egui package to UI2, workbench and
upstream bevy_egui. Public upstream signatures are preserved. Neither bevy_egui
nor emath/epaint is vendored or modified.

## Widget transitions

`src/widget_animation.rs` owns paint-color transitions, scoped by context,
viewport and widget ID. Widgets resolve their final target style first,
including scoped colors, explicit fills/strokes, selection and menu styles.
The transition uses cubic ease-out and the effective `Style::animation_time`.
A zero duration settles immediately. Retargeting starts from the current
displayed color; first appearances start settled. Removed widgets retire their
state after a missed frame. `Context::clear_animations` clears this state too.

Built-in paint paths use the shared implementation:

- Buttons, image buttons, selectable buttons, menu/submenu buttons and DragValue.
- Checkbox/radio interaction colors and fading checkmarks/dots.
- Slider and color-picker handles; color values and drag positions remain direct.
- TextEdit focus/background/border, interactive labels and link underlines.
- ComboBox open/hover/press colors, collapsing headers and resize grips.
- Scrollbar handle/track colors and opacity, panel resize separators, window
  close/resize controls and active titlebar fills.
- Drag-and-drop destination feedback.

Layout, hit bounds, input handling, font size, glyph meshes and UVs do not
participate in these transitions. Existing disclosure and scrolling motion
remain upstream. There is no blanket animation of data values, caret
blinking or text selection. Disabled input remains disabled immediately.

Existing `Style::interact` and `interact_selectable` retain their reference/value
contracts for external custom painters. The additive `interact_animated` and
`interact_selectable_animated` helpers return animated paint visuals.
`Context::animate_color_with_time` supports custom paint colors without another
animation implementation. Built-in widgets opt in internally, so existing
`ui.button`, `ui.checkbox`, `ui.add(Slider::new(...))`, etc. need no caller changes.

## Dropdown behavior retained from UI2

ComboBox popups use half the vertical item spacing as their anchor gap and hide
their scrollbars. Height limits and wheel scrolling remain intact. These are
the two presentation changes previously maintained in UI2's copied ComboBox;
UI2 now re-exports this crate's ComboBox.

Selectable rows and other buttons with `frame_when_inactive(false)` retain
their frame's border width and margins while hiding its colors. Removing the
inactive border also removed layout space already accounted for by the padding,
so hovering an unselected dropdown item shifted its text by one point and
changed its natural width by two points. Hidden and visible frames now use the
same geometry; color transitions still run normally.

## Popup opening motion

`Popup::show` owns opening motion for native menus, submenus, ComboBoxes,
tooltips and other popups. Bed1 is the reference: scale from 0.96 to 1.0 and
fade from transparent to opaque with cubic ease-out. The effective duration is
`Style::animation_time` (Bed1 used 150 ms; UI2 currently defaults to 160 ms).
Zero duration settles immediately. The native Area fade is disabled for these
popups so there is only one opening curve. Closing remains immediate.

The pivot is the source point of egui's chosen placement, clamped to the final
popup bounds. Native placement still flips at window edges. Layout and hit
geometry use final coordinates. Only the Frame's reserved background shape
(fill, border and shadow) and its clip rectangle receive the temporary scale
transform. All contents, including text, icons and widgets, fade at their final
geometry and pixel positions. The Frame exposes that reserved shape index only
within the crate; its public API is unchanged. No font substitution, glyph
compensation, custom resampling or epaint changes are needed.

As in Bed1, pointer interaction inside a visible popup or keyboard input settles
growth while the fade continues. Reopening, a missed frame or a changed anchor
starts a fresh opening. State belongs to the popup's context and ID, and
`Context::clear_animations` clears it. No application widget call changes are
required. Dialogs keep their existing Area motion.

## Verification and upgrades

Custom text canvases can call the additive `Context::register_text_input(id)`.
`text_edit_focused` recognizes the ordinary focused ID without a hidden TextEdit
buffer/history. UI2 and Workbench use the existing text capture/shortcut policy
for the virtualized editor; this adds no event-routing or second focus owner.

From the repository root:

```sh
cargo test -p egui --offline
cargo test -p bedtime-ui2 -p bedtime-workbench2 --offline
cargo check -p bed2 --all-targets --offline
cargo run -p bedtime-ui2 --example gallery --offline -- --smoke-dir target/egui-vendor-gallery-smoke
```

The added regression tests live in `src/widget_animation/tests.rs`. They verify
intermediate native widget colors, continuous retargeting, immediate editing
and click behavior, reduced motion, stable layout and state retirement. Selectable
row coverage checks both text positions and bounds through hover, press and exit,
at natural and expanded widths.
`src/containers/popup_tests.rs` checks popup pivots, easing, clip transforms,
stable input geometry, reopening, interaction and reduced motion. UI2 also
checks that opening tooltips keep identical glyph positions, bounds, layout
jobs and texture coordinates while opacity changes. The gallery smoke captures opening,
late and settled menus/tooltips at a deterministic 60 Hz.

When upgrading, compare against the recorded upstream package, reapply only the
changes described here, update provenance and run these checks. Keep
application themes, Bevy integration and workbench semantics outside this crate.
