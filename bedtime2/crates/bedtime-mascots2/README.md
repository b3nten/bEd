# bedtime-mascots2

Bed and Ducky scenes in the host's Bevy world, drawn through UI2. The crate
installs no windows, application cameras, menus, layouts or shell helpers.

```rust,ignore
use bedtime_mascots2::{Mascot, MascotKind, MascotPlugin, MascotScene};
use bedtime_ui2::Component;

app.add_plugins(MascotPlugin); // After the host's Bevy renderer and UI2 setup.
let scene = world.spawn(MascotScene::new(MascotKind::Bed)).id();
// Inside an ordinary UI2 drawing function:
Mascot::draw(ui, world, &context.for_view(scene), scene);
```

Each scene owns interaction, animation and playback state. Scheduled preparation
creates a linked subtree containing its camera, lights and geometry, plus a
`UiViewport` image on the scene entity. A scene is valid before GPU preparation;
drawing shows pending preparation or a model-loading error explicitly. The host
must supply Bevy asset/PBR/render plugins and UI2. No v1 frontend or renderer is
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

Pointer tracking, click squash, framing, lighting and geometry follow bed1.
Primary-button presses inside an eligible canvas trigger sound and animation;
popups, modals and unfocused windows cannot trigger clicks. UI2's theme accent
colors the bed's patterned cover. Reduced motion gives a neutral stationary pose
while keeping click audio. Hidden scenes disable cameras and pause their clock;
resuming never simulates the hidden interval. Animated visible views request
repaints at 30 Hz. UI2 owns image resize, texture registration and rounded clipping.

Audio opens one output stream lazily on the first click. Each scene owns a sink;
rapid clicks replace its previous sound. Closure stops playback. Device failure
logs once and leaves the visuals usable; it is retried on the next application
launch. Bed uses the bundled MP3; Ducky uses the same synthesized squeak as bed1.
No audio device is needed to create or restore scenes.

## Workbench adapter

Enable `workbench` to expose `BedPanel` and `DuckPanel`. Spawn either with
`InArea(area)`. Required components install `PanelInstance` and `MascotScene`
immediately. The descriptive keys are `bedtime.bed` and `bedtime.duck`, and the
labels are **Bed** and **Ducky**. Creation discovery and persistence remain
application-owned. Ordinary hidden-panel/layout/workspace drawing rules suspend
these viewports without a second activation state.

bEd2 exposes only Bed in View and saves both panels' placement in its existing
session and layout-default formats. Ducky is an easter egg opened through the
undocumented `+ducky` Terminal2 command: no arguments,
open/focus Ducky in the invoking workspace's selected layout, with creation in
the largest area. No implicit workspace activation is performed.

## Verification

```sh
cargo test --offline --locked -p bedtime-mascots2
cargo test --offline --locked -p bedtime-mascots2 --features workbench
cargo check --offline --locked -p bedtime-mascots2 --no-default-features --example standalone
cargo test --offline --locked -p bed2
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_mascots.py
cargo run --offline --locked -p bedtime-mascots2 --example standalone
```

The native smoke uses an isolated configuration directory and verifies rendered
output, interactions, themes, resize, hidden layouts/workspaces, cube coexistence,
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
