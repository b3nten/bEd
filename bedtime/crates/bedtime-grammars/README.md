# Bundled Tree-sitter grammars

One native grammar crate for 30 code/document languages, plus the Markdown inline
helper. Generated parsers, external scanners, headers, upstream queries and license
notices are vendored here. No per-language Rust packages, downloads, Node, or parser
generation run during Cargo builds. Desktop native builds are the supported target.

The selected languages are Rust, C, C++, C#, Java, Go, Swift, Kotlin, Dart, Python,
Ruby, PHP, Lua, Bash, PowerShell, JavaScript, TypeScript, TSX, HTML, CSS, JSON, YAML,
TOML, XML, Markdown, SQL, Dockerfile, CMake, Make and HCL.

## Provenance and updates

[sources.json](sources.json) records the pinned Arborium 2.18.2 packages (and
`tree-sitter-md` 0.5.3 for Markdown inline), registry archive checksums, reported
upstream revisions, license sources and individual file hashes. Ruby and CMake's
packaged READMEs omit a commit; their registry archive checksums identify the exact
inputs. Each grammar retains `UPSTREAM.md` and its upstream `LICENSE`. Most use MIT;
HCL uses Apache-2.0. Upstream query comments/attributions are preserved.

Generated C parsers are gzip-compressed to keep the checked-in bundle about 13 MiB
instead of 230 MiB. The build script expands them into Cargo's `OUT_DIR`, then
compiles each grammar with its own header paths. The only source transformation
prefixes `tree_sitter_*` symbols with `bed_`, keeping the bundled symbols
private to this native grammar set. This reduces the Cargo dependency
graph, not the number of C parsers that need compiling.

Verify or reimport from an already populated registry:

```sh
python3 tools/import_grammars.py --check
python3 tools/import_grammars.py ~/.cargo/registry/src/<registry>
cargo test --offline --locked -p bedtime-treesitter
```

File hashes describe uncompressed contents, independent of gzip metadata. Grammar
updates are deliberate source changes: review provenance/licenses, update the
bundle and hashes, then verify highlight, fold, outline and injection fixtures.
The checked-in Rust bindings and build list change when selecting languages;
the backend's public language list and fixtures must agree. Runtime grammar
installation and WebAssembly loading are outside this implementation.
