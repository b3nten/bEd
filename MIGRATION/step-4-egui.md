# Stage 4: egui and contextual input

Status: implemented, with automated and native Metal validation (2026-10-16).
This stage supersedes the ImGui integration direction in stages 2 and 3; those
documents remain implementation history.

## Implemented milestones

1. Replace v2's ImGui integration with upstream `bevy_egui` 0.43.1 and vendored
   egui 0.36.2. Preserve themes, PaperMono, icons, viewport cameras, native window
   appearance and the panel/layout model. Ordinary controls use egui; shell
   geometry and gestures retain Bed behavior. UI2 remains independent of workbench
   commands.
2. Add explicit panel input ownership and replace action types with independently
   registered ContextCommands, bindings and handlers. The UI migration remains
   usable independently of this command model.

V1 remains unchanged. Applications own Bevy windows, cameras and renderer.
`UiCamera(surface)` attaches the upstream integration to a selected camera.
Bed controls frame scheduling; upstream owns rendering, texture integration,
platform input, cursor and IME. A single drawing pass avoids repeating application
mutations. Bed's independent clipboard service remains authoritative.

## Presentation

Keep theme entities and semantic palette/metrics. Use native egui widgets with
small additions for text ownership, icons, viewports and motion. Preserve panel
gaps/borders, tabs, focus, split/join/resize/regroup/relocate gestures and previews.
Keep source-anchored scale/fade, 160 ms cubic ease-out, continuous retargeting and
reduced motion. Preserve final hit geometry during visual animation.

## Input and ContextCommands

Input order is UI/shell, panel input, binding lookup, command dispatch, queued
workbench edits, remaining application input. Preserve source-event identity and
ordered events; routed held state derives from admitted presses. Consumption is
explicit, not inferred solely from egui's remaining events. Bed text helpers
reserve editing input; unannotated focused text widgets conservatively capture
keyboard input. Vendored egui changes presentation while retaining upstream input semantics.

Panel input systems can consume events and capture keyboard/pointer streams.
Keyboard targets focused content; pointer targets its region or capture owner.
Capture survives frames and is cancelled on focus loss, hiding, closing or layout
switching. A game viewport can own Cmd+S before any workbench shortcut lookup.
Raw Bevy input is retained; application actions use routed input.

`ContextCommandId` is a serializable namespaced string. Command metadata is
registered independently. Bindings, registered system handlers and menu items
reference that identity. Bindings and handlers each resolve independently:
focused panel first, then workbench. A selected binding reserves its chord;
missing/disabled/failed handlers never retry a different binding. Disabled local
handlers block workbench fallback. Same-scope binding conflicts reserve their
chord, execute nothing and produce a diagnostic. Modifiers match exactly;
physical KeyCode identity is retained and repeats default to disabled.

Capture invocation context before dispatch. Context menus retain their target
panel; workbench menus use the focused panel. Availability and shortcut labels
use that same context and effective bindings, excluding shadowed shortcuts.
Dispatch runs one admitted batch; handler follow-ups wait for the next pass.
The future concrete `Workbench::Command` API is outside this stage.

Native menus remain clickable. Shortcut hints are label text: native accelerators
do not create a second keyboard execution path.

## Acceptance

- Standalone UI gallery, workbench and standalone Bevy game remain usable.
- Headless core and native services without UI compile; bed2 has no ImGui
  dependency path and v1 still builds.
- Preserve behavioral layout, motion, icon, stale-request and ownership tests.
- Native smoke covers panel gestures, viewport color/alpha/resize, native/in-app
  menus and plugin composition in either order, without changing OS clipboard.
- Check shell captures at matching size/theme/scale; ordinary controls receive
  aesthetic review instead of pixel matching.
- Test text/IME, no-op editing shortcuts, Enter/focus changes, exact modifiers,
  repeat, independent binding/handler override and effective menu hints.
- Exercise Shift+drag, capture cancellation, game-owned Cmd+S and input forwarding
  into a separate small Bevy world; no double actions or stuck held input.

Single-surface support remains the initial limit. Documents, persistence,
shortcut-editing UI, multi-stroke bindings and moving scene rendering into a
separate application remain outside this migration.

## Implementation and validation

UI2 now uses egui 0.36.2 / bevy_egui 0.43.1. The v2 ImGui renderer, Winit
adapter, texture bridge and control facade are removed. Upstream integration
supplies the render infrastructure. Bed overrides its shader asset to convert
premultiplied sRGB colors correctly for linear-light composition. UI cameras use
premultiplied output; scene overlays clear their intermediate UI target to
transparent every frame before compositing over the application's window camera.

The command API uses `ContextCommand`, `ContextCommandId`, `Binding`, `Handler`,
`Invokes` and `InvocationContext`. `register_panel_input` supplies borrowed
`PanelInput`; `RoutedInput` is the filtered application/guest-world state.
Native menu activations retain their installed context and revalidate current
source ownership/availability. A held press keeps its owner through release;
cancellation delivers released state even when a panel becomes hidden.

Validation includes 79 tests across UI2 (15), workbench (62) and clipboard (2),
plus 53 workbench tests with default features disabled. Coverage includes:

- Widget pointer priority and explicit viewport delegation; UI/theme, icon lifecycle and DPI, one-pass drawing, text/IME ownership, popup
  glyph stability during native fading, dropdown gap/scrolling and reduced motion.
- Workbench geometry/gestures, stale requests, serialized identity, independent
  binding/handler overrides, disabled/failing handlers, exact modifiers, repeat,
  conflicts, effective menu hints, modal priority and guest-world input tests.
- Shift-drag beyond content bounds, capture cancellation, hidden-panel releases
  and game-owned Cmd+S before workbench dispatch.
- Native UI-only gallery: pointer clicks, light/dark themes, dropdown selection,
  tooltip/menu/dialog captures, modal keyboard blocking, resizing and icon pack
  replacement/hot updates. Thirteen captures in `target/ui2-review-smoke`.
- Native workbench: all four tab-drop edges, regrouping, split/join/relocation,
  connected T-border resizing, factory drops, content-target priority, menus and
  clipboard availability. Viewport captures check rotation, resize, transparent
  linear alpha composition and close/recreate behavior. Explicit plugin
  composition with UI installed first also passes.
- Headless core, native services without UI, standalone Bevy scene and existing
  v1 application checks. `cargo tree -p bed2` contains no ImGui dependency path.

The standalone scene fixture counts warm-up frames after pipeline readiness;
counting frames during shader compilation could previously capture a black
startup frame. Native fixtures inject scripted pointer events before egui input processing,
then inspect presentation after drawing. Native smoke uses a memory clipboard.

One native run returned a blank resized viewport capture. Repeating the same
build passed all pixel and gesture checks; the cause of that intermittent
capture failure is not established. Keep this recorded for further GPU testing.

Run the migrated demo with `cargo run -p bed2`. See the updated
[UI2 README](../bedtime2/crates/bedtime-ui2/README.md) and
[workbench README](../bedtime2/crates/bedtime-workbench2/README.md) for the concrete
APIs and schedule. Visual captures have been inspected; exact cross-backend pixel
identity is not an automated acceptance criterion for ordinary widgets.

## Visual follow-up (2026-10-16)

Implemented review items 1–11, with the popup motion limitation below:
accent-filled primary buttons, dropdown spacing and hidden scrollbars, tabs
joined to their border with square lower corners, a foreground corner +, 20%
larger panel gaps, and rounded/inset borders even for a single panel.

Corner splits show both halves immediately. When tab drops or area moves create
splits, new areas appear at their final bounds and existing areas animate their
resize, in either direction. Direct border drags track the pointer; pure
relocation snaps. Tabs use their area's current geometry with no separate motion,
and panel contents have no entrance fade. Native gesture checks assert these
properties on the first presentation after a corner split and after an edge
drop, for all four edges.

The custom popup scale/pre-tessellation path did not resolve changing font
weight. It has been removed. Menus, tooltips and dropdowns now use egui's native
fade at the surface's motion duration, keeping glyph positions, size and UVs
constant. Anchor expansion is deferred under the request to avoid workarounds:
no font-weight compensation, additional popup render target or egui internals
patch is introduced.

Following the explicit allowance to vendor at most ComboBox and Popup, UI2 now
vendors only egui 0.36.2's `combo_box.rs`. The two behavioral edits set the gap
to half the vertical item spacing (default 4 points) and hide the ScrollArea's
scrollbar. The gallery and workspace/layout selectors use `bedtime_ui2::ComboBox`.
Popup's public gap API is sufficient; copying `popup.rs` would require private
`Context::pass_state_mut` access, so it remains upstream. No other egui modules
are copied. Provenance and MIT license accompany the local module.

The application can toggle `NativeMenuBar` at runtime; egui presents the same root
when native presentation is off. The demo exposes Native menu, Blur and Opacity
controls plus `--egui-menu` / `--blur` flags. `UiBackground::Transparent` delegates
background painting to the shell. The host paints its theme tint once across
panels, gaps and outer margins; the application paints the titlebar/toolbar tint
in the reserved top region. Alpha is neither omitted nor stacked in panel gaps.
On macOS, Bevy's `fullsize_content_view` with `titlebar_transparent` exposes
pixels behind native chrome, matching bed1's window setup. `WindowTitlebarInset`
reports the AppKit content layout reservation so controls sit below native chrome.
`WindowBlur` owns a passive, automatically resizing NSVisualEffectView below
Winit's unchanged GPU view. Window-wide opacity stays 1; GPU background pixels
carry the tint alpha. Bevy windows use transparent/PostMultiplied presentation.

Verification: UI2's 15 tests and workbench's 62 tests pass. The actual UiFrame
tooltip regression compares partially faded and settled glyph geometry; the
ComboBox regression checks a real anchor gap and retained wheel scrolling in a
long list. Native workbench checks passed with explicit plugin composition and
native/egui menu presentation. Panel, gap and titlebar readback each measured
217/255 alpha at 85% opacity, both with and without blur, and 255/255 at full
opacity with blur. Native scene overlays,
all four split edges with immediate new-panel bounds and unlagged tabs, corner
splits with both halves at their final bounds on the first presentation, joins,
T-border drags, panel lifecycle and native → egui → native menus passed. The
divider fixture now derives its pointer target from observed area geometry.
The widget gallery produced 13 inspected captures in `target/ui2-review-smoke`.
Bevy readback excludes OS-composited blur. Headless and native-without-UI builds
remain supported.

The corner-split follow-up passed all 62 workbench tests and the native smoke
with `--blur --egui-menu --ui-first --compose-workbench`; captures are in
`target/workbench-corner-split-smoke`. An initial native attempt stopped at the
earlier viewport divider-drag assertion; the unchanged rerun passed.

### Controls and menu demo follow-up

The application controls now live in an ordinary Controls panel below the gallery
and scene. Initial and newly created demo layouts include it. Controls can be
moved, grouped or closed; View → Open Controls focuses or restores it. Wrapped
rows and scrolling keep controls reachable after docking into a smaller area.
Only the native titlebar reservation and optional egui menu bar remain above
the panels, with their background sized from the current pass.

View, Layout and Workspace menus expose panel opening and container management
through ContextCommands. Each Notes panel contributes a focus-only Notes menu
with copy, insert-sample and clear handlers. Its copy binding overrides the
global greeting binding. Shared menu snapshots filter `InPanel` entries by
context, including native activation revalidation after focus changes. Panel
ownership retires menu trees, bindings and handlers on close.

`UiFrame::primary_button` styles egui's normal inactive/hovered/active visuals
inside a local scope. It no longer fixes the fill across all interaction states.
Tabs match bed1's transparent normal/selected fills and 12% text-color hover
tint. The tab-bar add button and its reserved width are removed; empty-area
context menus and the demo's View menu remain available.

Verification: 16 UI2 tests, 63 workbench tests, three ComboBox doctests and 54
headless workbench tests passed, as did `cargo check -p bed2 --all-targets --offline`.
Native smoke passed with the default native-menu setup and with
`--egui-menu --ui-first --compose-workbench`, both under `--blur`. It exercises
the Controls drag source, opening Notes through View, invoking the focused
panel's Copy Note Text handler, hiding the menu on focus change, and the existing
layout/viewport checks. Captures are in `target/workbench-controls-smoke` and
`target/workbench-controls-composed`. The standalone gallery passed and produced
13 captures in `target/ui2-controls-smoke`; light/dark styling and the embedded
gallery/control-panel layout were visually inspected.

### Panel content and status bar follow-up

Panel hosts now supply the complete content rectangle below their tab bars,
without content margins or automatic scrolling. Panels explicitly choose
`UiFrame::padded` and `scroll`. Theme `panel_padding` defaults to 14×12 points,
matching bed1's content spacing. Gallery, Notes and Controls opt in; the viewport
pads its text and fills the remaining area to its left, right and bottom edges.
Native egui image rounding preserves its bottom corners and shell borders draw
after content. Panel gaps and outer margins increase from 6 to 7.2 points.
Tab hover fills slide upward from the baseline at the surface's motion timing,
without translating tabs or text; reduced motion settles them immediately.

UI2 provides a status bar independent of workbench. Applications create its
surface view and reserve `status_bar_height(ui)`. Plugins register an item with
a required icon, left/right side, accessible label, inline width and Bevy draw
system, plus an optional expanded-tooltip system. Item-owned state remains
ordinary ECS data. Icons precede content. Lower-priority/newer items collapse to
icons when space runs out and expand when it returns; extreme icon overflow
scrolls horizontally. Explicit widths allocate before executing plugin systems,
so no renderer runs in a measurement pass. `UiFrame::tooltip_ui` shares native
tooltip timing/styling with the existing text helper. Bar/item/surface ownership
retires registered inline and tooltip systems. Bed2 contributes workspace,
focused-panel, rendering and motion items, and reserves the bar below its panels.

Verification: 19 UI2 tests, 63 workbench tests, three ComboBox doctests and 54
headless tests pass, along with the bed2 all-target check. Native checks pass
with both default presentation and `--egui-menu --ui-first --compose-workbench`,
under `--blur`. They check viewport edges, opt-in content padding, bar height and
icon ordering, real widget clicks, collapse/restore on resize, expanded tooltips,
and stationary tab geometry throughout hover. Panel/gap/titlebar/status pixels
each measured 217/255 at 85% opacity (with and without blur), and 255/255 at full
opacity. Captures are in `target/workbench-status-smoke` and
`target/workbench-status-composed`. Normal, collapsed-tooltip and early-hover
captures were inspected. One late settled-hover capture was blank, matching the
previously recorded readback issue; the gesture/geometry assertions passed.
The native fixture now reasserts scripted coordinates on press/release so real
cursor movement between scripted frames cannot redirect those events.

### Scrollbar, preview and color transitions

UI2's scroll helper uses egui's public `scroll_bar_rect` to stop its track short
of rounded corners without padding content. Corner-split lines use the source
surface's actual gap/window insets and account for stroke width; a public painter
clip also constrains transitions from a previous preview.

Panel focus borders, tab text/close colors, icon buttons and toggles fade using
the existing 160 ms cubic transition and continuous retargeting. Reduced motion
settles immediately. `UiFrame::animate_bool` shares that policy with custom
painting; layout and hit rectangles remain fixed during color changes.

Blanket color fades for stock egui widgets are deferred. Egui 0.36.2 selects
their state visuals inside each widget and provides no shared transition hook.
Ordinary buttons (including primary), checkboxes, sliders, menus and dropdowns
retain native state colors. No widget reimplementation, paint-output rewriting
or additional egui source changes are introduced.

Verification: 21 UI2 tests, 63 workbench tests and three ComboBox doctests pass.
Regression checks exercise intermediate hover/press/release/exit colors,
reduced motion, and scrollbar ends with an unchanged content viewport. The
native smoke passes with `--blur --egui-menu --ui-first --compose-workbench`,
including actual preview-line endpoints/clipping and the existing panel,
viewport, menu and status-bar checks. Captures in
`target/workbench-bounds-motion-smoke` were inspected, including the held
corner-split preview. The bed2 all-target check also passes.


### Workbench shell ownership

`attach_surface` now installs one workbench root on the application-selected
window. The host selects an explicit menu root and owns a UI2 status bar; plugins
register items on `WorkbenchHost::status_bar()`. The root reserves native chrome,
the optional egui menu and bottom status bar, then draws tiled panels in the
remaining rectangle. `bounds` optionally selects an inset outer allocation.
`WorkbenchPresentation` reports the resulting regions. The application's chrome
renderer and manually synchronized top/bottom insets are removed.

The root paints the shared background once and selects UI2's transparent
background policy on attachment. Full-window scene mode retains menu/status bands
while the central region remains transparent. UI2 status bars now expose
`create_status_bar` and `draw_status_bar`: the caller supplies an allocated Ui,
so the bar has no independent surface view or window-relative positioning.

Native titlebar measurement is independent of background and blur configuration
and runs on the main thread before UI drawing. The root converts the native
logical inset to egui points using the current zoom, reserving only the portion
inside its own bounds. AppKit appearance and passive blur lifecycle remain in
the native adapter. Drawing remains single-pass.

Verification: 22 UI2 and 64 workbench unit tests plus three doctests pass, including
status placement in an inset allocation and host-owned contribution retirement.
The 54 headless workbench tests and native-without-UI check pass. Native smoke
covers both native and egui menus, plugin composition, shell alpha, panel gestures,
scene overlays, status interactions, inset bounds and 1.5× UI zoom. Captures in
`target/workbench-shell-smoke` and `target/workbench-shell-composed` include the
inset and zoomed shell; those layouts were visually inspected.

### Vendored egui widget animations

The egui 0.36.2 crate is now in `vendor/egui`, selected with one root Cargo
patch for both UI2 and upstream bevy_egui. Public upstream signatures remain
compatible. [Provenance and patch notes](../vendor/egui/BED_PATCHES.md) record
the original package/commit, licenses, changed behavior and upgrade checks.

Built-in controls resolve their final target colors before fading them through
one implementation. This includes scoped primary-button colors, explicit
fills/strokes, selection, open dropdowns and menu styles. Hover/press/focus colors,
checkbox/radio marks, link underlines, scrollbars, resize feedback and active
window titlebars use cubic ease-out at `Style::animation_time`. UI2 already maps
surface MotionPolicy to that duration; zero settles immediately. Widget input,
data values and target layout remain direct. First appearances start settled,
rapid retargeting is continuous, hidden widgets retire their state, and
`Context::clear_animations` clears the new transitions as well.

UI2 removes its copied ComboBox in favor of a re-export. The existing dropdown
gap and hidden scrollbars now live in the fork. Icon buttons use native image
buttons instead of separately painted hover/press backgrounds. The custom
toggle retains its geometry and position motion, delegating its color fade to
egui. Primary-button accent styling remains theme data. Bed2 widget call sites
are unchanged. Native popup fading remains in place; this step introduces no
font scaling or glyph-rendering changes.

Verification: 56 egui unit tests (including six new animation regressions), 22
UI2 tests and 66 workbench tests pass, along with the bed2 all-target check.
Egui's documentation suite passes 167 examples with one upstream ignored test;
its omitted image fixtures are restored from the pinned upstream commit.
Tests cover native widget hover/press shades, selection, explicit fill
precedence, floating scrollbar opacity, immediate text editing/click semantics,
retargeting, reduced motion, state retirement and stable layout.
Native gallery and composed workbench smoke pass; captures in
`target/egui-vendor-gallery-smoke` and `target/egui-vendor-workbench-isolated`
were inspected. The workbench run checks blur/alpha, menus, viewport input,
panel gestures, status contributions, UI zoom and the theme editor.

Two initial workbench runs stopped at different menu/tooltip checks. The native
fixture now filters live OS cursor motion from egui's scripted input stream
after upstream input processing, preserving stationary frames for tooltip
timing. The isolated run passed all checks; application input code is unchanged.

### Source-based popup opening motion

Native menus, submenus, ComboBoxes, tooltips and popovers now grow from their
source inside vendored `Popup::show`; application call sites stay unchanged.
Bed1's opening animation supplies the reference: 0.96 initial scale, cubic
ease-out, matching opacity, and growth that settles on interaction while fading
continues. UI2 retains its configured motion duration (160 ms by default; Bed1
used 150 ms), with zero duration for reduced motion. Closing is immediate.

Egui still chooses placement and flips at window edges. Its chosen source point,
clamped to the final popup bounds, supplies the pivot. A temporary paint transform
scales the complete layer and clips; layout, hit geometry, menus and command
dispatch retain native coordinates. Text follows the ordinary shape transform,
without font substitution, glyph compensation, custom resampling or epaint
changes. The first version deliberately leaves the visible text-weight change
during scaling available for review. Dialogs retain their existing motion.

Verification: 60 egui unit tests, 22 UI2 tests, 67 workbench tests and the bed2
all-target check pass. Popup tests cover all kinds, multiple pivots, edge flipping,
the Bed1 curve, clipping, stable logical geometry, reopening, interaction and
reduced motion. UI2 verifies that tooltip glyph meshes scale while retaining
their layout jobs and UVs. Two layout test fixtures now explicitly select their
7.2-point gap rather than depending on the changed application default.

Native gallery and composed workbench smoke pass. The gallery uses deterministic
60 Hz timing and isolated scripted cursor input to capture opening, late and
settled frames in `target/egui-popup-scale-gallery`. Those captures were inspected:
tooltip text still appears heavier during growth than after settling, and is
left untouched as requested. The composed workbench run in
`target/egui-popup-scale-workbench` also passes menu dispatch, status tooltips,
panel gestures, shell alpha/blur, zoom, panel settings and theme editor checks.

### Popup contents fade at final geometry

The popup frame retains source-based growth and matching opacity, while all
contents now only fade. Text, icons and controls keep their final positions and
sizes throughout opening. `Popup::show` uses Frame's reserved background shape
to transform only its fill, border, shadow and clip. The shape index is visible
within the crate; no public API or application call sites change. Opening timing,
interaction settling, closing and reduced motion retain their existing policy.

The rendering path supports a bitmap-sampling explanation for the earlier
weight change: `TextShape::transform` scales an already laid-out glyph mesh
without rerasterizing the font atlas or changing UVs. Fractional scales change
the mapping between cached glyph texels and physical pixels. The tessellator
also rounds the text origin to pixels. Keeping content geometry fixed removes
those animation-induced sampling changes; opacity still varies naturally.

Verification: all 149 unit tests and the bed2 all-target check pass. Popup tests
now verify that frame geometry grows while content bounds, position and clip
remain fixed across menus, tooltips and popovers. UI2 checks exact glyph vertex
positions and UVs while opacity changes. Native gallery smoke passes with 17
captures in `target/egui-popup-content-fade-gallery`; opening and settled menu
and tooltip frames were inspected and show stable text geometry and weight.
The composed workbench smoke also passes in
`target/egui-popup-content-fade-workbench`, including menu dispatch and status
tooltips alongside the existing panel, viewport, blur/alpha and shell checks.
