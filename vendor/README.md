# Bed dependency patches

Prefer published Cargo packages with versions recorded in `Cargo.lock`. Keep a
local dependency here only when Bed needs a patch that the published package
does not provide. Each fork must retain its original licenses and a
`BED_PATCHES.md` explaining the active changes. The crate-boundary check enforces
this for local Cargo dependencies.

The remaining forks are Bed infrastructure, independent of the historical ned
source revision:

| Fork | Version | Why Bed needs it |
| --- | --- | --- |
| `dear-imgui-sys` | 0.18.0 | Bundled FreeType feature and font-loader access, SVG emoji hooks, and small docking accessors |
| `freetype-sys` | 0.23.0 | Static bundled zlib linkage and target-correct headers for the FreeType/libpng build |
| `dear-imgui-winit` | 0.18.0 | Owned viewport snapshots for event routing and a native teardown fix |
| `dear-imgui-wgpu` | 0.18.0 | Per-viewport postprocessing and GPU capture surface usage |
| `bevy_stl` | 0.18.0 | Bevy 0.19.1 compatibility and a shared byte-loading entry point for bounded document snapshots |

Read each fork's `BED_PATCHES.md` for details and source revisions. The native
FreeType, libpng, PlutoSVG and PlutoVG sources below these forks support the same
bundled font backend; their original notices remain alongside them. These are
build inputs, not a requirement to install those libraries on a remote SSH host.

All five forks are active build dependencies. Keep their source, tests and patch
records together so upgrades can be compared with the published versions.
Nested build caches, package-cache markers, runtime `imgui.ini` files, redundant
dependency lockfiles and dependency CI workflows are omitted. Bed's root
`Cargo.lock` and `.github/workflows/ci.yml` control builds and validation.

The original MIT/Apache notices for the three Dear ImGui Rust binding crates
are retained at each crate root. They come from upstream revision
`95f279150e95c728952db3e79226c687167ccf73`, matching their recorded package
provenance, and are included in release packages by the shared notice collector.

Original source credits are retained in `NOTICE` and fixture documentation.
The original C++ checkout and port-verification scripts are not part of the
development workflow. Active dependency versions come from `Cargo.lock`.
