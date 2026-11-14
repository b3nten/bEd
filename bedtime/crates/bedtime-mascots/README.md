# bedtime-mascots

Bed and Ducky scenes in the host's Bevy world, drawn through UI. The crate
installs no windows, application cameras, menus, layouts or shell helpers.

```rust,ignore
use bedtime_mascots::{Mascot, MascotKind, MascotPlugin, MascotScene};
use bedtime_ui::UiComponent;

app.add_plugins(MascotPlugin); // After the host's Bevy renderer and UI setup.
let scene = world.spawn(MascotScene::new(MascotKind::Bed)).id();
// Inside an ordinary UI drawing function:
Mascot::draw(ui, world, &context.for_view(scene), scene);
```

Each scene owns interaction, animation and playback state. Scheduled preparation
creates a linked subtree containing its camera, lights and geometry, plus a
`UiViewport` image on the scene entity. A scene is valid before GPU preparation;
drawing shows pending preparation or a model-loading error explicitly. The host
must supply Bevy asset/PBR/render plugins and UI. No v1 frontend or renderer is
installed. Both the bed GLB and click sound are embedded in the crate.

The scene system chooses a currently unused nonzero render layer from the world.
Hosts should reserve any additional layers before spawning mascots and avoid
assigning a live mascot's layer to unrelated geometry or lights. Despawning a
scene retires its linked entities and image handles; removing `MascotScene` also
retires its rendering components at the next deferred-command boundary. Attach
embedded scenes to their owning UI view with `ChildOf` when they should close
with that view; supply `Transform` on every parent in that hierarchy so Bevy can
propagate the camera and model transforms. Bed instances share immutable model assets, with separate mutable
blanket tint materials. Ducky also shares its sphere mesh and immutable materials. Shared model caches
live until the app is dropped.

Pointer tracking, click squash, framing, lighting and geometry follow the former application.
Primary-button presses inside an eligible canvas trigger sound and animation;
popups, modals and unfocused windows cannot trigger clicks. UI's theme accent
colors the bed's patterned cover. Reduced motion gives a neutral stationary pose
while keeping click audio. Hidden scenes disable cameras and pause their clock;
resuming never simulates the hidden interval. Animated visible views request
repaints at 30 Hz. UI owns image resize, texture registration and rounded clipping.

Audio opens one output stream lazily on the first click. Each scene owns a sink;
rapid clicks replace its previous sound. Closure stops playback. Device failure
logs once and leaves the visuals usable; it is retried on the next application
launch. Bed uses the bundled MP3; Ducky uses the same synthesized squeak as the former application.
No audio device is needed to create or restore scenes.

## Panels

`BedPanel` and `DuckPanel` (labels **Bed** and **Ducky**) are workbench
panels; spawn either with `InArea(area)`. Required components install
`PanelInstance` and `MascotScene` at once. They're saved with the layout
(feature `persistence`, default). Hidden panels suspend their viewports.

bEd opens Bed from View ▸ Bed. Ducky is an easter egg: `+ducky` in a terminal
opens (or focuses) it in the current layout's largest area.

## Verification

```sh
cargo test -p bedtime-mascots
python3 tools/bed.py run tests/automation/mascots.py
```

The native smoke uses an isolated configuration directory and verifies rendered
output, interactions, themes, resize, hidden layouts, cube coexistence,
shell routing, cleanup and session restoration. Captures do not verify audible
output; audio samples and MP3 decoding have deterministic coverage. Native
platform and audible checks must be reported separately when performed.

The asset bytes and procedural model were ported from `bedtime-plugin-mascots`.
The bed model is **Bed Low Poly** by **mezuna**, licensed under CC BY 4.0; its
original model, author and license links are preserved in
[`resources/licence.txt`](resources/licence.txt). Rendering neutralizes the tinted
blanket texture before applying the theme accent. Rust code retains the
repository's GPL-3.0-only license. Welcome and bedtime idle scenes are outside
this crate's current contract.
