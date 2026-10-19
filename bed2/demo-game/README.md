# Cube game

A normal Bevy scene, with a lit spinning cube and a custom fullscreen color-grade
shader. The game library depends on Bevy and wgpu, with no Bedtime dependencies.

```sh
cargo run -p bed2-demo-game --example standalone
```

`CubeGamePlugin` installs rendering and simulation once. `spawn_scene(world)`
creates a scene explicitly and returns its root; `setup` creates a single scene
for standalone examples. `GameScene` exposes the camera, cube and running flag.

The full-window embedded example uses the same child renderer and UI2, without
any Workbench dependency or panel system:

```sh
cargo run -p bed2-demo-game --example embedded
```

UI2 is only a dev dependency for that example; the game library continues to
depend solely on Bevy and wgpu.

The standalone example's native smoke path checks rendered pixels and rotation:

```sh
cargo run -p bed2-demo-game --example standalone -- --smoke-dir target/ui2-standalone-smoke
cargo run -p bed2-demo-game --example embedded -- --smoke-dir target/ui2-embedded-smoke
```
