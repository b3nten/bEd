Local extensions to upstream dear-imgui-rs 0.18.0. Original MIT/Apache notices are retained.

Bed adds an optional per-viewport postprocess target/encode hook with a separate surface output format. Each renderer-owned viewport holds its own postprocessor; callbacks, context ownership, managed texture epochs, fault reporting, submission and presentation remain upstream.

Optional additional surface usage flags enable final GPU frame readback where supported. Defaults retain the original render-attachment usage. One Alpha8 conversion iterator was updated for current Rust Clippy without changing conversion bytes.
