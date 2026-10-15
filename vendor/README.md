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
| `dear-imgui-sys` | 0.18.0 | Bundled FreeType/SVG fonts, font-loader access, dock selection/background operations and smaller checkbox painting |
| `freetype-sys` | 0.23.0 | Static bundled zlib linkage and target-correct headers for the FreeType/libpng build |
| `dear-imgui-wgpu` | 0.18.0 | GPU-only snapshot rendering with UI-thread texture completion and device recovery |
| `arboard` | 3.6.1 | Lexical file clipboard paths, Linux URI-list compatibility and macOS PNG image paste |
| `rio-vt` | 0.5.28 | Bounded terminal graphics/protocol buffers, budget accounting and terminal reset fixes |

Read each fork's `BED_PATCHES.md` for details and source revisions. The native
FreeType, libpng, PlutoSVG and PlutoVG sources below these forks support the same
bundled font backend; their original notices remain alongside them. These are
build inputs, not a requirement to install those libraries on a remote SSH host.

The five forks in this table are active build dependencies. Keep their source, tests and patch
records together so upgrades can be compared with the published versions.
Nested build caches, package-cache markers, runtime `imgui.ini` files, redundant
dependency lockfiles and dependency CI workflows are omitted. Bed's root
`Cargo.lock` and `.github/workflows/ci.yml` control builds and validation.

The original MIT/Apache notices for the Dear ImGui Rust binding crates
are retained at each crate root. They come from upstream revision
`95f279150e95c728952db3e79226c687167ccf73`, matching their recorded package
provenance, and are included in release packages by the shared notice collector.

The former `dear-imgui-winit` and `bevy_stl` vendor trees were removed.
`bedtime-desktop` owns its single-window platform adapter with Bevy-owned native
handles, and the model viewer loads bounded STL snapshots using published
`stl_io` directly. Original notices for adapted input mapping and triangle
expansion are retained under `licenses/` and included in release packages.

Original source credits are retained in `NOTICE` and fixture documentation.
The original C++ checkout and port-verification scripts are not part of the
development workflow. Active dependency versions come from `Cargo.lock`.
