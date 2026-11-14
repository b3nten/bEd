# Notices for source adapted into bEd

These original notices accompany code now maintained in bEd. Release packages
include them verbatim in `NOTICE` through `tools/collect-licenses.py`.

- `bevy_stl`: MIT notice from 0.18.0, revision
  `70adf44a1594676b4d7b045bfafbed753752fcfd`. bEd's
  `bedtime/crates/bedtime-model/src/stl.rs` adapts its triangle expansion, uses
  published `stl_io` directly, and owns snapshot validation and mesh creation.
- `bevygraph`: declared `MIT OR Apache-2.0`, revision
  `0d520f5f4e8693166408f082e8d25dd377dabc3e`, with no license file upstream.
  `bedtime/crates/bedtime-schedules` adapts it: the schedule extractor (ported
  to Bevy 0.20, reading the access the schedule keeps instead of initialising
  systems again), the snapshot model, the layout and the viewer.

Neither crate is a dependency, their notices remain here with the adapted code.
