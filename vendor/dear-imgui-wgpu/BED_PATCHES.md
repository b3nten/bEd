Local extensions to upstream dear-imgui-rs 0.18.0. Original MIT/Apache notices are retained.

The former per-viewport postprocess factory, separate output format and
additional surface usage flags were removed. Bed uses Bevy-owned composition,
effects and GPU capture. The native multi-viewport implementation is restored
to published 0.18.0 behavior. One Alpha8 conversion iterator remains updated for
current Rust Clippy without changing conversion bytes.

Bed adds `WgpuSnapshotRenderer` and its UI-thread owner, `WgpuSnapshotContext`, for rendering owned `FrameSnapshot` values in Bevy's render world. The GPU renderer is `Send + Sync` on native targets and retains no Dear ImGui context pointers. It reuses the synchronous backend's pipelines, geometry validation, clipping, buffer uploads, samplers and managed-texture protocol. Existing `WgpuRenderer` constructors and embedding behavior remain supported.

Snapshot capture and completion polling remain on the UI thread. The detached renderer supports reset/sampler commands and external textures; snapshot capture rejects arbitrary raw callbacks. Frames that cannot be drawn still reconcile their texture requests. Explicit shutdown validates that all epochs completed before releasing GPU resources and resetting Context-owned texture bindings, allowing device recovery with the same Dear ImGui context. Snapshot tests exercise cross-thread rendering, clipped pixel output, external images, upload feedback, replacement and epoch ordering.
