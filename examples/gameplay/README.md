# Same game, editor and runtime

`game` contains a game plugin that depends only on Bevy. `editor` launches through
`bed_app::Application`, renders the game camera through a
workbench canvas and admits ordinary Bevy input only for its focused viewport.
`runtime` adds that same game plugin to ordinary Bevy and renders into the window;
it has no bEd crate dependencies.

```sh
cargo run -p bed-example-game-editor
cargo run -p bed-example-game-runtime
cargo run -p bed-example-game-editor -- --smoke-test
cargo run -p bed-example-game-runtime -- --smoke-test
cargo tree -p bed-example-game-runtime
```

Click the viewport, use WASD to move and mouse motion to rotate the blue player.
Pause changes the game's own execution resource. The adapter changes only the
camera target and input admission; closing/reopening or resizing the panel leaves
its scene and simulation intact. Use **Open game viewport** to reopen it.

The SDK establishes asset sources before Bevy's asset plugin and installs the
editor foundation before the application adds its game and viewport plugins.
Editor/runtime selection is construction-time; the example does not unload
plugins or implement a gameplay framework.
