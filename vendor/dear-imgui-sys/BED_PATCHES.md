Vendored from the crates.io `dear-imgui-sys` 0.18.0 release, whose package records
repository commit `95f279150e95c728952db3e79226c687167ccf73` and cimgui/ImGui
revisions in `Cargo.toml`. Original sources, notices, and embedded native
licenses are preserved.

Bed adds the `bundled-freetype` feature. It uses exact `freetype-sys` 0.23.0 as a
normal dependency with its bundled feature, supplies matching FreeType 2.13.2
headers to the existing ImGui FreeType backend, and skips pkg-config/vcpkg
lookup in this feature branch. Other native build paths remain original.
The native FreeType bindings are re-exported to retain Cargo's archive linkage
and support owned, validated memory font descriptors in Bed.
One thin C accessor exposes the native static FreeType loader callback table so
Bed selects the matching atlas-wide backend before adding validated fonts.
FreeType has atlas-wide state and cannot be a per-font override. Appending to
an existing host atlas requires its global loader to be FreeType; a different
loader returns the binding's error without changing the host's fonts.

The same feature compiles unchanged PlutoSVG 0.0.8 sources from official commit
`fd8a080b3d0bccc41f1ae708d3785e7f08398a58`, together with its exact PlutoVG
1.3.3 submodule `bbd91f0d06a71491691b36330f29dffa4af87ccf`. Source build lists
mirror the static CMake targets. Original MIT notices, embedded stb licenses,
and the FreeType-derived rasterizer's FTL.TXT are retained. ImGui's original
`IMGUI_ENABLE_FREETYPE_PLUTOSVG` integration installs their FreeType SVG hooks;
no renderer algorithm or font asset is changed. This is required by the
licensed pinned NotoColorEmoji/Emoji fonts, which contain SVG glyphs.
Sources: https://github.com/sammycage/plutosvg/tree/fd8a080b3d0bccc41f1ae708d3785e7f08398a58
and https://github.com/sammycage/plutovg/tree/bbd91f0d06a71491691b36330f29dffa4af87ccf.

`third-party/freetype2/include` is copied unchanged from `freetype-sys` 0.23.0;
its crate records commit `9040df60c41b76aeaa19c231d488e79d91e666fe`. Matching
FreeType license texts are included under `third-party/freetype2`. Bed uses
the FreeType License option, rather than the alternative GPL license.

`dock_builder_shim.cpp` adds two operations used by Bed's tiling UI:
`bed_imgui_dock_node_select_window` makes the selected document the visible
member of a native `NoTabBar` dock area, and
`bed_imgui_dock_node_clear_background` clears the cached native dock background
after submission so Bed owns background composition. No opaque native struct
layout is copied into Rust.

Unused node-ID/split accessors, native dock-menu/grip/close-button changes, and
native viewport alpha/clearing patches were removed. Bed owns its tabs and
menus, disables native docking/tab bars for its areas, and renders into a
Bevy-owned window with application-owned composition and clearing.

The original Rust binding `LICENSE-MIT` and `LICENSE-APACHE` files are copied
unchanged from repository commit `95f279150e95c728952db3e79226c687167ccf73`.
The published crate archive omitted these workspace-root notices; they are
retained here alongside the native sources' separate notices.

Checkboxes paint their square and check mark at 70% of the native frame size,
centered within the original bounds. Row layout, label baseline, keyboard
navigation, and the full click target are preserved globally.
