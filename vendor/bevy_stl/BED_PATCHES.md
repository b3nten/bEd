# Bed patches to bevy_stl

Based on the released `bevy_stl` 0.18.0 source at upstream revision
`70adf44a1594676b4d7b045bfafbed753752fcfd` (`v0.18.0`):
<https://github.com/nilclass/bevy_stl/tree/70adf44a1594676b4d7b045bfafbed753752fcfd>.
The original MIT license is retained in `LICENSE`.

- Pin the runtime and example Bevy dependency to 0.19.1, matching Bed's shared
  renderer and avoiding an additional, incompatible Bevy/wgpu dependency tree.
  The original loader and mesh API need no source changes for this release.
- Expose `StlLoader::load_bytes` using the original `stl_io` parser and triangle
  mesh conversion. Bed loads local and SSH document snapshots in a background
  worker, so requiring a Bevy asset path would discard that integration.
- Prefer the exact binary facet-count/file-length envelope when a binary STL's
  arbitrary header starts with `solid `. Mask that unused header before calling
  `stl_io`, whose first-line ASCII probe otherwise misclassifies such files.
  Both loader entry points use this shared parser wrapper.
- Derive `Default` for the unit `StlPlugin`.

Bed disables the optional wireframe feature. The original asset-server loader
and optional labeled wireframe asset retain their upstream behavior. Bed applies
resource limits before calling the in-memory loader and validates its mesh data
before passing it to the renderer.
