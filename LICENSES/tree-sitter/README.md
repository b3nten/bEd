# Supplemental Tree-sitter grammar notices

These files retain the original license and copyright texts omitted from the
published Cargo archives used by Bed. Each file is copied unchanged from the
provider's official repository at the exact Git revision recorded in that
crate's `.cargo_vcs_info.json`, rather than from a moving branch or an unrelated
upstream grammar. Kotlin-codanna and TOML-ng use their own released repositories.

`sources.json` records the crate name, published version, source repository,
exact revision, original license URL, local filename, and SHA-256 of the retained
text. `scripts/collect-licenses.py` verifies the version, revision, and notice
checksum before copying a missing notice into
`LICENSES/dependencies/cargo/<crate>-<version>/ORIGINAL-LICENSE.txt` in packages.
The dependency index records the original notice URL and revision. Cargo caches
remain unchanged and collection does not download any license text.

The collector preserves license files already shipped in Cargo archives. The
Tree-sitter runtime, language wrapper, and HCL grammar include their own notices
and need no supplement. An active Tree-sitter package with neither an archive
notice nor an exact supplemental record fails collection, so dependency updates
must retain the matching original notice before packaging.

| Cargo package | Version | Published source revision | Original license |
| --- | --- | --- | --- |
| `tree-sitter-bash` | `0.23.3` | `487734f87fd87118028a65a4599352fa99c9cde8` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-bash/487734f87fd87118028a65a4599352fa99c9cde8/LICENSE) |
| `tree-sitter-c` | `0.23.4` | `3efee11f784605d44623d7dadd6cd12a0f73ea92` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-c/3efee11f784605d44623d7dadd6cd12a0f73ea92/LICENSE) |
| `tree-sitter-c-sharp` | `0.23.1` | `362a8a41b265056592a0c3771664a21d23a71392` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-c-sharp/362a8a41b265056592a0c3771664a21d23a71392/LICENSE) |
| `tree-sitter-cpp` | `0.23.4` | `f41e1a044c8a84ea9fa8577fdd2eab92ec96de02` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-cpp/f41e1a044c8a84ea9fa8577fdd2eab92ec96de02/LICENSE) |
| `tree-sitter-css` | `0.23.2` | `c0d581e32d183a536731ed6c3a72758b27e20411` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-css/c0d581e32d183a536731ed6c3a72758b27e20411/LICENSE) |
| `tree-sitter-go` | `0.23.4` | `3c3775faa968158a8b4ac190a7fda867fd5fb748` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-go/3c3775faa968158a8b4ac190a7fda867fd5fb748/LICENSE) |
| `tree-sitter-html` | `0.23.2` | `5a5ca8551a179998360b4a4ca2c0f366a35acc03` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-html/5a5ca8551a179998360b4a4ca2c0f366a35acc03/LICENSE) |
| `tree-sitter-java` | `0.23.5` | `94703d5a6bed02b98e438d7cad1136c01a60ba2c` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-java/94703d5a6bed02b98e438d7cad1136c01a60ba2c/LICENSE) |
| `tree-sitter-javascript` | `0.23.1` | `3a837b6f3658ca3618f2022f8707e29739c91364` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-javascript/3a837b6f3658ca3618f2022f8707e29739c91364/LICENSE) |
| `tree-sitter-json` | `0.24.8` | `ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-json/ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24/LICENSE) |
| `tree-sitter-kotlin-codanna` | `0.3.9` | `7f675c3e2c60cab4c63eb623b9e669fa27df0419` | [LICENSE](https://raw.githubusercontent.com/bartolli/tree-sitter-kotlin/7f675c3e2c60cab4c63eb623b9e669fa27df0419/LICENSE) |
| `tree-sitter-python` | `0.23.6` | `bffb65a8cfe4e46290331dfef0dbf0ef3679de11` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-python/bffb65a8cfe4e46290331dfef0dbf0ef3679de11/LICENSE) |
| `tree-sitter-ruby` | `0.23.1` | `71bd32fb7607035768799732addba884a37a6210` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-ruby/71bd32fb7607035768799732addba884a37a6210/LICENSE) |
| `tree-sitter-rust` | `0.24.0` | `18b0515fca567f5a10aee9978c6d2640e878671a` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-rust/18b0515fca567f5a10aee9978c6d2640e878671a/LICENSE) |
| `tree-sitter-toml-ng` | `0.7.0` | `64b56832c2cffe41758f28e05c756a3a98d16f41` | [LICENSE](https://raw.githubusercontent.com/tree-sitter-grammars/tree-sitter-toml/64b56832c2cffe41758f28e05c756a3a98d16f41/LICENSE) |
| `tree-sitter-typescript` | `0.23.2` | `f975a621f4e7f532fe322e13c4f79495e0a7b2e7` | [LICENSE](https://raw.githubusercontent.com/tree-sitter/tree-sitter-typescript/f975a621f4e7f532fe322e13c4f79495e0a7b2e7/LICENSE) |
