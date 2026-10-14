# bedtime-plugin-automation

A shared Rust API for inspecting and operating a Bedtime workbench. The owned
`bedtime` launcher installs it after the workbench; minimal hosts can install
`AutomationPlugin` themselves. The plugin is idempotent for existing hosts.
The launcher also installs the local transport, disabled by default. No extra
application runner is added. See the [external control and Python client guide](../../../tools/automation/README.md).

Submit owned `Operation` values through `AutomationRequests::submit`, then consume receipts through `AutomationResults::take`. Completion follows the actual workbench receipt, input sequence, or GPU readback. Application exit completes outstanding operations with errors.

```rust,ignore
use bedtime_plugin_automation::{AutomationRequests, Operation};

let open = world.resource_mut::<AutomationRequests>().submit(
    Operation::OpenPanel {
        panel_type: "example.panel".into(),
        state: serde_json::Value::Null,
        area: None,
    },
);
```

`Inspect` returns owned panel, command, layout, and UI-frame observations. Availability and readiness use the same facts as normal menus and panels. Handles include application and document-session identity and the entity generation, so closed panels and replaced sessions cannot target later content.

The `documents` feature adds scoped document observations to snapshots, `Operation::ReadDocument`, and the read-only `read_document` function. Document reads require the shared document service; inspection and generic panel operations work without it. The plugin owns no file, save, settings, or workspace transitions. Applications submit those to their normal transition owner. bEd's adapter lives in `src/automation.rs` and preserves captured application receipts.

`panel_handle` captures a live panel identity for application-owned operations; `resolve_panel` validates it before use.

`Input` accepts explicit `DesktopInputStep` values, one step per UI frame. Pointer, key, modifier, text, and scroll events pass through native interception, ImGui capture, and gameplay routing. Key events do not insert text; provide one `Text` event. Held input is released before its receipt.

`Capture` batches window and visible panel-content targets from the same composited frame. It can depend on a completed operation, including after its result was consumed. It waits for a subsequent UI frame after that dependency completes. Failed dependencies, hidden or closed targets, deadlines, and readback failures produce errors. Device replacement retries on the new generation and rejects obsolete callbacks.

Captured pixels are owned RGBA images with PNG and PPM export. Window captures cover client rendering; native titlebar controls and the OS cursor are outside that image. Hidden panels are not rendered specially for capture. Input and capture require the owned desktop launcher; generic inspection and panel commands also work in headless tests.

Applications register `ExtensionDefinition` values with `register_extension`. A
submit adapter validates its arguments and submits to the existing transition
owner. Return `ExtensionStart::Complete` for immediate observations or
`ExtensionStart::pending` with an owned receipt poller. Polling occurs only at the
shared completion point on the app thread; extensions share ordinary operation
IDs, completion-frame stamps, errors and capture dependencies. `Describe` exposes
registered descriptions and JSON schemas; `Extension` invokes a named adapter.

`AutomationResults::release` retires a completed receipt explicitly. External
connections own their receipts; the transport defers release while an accepted
capture depends on one. Document/panel wire handles preserve scope checks across
process restarts. `PanelInfo` additionally reports its bound document handle.
