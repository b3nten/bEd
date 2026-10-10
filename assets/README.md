# Source assets

These files support design, packaging and tests. The Bed mascot embeds its GLB
and `bed.mp3` click sound in the executable; other source assets are not copied
into the installed application's runtime resources. Blender source files remain
development assets.

`bEd.icon` is the editable Icon Composer project, including the SVG Repo artwork
in `bEd.icon/Assets/bed-svgrepo-com.svg`. `bEd-iOS-Default-1024@1x.png` is its
1024-pixel export and the shared input for platform icon generation. Update that
export after editing the project. The packaging scripts generate macOS ICNS and
Linux launcher PNGs in the build output; generated icons are not checked in.

## Sample models

`bed_low_poly.glb` is **Bed Low Poly** by
[mezuna](https://sketchfab.com/mezuna), licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
[Original model on Sketchfab](https://sketchfab.com/3d-models/bed-low-poly-d58eea1a6ee746d29fd18f2d9e1a559d).
The file is embedded unchanged in the Bed mascot panel. At runtime the model is
centered and scaled, follows the pointer, and briefly squashes when clicked.
The original attribution is also present in the GLB's asset metadata and in
[NOTICE](../NOTICE), in the Bed Low Poly section, for release packages.

`LittlestTokyo.glb` is **Littlest Tokyo** by
[glenatron](https://sketchfab.com/glenatron), licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
[Original model on Sketchfab](https://sketchfab.com/models/94b24a60dc1b48248de50bf087c0f042).
The attribution above is recorded in the GLB's asset metadata. The supplied
model is used unchanged to exercise Draco decoding and static skin posing.

## Model development

`house.glb` and `house2.glb`, with their corresponding `.blend` sources, are
available for scene composition, extracting objects and editing geometry. The
Ducky and bEdtime scenes generate their geometry in code.

The source record supplied for `house.glb` is retained in `house.licence`:
[Bedroom on CGTrader](https://www.cgtrader.com/free-3d-models/architectural/other/bedroom-04e35825-4a7a-4a50-b368-f383b160c2eb).
That file records a source URL rather than licence text.
