Local extensions to upstream dear-imgui-rs 0.18.0. Original MIT/Apache notices are retained.

Bed adds owned native viewport window snapshots for event routing and redraw/close scheduling. Existing context ownership, callback safety and native platform teardown remain upstream.

The direct `WinitPlatform::shutdown` path now drops its temporary runtime RefCell borrow before runtime teardown clears the same slot. The native macOS lifecycle exposed this shutdown panic; a platform-driven teardown regression covers it.

Current Clippy cleanups remove redundant macOS f64 casts and combine a touch-input condition without changing its branch order or behavior.
