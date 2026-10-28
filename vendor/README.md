# Dependency patches

Prefer published Cargo packages pinned in `Cargo.lock`. Keep a local dependency
only when bEd needs an unavailable upstream fix. Each fork retains its original
licenses and a `BED_PATCHES.md`; the boundary checker enforces that record.

| Fork | Version | Active changes |
| --- | --- | --- |
| `bevy_render` | 0.20.0 | Window screenshot readback without a swapchain drawable |
| `bevy_winit` | 0.20.0 | Relaunch an App in the same window (one App per workspace) |
| `egui` | 0.36.2 | See its patch record for the active presentation/input fixes |
| `arboard` | 3.6.1 | Lexical file paths, Linux URI lists and macOS PNG paste |
| `rio-vt` | 0.5.28 | Bounded graphics/protocol buffers, accounting and reset fixes |

Keep each fork's source, tests, notices and patch record together for upgrades.
Nested caches and dependency CI workflows are omitted. Root Cargo manifests,
`Cargo.lock` and the repository CI workflow control builds and validation.

The former Dear ImGui and bundled FreeType forks were removed with the old UI.
Retained source adaptations are documented under [licenses](../licenses/README.md).
