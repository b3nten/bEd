# Regression fixtures

These committed baselines are consumed by ordinary Rust tests. They preserve
expected behavior while Bed evolves; builds and tests require no original C++
checkout or upstream regeneration tools. Source attribution remains in
[NOTICE](../../NOTICE) and the accompanying licenses.

## Highlighting

`highlight.json` retains source samples from ned commit
`2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff` for the original 17 languages.
Bed now uses Arborium's grammars and queries, so exact upstream colors and
capture ranges are no longer a baseline. Behavioral tests cover Markdown,
embedded languages, byte offsets, editing, cancellation and theme changes;
all 112 bundled grammars must compile their highlight and injection queries.

```sh
cargo test --locked -p bed-highlight --test highlight
```

## Editor painting and measurements

`views.json` records vertices, packed colors, triangle order and byte text
measurements from the same ned revision's `TextView`, `CaretView`, `GutterView`
and `EditorUtils`. The original ImGui version is recorded in the fixture.
Fourteen cases cover selections, carets, syntax and Git markers, clipping,
Unicode/malformed UTF-8/NUL, diagnostic ranges and rainbow phases.

Both the captured baseline and Rust comparison use the stb font loader and a
deliberately fractional `A` advance. Tests compare geometry within 0.0001px and
colors/index order exactly; font-atlas UV packing is excluded. Bed's compact
gutter comparison accounts for the 6px removed from the original trailing
padding. These are paint/measurement checks, not GPU, FreeType rasterization,
whole-frame layout or tooltip activation tests.

```sh
cargo test --locked -p bed-editor-ui --test views
```

## Terminals

Terminal protocol scenarios are authored directly in `crates/bed-terminal/tests`.
The presentation suite covers text, styles, colors, Unicode and cursor shapes;
PTY and input regressions exercise live shells, process cleanup and negotiated
keyboard/paste behavior. Imported terminal adapter fixtures are no longer used.

```sh
cargo test --locked -p bed-terminal
```

## Model viewers

`gltf/cube.glb` and `gltf/cube.gltf` are original synthetic fixtures containing
the same cube geometry and embedded checker texture in binary and JSON forms.
Their small generator remains available:

```sh
python3 tests/fixtures/gltf/generate.py
cargo test --locked -p bed-plugin-gltf
```

Draco tests also use `assets/LittlestTokyo.glb`, including its 71
compressed primitives, eight skinned meshes and four textures. Malformed-stream
tests isolate a small primitive from that asset. Attribution remains in
[assets/README.md](../../assets/README.md). Opt-in native
GPU tests and the application's `--plugin-smoke` check rendering and lifecycle
behavior separately from these loader fixtures.

`fonts/` retains Source Code Pro and DejaVu Sans only for font-viewer and SVG
regressions, together with their licenses. They are not runtime assets and are
not copied into desktop packages. Runtime fonts are exclusively Paper Mono.
