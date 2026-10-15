# Cube game

A normal Bevy scene, with a lit spinning cube and a custom fullscreen color-grade
shader. This crate depends on Bevy and wgpu, with no Bedtime dependencies.

```sh
cargo run -p bed2-demo-game --example standalone
```

`bed2` installs the same `CubeGamePlugin`. Its adapter routes the `GameCamera` to
an image for the editor viewport or directly to the window beneath UI. Scene
rendering and its custom post-processing remain in Bevy's normal `Core3d` schedule.

The standalone example's native smoke path checks rendered pixels and rotation:

```sh
cargo run -p bed2-demo-game --example standalone -- --smoke-dir target/ui2-standalone-smoke
```
