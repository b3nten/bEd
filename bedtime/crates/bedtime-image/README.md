# bedtime-image

An image viewer for workbench apps: raster images, animations and SVG.

```rust,ignore
app.add_plugins(ImagePlugin);
```

- **Decoding** happens in the model (`Picture`), on the document's load worker:
  formats by their signature (TGA has none, so it's tried last), SVG rendered
  at a sharp size (2048 pixels on its long side), animated GIF, APNG and WebP
  frames with their delays. Up to 256 MiB of pixels; images longer than 8192
  pixels are scaled down for the GPU. SVGs never read other files.
- **Viewing.** The image fills its panel, over a checkerboard. Scroll or pinch
  zooms around the pointer, dragging pans, a double click switches between
  fitting and actual size (a point per pixel); Cmd/Ctrl+1 too, and Zoom In,
  Zoom Out and Reset Zoom (Cmd/Ctrl+=, - and 0: it handles workbench's
  actions, so they zoom the picture, not the panel).
  Zoomed in, pixels show sharp (unless Smooth When Zoomed In). Animations play;
  Space pauses.
- **Floating:** play (animations), zoom out, the zoom (click: actual size),
  zoom in, fit at the bottom; size and format at the bottom left.
- **Right-click:** fit, actual size, zoom, play, the background (checkerboard,
  dark, light, none), smoothing. The background and smoothing are saved.

```sh
cargo test -p bedtime-image
```
