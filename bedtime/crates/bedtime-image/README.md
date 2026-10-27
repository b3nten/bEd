# bedtime-image

Embeddable static raster/SVG viewing through UI. The standalone host owns file identity,
source loading, menus and persistence. The canvas installs no filesystem reads, workers, windows or application viewer
associations. The optional Workbench document adapter supplies managed preparation.

Call `ImageContent::prepare(bytes, path)` and `content.render(physical_size)` on a
host-owned worker. Share immutable content through `Arc`; feed the resulting
`DecodedImage` and `content.dimensions()` into `ImageView::set_image`. Keep one
`ImageView` per canvas and call `view.draw(ui, view_id)`. Its returned response
accepts ordinary host context menus. `requested_size()` reports the next desired
physical raster size, allowing SVG rerasterization after zoom or display changes.
Raster rendering reuses its original pixels. Texture upload occurs during drawing;
texture handles are owned by the view and retire when it is dropped.

The canvas starts centered and contained, enlarging small images if needed.
Wheel/pinch zoom around the pointer; primary-button dragging pans; double-click
resets containment. Zoom is bounded to 0.01–100 times containment. Resizing retains
the normalized image coordinate at viewport center and the relative zoom. Pan is
unrestricted. The canvas clips to its bounds, preserves alpha and has no toolbar,
scrollbars or background artwork. Input respects popup/modal/window eligibility.

Supported extensions are PNG/APNG, JPEG/JFIF, SVG, GIF, WebP, BMP, ICO, TIFF,
TGA, PNM variants, QOI, DDS, HDR and EXR. Animated formats show their first frame.
Raster signatures take precedence over extension hints, with a hint for TGA.
Raster decoded allocation and SVG output are bounded to 64 MiB RGBA. SVG output
resolution reduces proportionally to that budget; high zoom can therefore reach
the rasterization limit. SVG text uses a lazily loaded system font database on the
worker. Embedded images obey dimension limits; external image references are
ignored. The host must bound encoded source reads (bEd uses 256 MiB).

Enable `workbench` for the minimal `workbench::ImagePanel`, or compose
`document::ImagePreview` in an existing document panel. The latter owns managed
preparation from canonical text/binary bytes and revision checks. bEd uses it
and supplies viewer associations, tab placement, context menus and persistence.

```sh
cargo test --offline --locked -p bedtime-image
cargo test --offline --locked -p bedtime-image --features workbench
cargo check --offline --locked -p bedtime-image --no-default-features
cargo test --offline --locked -p bed --features automation
cargo build --offline --locked -p bed --features automation
python3 -B bedtime/tools/smoke_images.py
```

Decoder behavior and fixtures are ported from the repository's v1 image plugin;
there is no dependency on its plugin API or frontend.

With `workbench`, `document::ImagePreview` supplies managed preparation from a
canonical TextDocument or BinaryDocument. SVGs observe current text, raster images
read a bounded binary snapshot, and obsolete jobs cannot replace newer revisions.
The host supplies its context menu and placement.
