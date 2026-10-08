# Source assets

These files support design, packaging and tests. They are not copied into the
installed application's runtime resources.

`bEd.icon` is the editable Icon Composer project, including the SVG Repo artwork
in `bEd.icon/Assets/bed-svgrepo-com.svg`. `bEd-iOS-Default-1024@1x.png` is its
1024-pixel export and the shared input for platform icon generation. Update that
export after editing the project. The packaging scripts generate macOS ICNS and
Linux launcher PNGs in the build output; generated icons are not checked in.

## Sample models

`LittlestTokyo.glb` is **Littlest Tokyo** by
[glenatron](https://sketchfab.com/glenatron), licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
[Original model on Sketchfab](https://sketchfab.com/models/94b24a60dc1b48248de50bf087c0f042).
The attribution above is recorded in the GLB's asset metadata. The supplied
model is used unchanged to exercise Draco decoding and static skin posing.
