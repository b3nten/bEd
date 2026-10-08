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

`dock_builder_shim.cpp` includes a null-safe accessor for a native dock node's
ID. Bed uses it with the existing leaf/rectangle bindings to choose a panel's
destination in Rust, without duplicating the opaque ImGuiDockNode layout. The
accessor contains no application layout policy.

A second null-safe accessor exposes the node's existing tab bar. Bed queues
selection of a split's source document in Rust while the duplicate receives
keyboard focus. This avoids copying the opaque node layout or changing ImGui's
focus/docking algorithms.

Unused experimental node-info and central-node mutation accessors were removed
during repository cleanup. Only the ID and tab-bar Bed accessors remain.

The native docking tab-list popup uses `SetNextWindowViewport` in
`DockNodeWindowMenuUpdate` instead of setting an explicit popup position.
This keeps the popup in its originating native viewport after a group is
detached. ImGui still chooses its placement and handles the menu contents.

The original Rust binding `LICENSE-MIT` and `LICENSE-APACHE` files are copied
unchanged from repository commit `95f279150e95c728952db3e79226c687167ccf73`.
The published crate archive omitted these workspace-root notices; they are
retained here alongside the native sources' separate notices.

Native viewport window and title backgrounds preserve style alpha instead of
forcing it to 255. Bed requests transparent native windows and applies opacity
to backgrounds only; foreground colors are unchanged. Translucent viewport
backgrounds retain renderer clearing, preventing opacity from accumulating
across frames in persistent postprocess targets.

Checkboxes paint their square and check mark at 70% of the native frame size,
centered within the original bounds. Row layout, label baseline, keyboard
navigation, and the full click target are preserved globally.
