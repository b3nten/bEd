# Shared viewer fixtures

These assets are used by the retained application/library tests, examples and
native smoke scripts. Tests do not require an upstream checkout or network.

- `gltf/cube.gltf` and `gltf/cube.glb` are original synthetic fixtures with cube
  geometry and an embedded checker texture. `gltf/generate.py` regenerates them.
- `gltf/LittlestTokyo.glb` exercises Draco decoding and static skinning. The
  unchanged upstream model is by glenatron under CC BY 4.0; its source and
  attribution are recorded in [gltf/licence.txt](gltf/licence.txt).
- `images/sample.png` is a small image-decoding and file-opening fixture.
- `fonts/DejaVuSans.ttf` exercises rejection of font bytes as images; its original
  license is retained in [fonts/DejaVuSans-LICENSE.txt](fonts/DejaVuSans-LICENSE.txt).

These are development assets. Packages embed their own runtime fonts, icons,
models and environment maps with notices beside their owning libraries.
