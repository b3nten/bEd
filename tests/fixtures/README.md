# Shared viewer fixtures

These assets are used by the retained application/library tests, examples and
native smoke scripts. Tests do not require an upstream checkout or network.

- `gltf/cube.gltf` and `gltf/cube.glb` are original synthetic fixtures with cube
  geometry and an embedded checker texture. `gltf/generate.py` regenerates them.
- `gltf/LittlestTokyo.glb` exercises Draco decoding and static skinning. The
  unchanged upstream model is by glenatron under CC BY 4.0; its source and
  attribution are recorded in [gltf/licence.txt](gltf/licence.txt).
- `images/sample.png` is a small image-decoding and file-opening fixture.
- `fonts/DejaVuSans.ttf` is the font viewer's fixture (and font bytes that
  aren't an image); its original license is retained in
  [fonts/DejaVuSans-LICENSE.txt](fonts/DejaVuSans-LICENSE.txt).
- `video/clip.mp4` is a two-second, silent 320 × 180 H.264 clip (a gradient
  and a moving square), made by `video/make_clip.swift` (`swift make_clip.swift
  clip.mp4`, macOS).

These are development assets. Packages embed their own runtime fonts, icons,
models and environment maps with notices beside their owning libraries.
