Vendored from the crates.io `freetype-sys` 0.23.0 release (repository commit
`9040df60c41b76aeaa19c231d488e79d91e666fe`), including original FreeType 2.13.2,
libpng sources, licenses, and Rust bindings.

Bed enables the existing libz-sys dependency's `static` feature, which builds
its bundled zlib. The FreeType/libpng build uses the actual target headers
exported as `DEP_Z_INCLUDE` instead of the release's missing relative
`libz-sys/src/zlib` directory. An explicit Rust import retains libz-sys's native
link metadata. These changes let the existing bundled build run without a
system FreeType, libpng, or zlib installation on all target platforms.

FreeType's FTL/GPL license alternatives are preserved in `freetype2/docs`;
Bed uses the FreeType License (FTL) option. libpng retains its original license.
No rasterizer or font parser implementation is changed.
The crate-scoped `unpredictable_function_pointer_comparisons` lint allowance
preserves upstream raw FFI structs' legacy Eq/Hash derives on newer compilers.
