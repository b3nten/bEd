# Notices for source adapted into bEd

These original notices accompany code now maintained in bEd. Release packages
include them verbatim in `NOTICE` through `tools/collect-licenses.py`.

- `bevy_stl`: MIT notice from 0.18.0, revision
  `70adf44a1594676b4d7b045bfafbed753752fcfd`. bEd's
  `bedtime/crates/bedtime-model/src/stl.rs` adapts its triangle expansion, uses
  published `stl_io` directly, and owns snapshot validation and mesh creation.

The crate isn't a dependency; its notice remains here with the adapted code.
