# bedtime-model2

Embeddable glTF/GLB and STL viewing in the host's Bevy world, drawn with UI2.
The canvas installs no windows, file loading, application file associations or
session storage. Its optional Workbench document adapter supplies managed loading
and bounded native sidecar access.

Register `register_asset_source(&mut app)` **before** the host's `AssetPlugin`;
install `ModelPlugin` after the host renderer and UI2. The host supplies Bevy's
asset, glTF, PBR, anti-aliasing and render plugins (ordinary `DefaultPlugins` with
this crate's features provides these). The dedicated memory source contains only
validated, self-contained model snapshots and never loads model sidecars itself.

On a host-owned worker, call `PreparedModel::prepare(bytes, path, resolve)`.
`resolve` receives a decoded relative resource path; the host reads it relative
to the model directory and must bound each read to 64 MiB. The host owns
cancellation, dependency observation and acceptance of obsolete results. Share
prepared content with `Arc`. Attach `ModelScene` to a view, call `set_model`, then
call `ModelView::show(ui, world, entity)`. Its optional canvas response supports
ordinary host context menus. `info`, `error`, `is_ready`, `is_loading` and
`generation` expose scene loading/inspection state; readiness means the scene is
instantiated, rather than a guarantee that asynchronous shader compilation has
finished. `view` contains typed camera/appearance state. Validate deserialized
`ViewState` at the host's persistence boundary.

`ModelScene` requires a transform and visibility. Each instance owns a linked
subtree with its camera, lighting, imported geometry and debug geometry. Use
identity transforms for model-view ancestors. A free nonzero render layer is
chosen from the actual world; hosts must reserve custom layers before creating
model scenes and must not reuse a live scene's layer for unrelated content.
Imported cameras, lights and animation playback are disabled, matching bed1.
Bevy retains authored geometry/materials and GPU skeleton posing. Inspection
uses the authored pose to calculate bounds and display debug meshes.

The last valid scene remains during replacement loading and errors. Camera and
appearance survive replacement; use a fresh `ModelScene` for a different source.
Hidden viewports disable cameras and scene visibility. Removing `ModelScene` or
its owning entity retires linked scene entities, target handles and pending
memory assets. Immutable environment assets remain shared until application
shutdown. UI2 owns target resizing and weak egui texture registration.

## Controls and rendering

Frame All and double-click fit the model. Primary-drag orbits, right/middle-drag
pans and wheel input zooms. Popups and unfocused windows suppress canvas input.
Hosts supply keyboard shortcut policy; `zoom` and `view.camera = None` support
zoom steps and reframing. The canvas fills its allocation. Frame All, Info and Appearance float at the
top-left; a smaller muted interaction hint sits at the bottom-right. Controls,
loading messages and errors overlay the canvas without reserving a toolbar row.

Appearance includes Shaded, Wireframe, Wireframe overlay and Normals; sampled
normal vectors (at most 6,000); Studio, Outdoor, Neutral and two embedded Poly
Haven HDRIs; skybox visibility, blur and horizon; shadows, ambient occlusion and
exposure. Background blur leaves environment lighting/reflections sharp. Wire
views use portable line meshes. Ambient occlusion is disabled when the adapter's
storage-texture limit cannot support it. Defaults match bed1.

Preparation ports bed1's Draco, meshopt, quantization, WebP, material validation,
UV/tangent corrections and STL import. Unsupported required extensions fail;
unsupported material details produce the existing compatibility warnings.
Animations are counted, with no playback controls. Limits remain 128 MiB glTF
input, 64 MiB STL/input resources, one million vertices, three million indices,
100,000 nodes and 512 MiB total decoded texture allocation, with existing
hierarchy and numeric-range validation. Embedded/data-URI resources work without
a resolver. Relative local sidecars, including parent-directory references,
are supported; absolute paths, URL schemes and malformed URI escapes are rejected.
Only selected texture sources are resolved, preserving unused WebP fallbacks.

Enable `workbench` for the minimal `workbench::ModelPanel`, or compose
`document::ModelPreview` in an existing document panel. The latter owns managed
root/sidecar preparation and revision/dependency checks. bEd2 uses it and supplies
viewer associations, placement, context menus and persistence.

## Verification

```sh
cargo test --offline --locked -p bedtime-model2
cargo test --offline --locked -p bedtime-model2 --features workbench
cargo check --offline --locked -p bedtime-model2 --no-default-features --example standalone
cargo test --offline --locked -p bed2 --features automation
cargo build --offline --locked -p bed2 --features automation
python3 -B bedtime2/tools/smoke_models.py
cargo run --offline --locked -p bedtime-model2 --example standalone
```

The standalone example creates two independent views without Workbench. Native
smoke runs use isolated application configuration; native platform checks must
be reported separately from deterministic/headless tests.

Implementation and assets are ported from `bedtime-plugin-model-viewer`. HDRI
sources/licenses remain in [resources/environments/README.md](resources/environments/README.md)
and the repository NOTICE. STL triangle expansion retains the existing
[bevy_stl MIT notice](../../../licenses/bevy_stl/LICENSE). Rust code is GPL-3.0-only.

With `workbench`, `document::ModelPreview` supplies managed preparation from
canonical text/binary documents, bounded native sidecars, dependency observation
and revision-checked acceptance. Textual glTF follows unsaved edits; binary roots
use version-bound document byte access. The host supplies menus and placement.
