# Step 2: UI foundation and widget gallery

This records the implementation at that migration stage. The later
[workbench split](step-3-split.md) retires `bedtime-desktop2`, moves native
integration into workbench and extracts an independent clipboard crate.

This step implements `bedtime-ui2` alongside the existing framework. The runnable
target is a gallery of reusable controls, animations, and Everforest Medium light
and dark themes. `bed2` embeds it with the desktop and command plugins; a standalone
example proves that UI does not require workbench.

## Decisions carried into the implementation

- Keep shared application state, surface configuration and theme selection in ECS.
  Keep control interpolation private to the UI runtime; form state used only by a
  view can live in a struct or Bevy `Local<T>`.
- Retain immediate drawing: ImGui owns layout and interaction machinery, while
  entities give views, themes and action placements identities.
- Supply themes as ordinary data. Consumers own selection, identity and storage;
  the UI crate assigns no stable theme keys and requires no theme registry.
- Make built-in controls the convenient route for ordinary screens, while leaving
  ImGui available for specialized layout and painting.
- Prepare the shared style before drawing. Ordinary controls inherit it; variants
  change only their differences. Closed context menus perform no style setup and
  never invoke their contents closure. This avoids v1's repeated full menu-style
  construction for every file-tree row.

## Data and ownership

| Data | Owner / representation |
| --- | --- |
| UI destination | `UiSurface` on an application-owned Bevy window. |
| Theme | A `Theme` component with semantic palette and metrics. |
| Theme selection | Non-owning `UsesTheme` / `ThemeUsers` relationship. |
| Icon artwork and packs | Bevy `Icon` / `IconSet` assets, referenced by strong handles. |
| Icon selection and load status | `UiIcons` and `UiIconStatus` on the surface. Missing selection means bundled icons. |
| Scene viewport | `UiViewport` component holding a standard Bevy `Handle<Image>` and current layout observations. |
| Ordered drawing | Registered Bevy system entities marked `UiView`, owned by `OnSurface` / `SurfaceViews`. |
| View state | Application components when shared; structs or Bevy `Local<T>` when private to drawing. |
| View-owned objects | `InView` / `ViewParts`, with recursive cleanup. |
| Motion policy | `MotionPolicy` on the surface. |
| Control/transient animation | Private context-owned cache, keyed by view, ImGui item ID and animation kind. |
| Input capture and diagnostics | `UiCapture` and `UiDiagnostics` on the surface. |
| Native/GPU machinery | Private context, renderer and frame-transport storage. |

The private runtime owns native context, renderer handoff and animation cache;
views and themes remain ECS data. Built-in controls take borrowed values and
manage motion automatically, without animation entities, registrations or queries.
Only submitted controls retain cache entries, with a one-frame grace period;
removed views release theirs and hidden controls stop requesting redraws.
Deleting a surface deletes its view/system entities and their owned parts;
referenced theme entities survive independently.

## Frame and rendering boundary

The application installs Bevy's plugins, then `DesktopPlugin` and `UiPlugin`,
selects a theme for its window and supplies a camera using `UiGraph`.

In `PostUpdate`, `UiSystems::Draw` owns one controlled frame. Controls sample and
retarget their local animations as they draw. Motion uses Bevy's real clock and keeps reactive windows redrawing while
transitions are active, independently of paused/scaled simulation time. The
frame owner adapts native input, prepares the selected theme and fonts, begins
ImGui, invokes the ordered registered drawing systems, records capture state and
finishes an owned draw snapshot. `UiFrame` is borrowed Bevy system input, allowing
ordinary queries/resources without retaining a frame pointer in the ECS world.

The renderer uses Bevy's device and queue. Owned snapshots cross extraction; raw
ImGui pointers stay on the UI thread. A full-window UI render graph draws to an
intermediate target and composes into the actual camera output with explicit
sRGB handling. Unrendered snapshots are reconciled, resizing does not stretch old
geometry into new dimensions, and packets are associated with device generations.
`UiRenderSystems::Draw` and `Composite` separate the transparent UI layer from
window presentation. UI camera clearing follows Bevy's clear policy, and
premultiplied alpha is decoded and blended correctly on sRGB output. An ordinary
cleared camera serves desktop apps; a later UI camera with `ClearColorConfig::None`
preserves an earlier scene camera for full-window UI overlays.

UI adapts pointer, keyboard, text, focus, scrolling, cursor, clipboard and IME
caret requests. The clipboard adapter temporarily borrows the desktop service
during the controlled draw transaction. Applications honor `UiCapture`; UI does
not erase Bevy's global input state.

## Commands

Workbench ingress now runs in `PostUpdate`, immediately before its dispatcher.
`bed2` orders `UiSystems::Draw` before `CommandSystems::Ingress`. Native menus
and UI placements can therefore submit to the same dispatcher in the same frame,
after the ImGui frame borrow has ended. Workbench retains its headless dependency
boundary and does not depend on UI.

The gallery's optional action placements use `Activates` to reference registered
commands. Its Copy Greeting button and native menu share availability; a second
button toggles that shared state.

## Gallery and scope

Run either host from the repository root:

```sh
cargo run -p bed2
cargo run -p bedtime-ui2 --example gallery
```

The gallery includes buttons and disabled states, asset-backed SVG icons, checkboxes,
animated hover/toggle/disclosure controls, text fields, selectors, sliders, form
rows, semantic feedback, tooltips, context menus, a modal dialog, palette swatches,
live theme and motion controls, and a clipped list containing 10,000 rows.
Its icon browser filters the bundled catalog, varies preview size and switches
between the default artwork and a sample partial replacement pack at runtime.

Stage 2 supports one active surface/context, bundled Paper Mono typography and
scalar motion. Multiple detached OS viewports, custom title bars, additional font
families/fallbacks, general image/canvas integration, editor GPU effects,
workbench panels and document widgets remain future work. No v1 runtime is used
by the new packages.

The v1 tooltip/popover behavior is the motion reference: tooltips scale from the
source control's anchor and context menus from their opening anchor, starting at
0.96 scale while fading in. Dialogs fade without scaling. Motion and source identity
live in the private control cache. After drawing, the frame owner
resolves current native window IDs and transforms completed vertices and clip
rectangles, including child windows/custom painting, while preserving native
layout and hit testing. Cached state retains no native pointers and the implementation
does not use a native animation hook. Closed row menus perform no animation or
style setup. Default buttons animate hover; toggles animate their knob and fill;
disclosures measure and animate their content without a caller-supplied height.

Popup spacing follows v1's file-tree menu: 8×6 padding, 6×4 spacing, seven-point
rounding and a one-point border. Tooltips use 12×10 padding. Context menus have
action rows without a gallery heading. Switches draw a pill without a second
button frame. Selectors use native buttons, popup windows and selectable rows,
leaving a four-point gap and choosing above/below placement within the viewport.
The UI projects the theme background onto desktop's `WindowBackground` component;
desktop applies it to AppKit's backing color, filling transparent title bars.

## Icon assets and pack switching

`Icon` owns a validated, immutable SVG tree; `IconSet` maps fixed `BuiltinIcon`
slots to artwork handles. All 126 v1 icons and their attribution/licenses are
included, plus three original Sun/Moon/Save glyphs. Bevy's asset stores own these
shared assets. Individual icon instances do not become entities. `IconSource`
can be a component on an application object or an immediate control argument.

`UiIcons(Handle<IconSet>)` selects a pack for a surface independently of its theme.
An absent component selects bundled artwork. Partial packs always inherit omitted
slots from the bundle. Explicit artwork handles bypass pack selection.
The `.svg` and `.icons.json` loaders use Bevy's asset sources and dependency
tracking; manifest paths resolve relative to the manifest. Unknown slots and
invalid SVGs fail at loading boundaries. Programmatic asset construction is also
supported. A replacement becomes active only once its complete dependencies and
rasters are ready. Until then, the previous pack remains displayed.
`UiIconStatus` reports actual selection/loading/errors separately from the
requested selection. New errors also produce `UiError` observations.

The private cache retains managed textures keyed by asset, immutable artwork
revision and physical pixel size. Normal-size rasters are prepared before drawing;
new large sizes can use those for one frame before receiving an exact raster.
Existing visible sizes are prepared against a replacement pack before committing.
Tint and current style alpha are applied while drawing, so theme changes do not
rebuild textures. Superseded artwork, abandoned sizes and retired surfaces release
textures through the managed snapshot lifecycle, preserving in-flight frames.
`UiDiagnostics` exposes cache counts for checking repeated-row behavior.

## Bevy scene embedding and compositor validation

`viewport_image` supplies a normal Bevy `Image` render target. `UiViewport` records
its handle, desired physical size, logical rectangle, visibility and hover state.
The application keeps ownership of scene cameras, simulation and input admission.
Controls draw the completed target inside an ImGui panel. `PrepareTargets` follows
UI drawing and runs before camera updates, resizing visible images and refreshing
their camera projections together. Hidden cameras can be suspended by the host.

Scene render schedules remain normal Bevy schedules, including HDR/MSAA and game
post-processing. Their final viewport output is resolved, tone-mapped RGBA/BGRA8
with compatible sRGB and encoded-color views. Private texture registrations bind
the current render-world image view before UI drawing and hold snapshot leases
through encoding; this handles resize/replacement without stale views or early
unregistration. Repeated frames reuse registrations and hidden viewports retire
them after a short grace period.

`bed2-demo-game` is a separate Bevy-only crate with a spinning lit cube and a
custom fullscreen color-grade shader. `bed2` adds the editor adapter that routes
the same camera to a panel texture or the window. The latter mode draws floating
controls and a full-window HUD above the scene. No Bedtime API appears in the game
crate. Its standalone example installs the same game plugin without Bedtime.
The existing widget-gallery example remains a UI-only app.

## Validation

Focused headless tests cover borrowed drawing-system execution, ownership cleanup,
theme validation/switching, transition continuity/reduced motion, control identity
and retirement, closed/open menu
contents, list clipping, keyboard/mouse mapping, copied IME caret lifetime, and
same-frame UI action ingress. Existing desktop/command tests remain applicable.
Icon tests cover the full catalog, partial packs, explicit artwork, pending and
failed dependency loads through the real asset server, same-handle SVG replacement,
normal/high-DPI raster sizes, repeated-row reuse and surface-cache retirement.
Viewport tests cover target creation, immediate camera refresh when resizing, and
preserving hidden target dimensions. Native screenshot checks cover cube pixels,
rotation, actual ImGui resizing, matching panel/window colors, linear-light UI
alpha blending and close/reopen. The standalone game is verified separately.

```sh
cargo test --offline --locked -p bedtime-ui2 -p bedtime-desktop2 -p bedtime-workbench2 --lib
cargo check --offline --locked -p bedtime-ui2 -p bed2 --all-targets
python3 scripts/check-crate-boundaries.py --offline
cargo run --offline --locked -p bedtime-ui2 --example gallery -- --smoke-dir target/ui2-smoke
cargo run --offline --locked -p bed2 -- --viewport-smoke-dir target/ui2-viewport-smoke
cargo run --offline --locked -p bed2-demo-game --example standalone -- --smoke-dir target/ui2-standalone-smoke
```

The macOS smoke path exercises native-shaped input through the actual adapter,
light/dark rendering, animation state, dropdown selection, context menus, resizing
and Bevy screenshot capture, plus pack swaps, artwork updates and stable icon-cache
counts. Its artifacts are `dark.png`, `tooltip.png`, `light.png`, `motion.png`,
`dropdown.png`, `menu.png`, `resized.png`, `icons-bundled.png`, `icons-custom.png`,
`icons-updated.png` and `icons-restored.png` under the
requested directory. Linux native behavior, cross-monitor DPI transitions and
device-loss recovery are not established by that run.
