# bed-plugin-automation

An optional Rust API for systems that operate a Bed app. Install `AutomationPlugin` after `WorkbenchPlugin` or `AppRuntimePlugin`; it adds no runner or network transport. The Bed executable enables it with `--features automation`.

Submit owned `Operation` values through `AutomationRequests::submit`. Poll `AutomationResults::status` and consume the owned result with `take`. Submission immediately returns a pending operation ID. Completion follows the actual workbench/application receipt, input sequence or GPU readback; a submitted save is not a completed save. Application exit completes outstanding operations with errors.

For example, submit an open request from an application system or a test:

```rust
use bed_plugin_automation::{AutomationRequests, AppOperation, Operation};

let open = world.resource_mut::<AutomationRequests>().submit(
    Operation::App(AppOperation::OpenFile {
        path: "src/main.rs".into(),
        viewer: None,
        additional: false,
    }),
);
```

Let the app advance normally, then consume its result. A capture can depend on
the completed operation even after its result has been taken:

```rust
use bed_plugin_automation::{AutomationResults, CaptureTarget, OperationValue};
use std::time::Duration;

if let Some(result) = world.resource_mut::<AutomationResults>().take(open) {
    let OperationValue::Opened(panel) = result? else { return Ok(()); };
    let capture = world.resource_mut::<AutomationRequests>().submit(
        Operation::Capture {
            targets: vec![CaptureTarget::Window, CaptureTarget::Panel(panel)],
            after: Some(open),
            timeout: Duration::from_secs(10),
        },
    );
    // Poll `capture` on subsequent updates and export its owned images.
}
```

These document operations require the crate's `app` feature.
`AppOperation::OpenDirectory` selects a live provider-qualified directory without
requiring a saved workspace. Enable the separate `workspaces` SDK feature for
`AttachWorkspace` and `SetWorkspace`, whose specification belongs to
`bed-plugin-workspaces`. Capture and
input operations require `DesktopPlugins` in the same app; generic panel commands
also work in an app without a window.

`Inspect` returns owned panel, command, layout and UI frame observations. With the `app` feature, it also includes workspace/document observations and typed panel readiness. Commands use the same published availability facts as menus. Handles include application and session identity as well as the entity generation; closed panels and replaced sessions cannot accidentally target later content. Attaching a workspace to the existing session preserves handles.

Semantic operations open/focus/split/close panels and invoke registered commands. The optional application API opens and reads files, applies revision-bound byte edits, saves or saves as, undoes/redoes, changes viewers/settings and selects directories. The `workspaces` feature adds
workspace attachment/replacement. These requests use the ordinary captured application queue and draft/save barriers.

`Input` accepts explicit `DesktopInputStep` values: one step per UI frame. Pointer, key, modifier, text and scroll events pass through the native interception, ImGui capture and gameplay routing stages. Key events do not insert text; provide a `Text` event once. Held input is released before the sequence receipt.

`Capture` batches window and visible panel content targets from the same final composited frame. Panel crops use content and clip geometry in logical coordinates, the actual display origin and physical framebuffer scale. A capture after an operation waits for a subsequent UI frame after that operation completes. A failed dependency, hidden/closed target, deadline or failed readback returns an error. Device replacement retries on the new generation and rejects obsolete callbacks.

Captured pixels are owned RGBA images and support PNG and PPM export. Window captures cover the client rendering; native titlebar controls/materials and the OS cursor are outside that image. Hidden panels are not rendered specially for capture.
