# Upstream verification tools

These optional tools compile unchanged upstream implementations to verify Bed's
translations and regenerate the committed comparison fixtures. They are not Bed
build dependencies and are not included in desktop or remote helper packages.
Normal Rust builds and tests use the committed fixtures without this tooling.

The tools require an ignored checkout at `reference/ned`, pinned to the commit
in `UPSTREAM_REVISION`, with the source submodules recorded in
`UPSTREAM_SUBMODULES`. They verify the relevant revisions before running. Keep
the original source and its licenses unchanged; recorded upstream names and
fixture identifiers intentionally retain their history.

- `verify-upstream-model.sh` runs the original document and editing tests. Its
  optional UTF-8, command, and highlighting checks use the genuine upstream
  ImGui and GLFW headers.
- `update-highlight-fixtures.sh` regenerates the exact highlighting captures.
- `update-view-fixtures.sh` regenerates native view geometry and measurements.
- `update-terminal-fixtures.sh` regenerates the original terminal state cases.

Run each script from any working directory; it locates the Bed repository from
its own path. Regenerating fixtures writes to `tests/fixtures/` and uses build
artifacts under `target/upstream-model` or `target/upstream-terminal`.

Regeneration requires a C++20 compiler and the original native source headers.
View generation also uses the local Cargo-built libgit2/zlib archives; terminal
generation requires the genuine pinned Fontconfig headers. The exact setup,
source hashes, and comparison limits are documented in
`tests/fixtures/README.md` and `tests/fixtures/terminal_core/README.md`, with
additional requirements in each script. These tools do not download or modify
the original implementations.
