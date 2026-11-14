# bedtime-model

Workbench glTF, GLB and STL viewer. Renders in the application's Bevy world.

```rust,ignore
app.add_plugins(ModelViewerPlugin); // after the renderer and UI
```

`ModelViewerPlugin` adds `ModelPlugin` and an opener (`ModelFile`, the file's
bytes). Each revision of the document is prepared on a worker (`prepare`,
reading a `.gltf`'s other files from beside it, up to 64 MiB each) and shown
once ready, keeping the camera. The scene fills the panel; floating at the top
right are Frame All, About This Model (counts, size, compression, warnings,
and how to move around) and Appearance. The right-click menu has Frame All,
Display, Lighting, Skybox, Shadows and Reset Appearance. Zoom In, Zoom Out
and Reset Zoom (Cmd/Ctrl+=, - and 0) move the camera and reframe: it handles
workbench's actions, rather than the panel zooming. Changes to a `.gltf`'s other files aren't watched.

## Embedded scenes

Install `ModelPlugin` after the host renderer and UI. The host supplies Bevy's
asset, glTF, PBR, anti-aliasing and render plugins (ordinary `DefaultPlugins` with
this crate's features provides these). Each revision is written as a
validated, self-contained snapshot to Bevy's `embedded://` source, under
`bedtime_model/snapshots/`, and never loads model sidecars itself.

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
Imported cameras, lights and animation playback are disabled.
Bevy retains authored geometry/materials and GPU skeleton posing. Inspection
uses the authored pose to calculate bounds and display debug meshes.

The last valid scene remains during replacement loading and errors. Camera and
appearance survive replacement; use a fresh `ModelScene` for a different source.
Hidden viewports disable cameras and scene visibility. Removing `ModelScene` or
its owning entity retires linked scene entities, target handles and pending
memory assets. Immutable environment assets remain shared until application
shutdown. UI owns target resizing and weak egui texture registration.

## Controls and rendering

Double-click (or `view.camera = None`) fits the model. Primary-drag orbits,
right/middle-drag pans, scrolling and pinching zoom. Popups and unfocused
windows suppress canvas input. `ModelView::show` draws the viewport and
handles the camera, returning the response for menus; the panel adds the
controls.

Appearance includes Shaded, Wireframe, Wireframe overlay and Normals; sampled
normal vectors (at most 6,000); Studio, Outdoor, Neutral and two embedded Poly
Haven HDRIs; skybox visibility, blur and horizon; shadows, ambient occlusion and
exposure. Background blur leaves environment lighting/reflections sharp. Wire
views use portable line meshes. Ambient occlusion is disabled when the adapter's
storage-texture limit cannot support it.

Preparation supports Draco, meshopt, quantization, WebP, material validation,
UV/tangent corrections and STL import. Unsupported required extensions fail;
unsupported material details produce compatibility warnings.
Animations are counted, with no playback controls. Limits: 128 MiB glTF
input, 64 MiB STL/input resources, one million vertices, three million indices,
100,000 nodes and 512 MiB total decoded texture allocation, with
hierarchy and numeric-range validation. Embedded/data-URI resources work without
a resolver. Relative local sidecars, including parent-directory references,
are supported; absolute paths, URL schemes and malformed URI escapes are rejected.
Only selected texture sources are resolved, preserving unused WebP fallbacks.

Enable `workbench` for the minimal `workbench::ModelPanel`, or compose
`document::ModelPreview` in an existing document panel. The latter owns managed
root/sidecar preparation and revision/dependency checks. bEd uses it and supplies
viewer associations, placement, context menus and persistence.

## Verification

```sh
cargo test --offline --locked -p bedtime-model
cargo test --offline --locked -p bedtime-model --features workbench
cargo check --offline --locked -p bedtime-model --no-default-features --example standalone_model
cargo test --offline --locked -p bed --features automation
cargo build --offline --locked -p bed --features automation
python3 -B bedtime/tools/smoke_models.py
cargo run --offline --locked -p bedtime-model --example standalone_model
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
