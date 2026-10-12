Local extensions to upstream dear-imgui-rs 0.18.0. Original MIT/Apache notices are retained.

Bed adds an optional per-viewport postprocess target/encode hook with a separate surface output format. Each renderer-owned viewport holds its own postprocessor; callbacks, context ownership, managed texture epochs, fault reporting, submission and presentation remain upstream.

Optional additional surface usage flags enable final GPU frame readback where supported. Defaults retain the original render-attachment usage. One Alpha8 conversion iterator was updated for current Rust Clippy without changing conversion bytes.

Bed adds `WgpuSnapshotRenderer` and its UI-thread owner, `WgpuSnapshotContext`, for rendering owned `FrameSnapshot` values in Bevy's render world. The GPU renderer is `Send + Sync` on native targets and retains no Dear ImGui context pointers or viewport postprocessor policy. It reuses the synchronous backend's pipelines, geometry validation, clipping, buffer uploads, samplers and managed-texture protocol. Existing `WgpuRenderer` constructors and embedding behavior remain supported.

Snapshot capture and completion polling remain on the UI thread. The detached renderer supports reset/sampler commands and external textures; snapshot capture rejects arbitrary raw callbacks. Frames that cannot be drawn still reconcile their texture requests. Explicit shutdown validates that all epochs completed before releasing GPU resources and resetting Context-owned texture bindings, allowing device recovery with the same Dear ImGui context. Snapshot tests exercise cross-thread rendering, clipped pixel output, external images, upload feedback, replacement and epoch ordering.
