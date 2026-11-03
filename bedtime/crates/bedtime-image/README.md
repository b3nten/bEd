# bedtime-image

Workbench viewer for raster images, animations and SVG.

```rust,ignore
app.add_plugins(ImagePlugin);
```

- **Decoding:** `Picture` detects formats by signature on the load worker
  (TGA is tried last), rasterizes SVG at 2048 pixels on its longest side, and
  decodes animated GIF, APNG and WebP frames with their delays. Up to 256 MiB of pixels; images longer than 8192
  pixels are scaled down for the GPU. SVGs never read other files.
- **Viewing.** The image fills its panel, over a checkerboard. Scroll or pinch
  zooms around the pointer, dragging pans, a double click switches between
  fitting and actual size (a point per pixel); Cmd/Ctrl+1 too, and Zoom In,
  Zoom Out and Reset Zoom (Cmd/Ctrl+=, - and 0: it handles workbench's
  actions, so they zoom the picture, not the panel).
  Magnification uses nearest-neighbor filtering unless Smooth When Zoomed In
  is enabled. Animations autoplay; Space pauses.
- **Floating:** play (animations), zoom out, the zoom (click: actual size),
  zoom in, fit at the bottom; size and format at the bottom left.
- **Right-click:** fit, actual size, zoom, play, the background (checkerboard,
  dark, light, none), smoothing. The background and smoothing are saved.

```sh
cargo test -p bedtime-image
```
