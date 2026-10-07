`highlight.json` records the exact per-line byte spans and theme slots produced
by ned at `UPSTREAM_REVISION`, using its pinned native Tree-sitter runtime,
grammars, query files, and unchanged `TreeSitter::highlightSnippet` algorithm.
It covers all 17 bundled languages, Unicode text, interpolation, comments,
functions, builtins, and syntax roles.

Regenerate with `bash scripts/provenance/update-highlight-fixtures.sh`. The generator
`upstream_highlight.cpp` supplies input samples and serializes upstream output;
it does not implement or substitute any parsing or highlighting behavior.
The upstream implementation and bundled queries are attributed under the
repository's `LICENSE` and `NOTICE`. The pinned reference checkout retains the
original native grammar and runtime notices. Distributed Bed dependency notices
are collected from the selected Cargo sources under `LICENSES/dependencies/`.

`views.json` records draw vertices, packed RGBA colors, triangle order, and
bounded-byte text measurements from the unchanged pinned `TextView`,
`CaretView`, `GutterView`, and `EditorUtils` implementations. Fourteen cases cover selections,
multiple carets, fractional text advances, indentation guides, syntax runs,
horizontal and vertical clipping, Unicode, malformed UTF-8, embedded NUL,
blocked input, two rainbow phases, UTF-16 diagnostic ranges across emoji and
tabs, all four diagnostic severities, degenerate/reversed/multiline diagnostic
ranges, clipped squiggles, Git edited-line colors, and a scrolled four-digit
gutter. Diagnostic widths include an attached but empty store; fractional top
margins exercise the gutter's coordinate convention.

Regenerate with `bash scripts/provenance/update-view-fixtures.sh`; compare with Rust using
`cargo test --offline -p bed-ui --test views`. The generator links the original document,
view, highlight, diagnostics, tooltip, Git, settings/keybinds, and ImGui source
implementations. Git markers use a real temporary libgit2 repository and HEAD
blob. The full Settings constructor runs with HOME unset and loads the bundled
profile without writing ned configuration. Its host clock adapts platform
infrastructure only; no editor
or parsing behavior is replaced. The generator verifies the recorded ned and
ImGui commits. Both runners use the built-in stb font loader and the same
deliberately fractional `A` advance, then compare positions, colors, and index
order exactly (positions allow 0.0001px floating-point tolerance). The original
ImGui version is recorded in the fixture; Bed uses the compatible binding
runtime. Font atlas UV packing is excluded from the comparison.

These are paint-leaf and measurement comparisons, not full application
screenshots. They do not validate GPU shader output, production FreeType
rasterization, the whole Frame layout, or tooltip activation.
Native application capture and platform acceptance checks remain separate.

Bed's requested compact gutter reserves the document's actual digit count.
The draw-leaf comparison supplies the unchanged original's recorded gutter
width to isolate marker positions, text alignment, colors, and clipping;
the live Frame regression verifies the compact panel's leading space.

Regeneration is optional provenance tooling, separate from Bed's Rust builds
and ordinary tests, which consume these committed fixtures. It requires the
ignored pinned checkout in `reference/ned`; setup and tool requirements are
documented in `scripts/provenance/README.md`. Original upstream names and
recorded identifiers remain unchanged so comparisons retain their source history.
