# Notices for source adapted into Bed

These original notices accompany code now maintained in Bed. Release packages
include them verbatim in `NOTICE` through `scripts/lib/collect-licenses.py`.

- `dear-imgui-winit`: MIT/Apache-2.0 notices from dear-imgui-rs revision
  `95f279150e95c728952db3e79226c687167ccf73`, the recorded source of 0.18.0.
  Bed's `bedtime-desktop/src/input_map.rs` adapts its keyboard and mouse mapping.
- `bevy_stl`: MIT notice from 0.18.0, revision
  `70adf44a1594676b4d7b045bfafbed753752fcfd`. Bed's
  `bedtime-plugin-model-viewer/src/stl.rs` adapts its triangle expansion, uses
  published `stl_io` directly, and owns snapshot validation and mesh creation.

Neither crate is an active local dependency. Their former vendor trees were
removed; attribution and permission notices remain here with the adapted code.
