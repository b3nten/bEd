# bedtime-plugin-layouts

Retained layouts for Bevy workbenches, with three separate responsibilities:

- A `LayoutDefinition` describes geometry, initial panel types, optional viewer
  types and initial focus. Applications may hardcode named or numbered definitions.
- A snapshot describes actual panel instances and saved feature state. Snapshots
  remain JSON so unavailable features and unvisited layouts retain opaque data.
- `LiveLayout` retains entities, widget state and processes while another layout
  is active. Switching back resumes those instances.

The crate does not select project roots or choose a filesystem persistence policy.
`LayoutsPlugin` installs `LayoutSlots<LayoutId, ()>` and the workbench if needed.
`LayoutId::new(number)` accepts any positive `u32`. Applications with named
layouts implement `LayoutKey` on their own enum; there is no bEd-specific slot enum.

Construct definitions using ordinary workbench geometry:

```rust,no_run
use bedtime_plugin_layouts::{LayoutDefinition, PanelDefinition};
use bedtime_workbench::workspace::tiling::Layout;
# fn definition() -> std::io::Result<LayoutDefinition> {
let mut geometry = Layout::default();
let right = geometry.split(1, 0, 6500, 1).unwrap();
let mut definition = LayoutDefinition::new(geometry);
let browser = definition.add_panel(PanelDefinition::new("example.browser"), 1)?;
definition.add_panel(PanelDefinition::new("example.details"), right)?;
definition.focus(browser)?;
# Ok(definition)
# }
```

`initial_state` configures a newly created panel. It does not overwrite a retained
instance. `to_value`/`from_value` persist definitions separately from snapshots.
`capture_definition::<WorkbenchLayouts>` captures geometry and panel types without
paths, histories, saved feature state or transient panels. `snapshot()` supplies
an initial plain-workbench snapshot for existing restore APIs.

Call `submit_transition::<LayoutId, WorkbenchLayouts>` between UI scopes with the
selected key and its fallback snapshot. `LayoutControllerPlugin<Key, Integration>` drives the FIFO between frames once the
integration's pending-operation barrier permits it. `LayoutsPlugin` installs the
plain numbered controller. A workbench permits one installed controller; manual
headless hosts can call `poll` explicitly. `take_result` consumes a
completion receipt; `cancel_requests` rejects queued intentions when their
workspace scope is replaced. Direct `transition` remains available for synchronous
hosts. The selected slot uses its retained instances first, then its saved
snapshot, then the supplied fallback definition.

`submit_rearrange` uses the same queue and save barrier as switching.
`rearrange::<WorkbenchLayouts>` resets an arrangement to a definition while
preserving existing panel state. It matches panel instances in tab order, creates
missing initial tools and places additional tabs in the largest area. Successful
changes advance the layout epoch. Failed creation or persistence restores the
previous geometry and focus and retires newly created panels.

The application may install `LayoutPersistence`, an ordinary registered Bevy
system returning `Result<(), String>`. The controller invokes it before leaving
an arrangement and after committing the next one. Use any storage adapter:
files, a database, a workspace repository, or application-owned storage.
Without the resource, layouts work entirely in memory.

`save(world)` captures all live layouts without activating suspended ones and
preserves saved slots that have never been loaded. Its shape is
`{"active_layout_slot": number, "layouts": {"number": workbench_snapshot}}`.
Load an application-selected key and snapshot map through `LayoutSlots::from_saved`,
then restore the selected snapshot with `restore`. IDs that collide with retained
panels are remapped. Missing features retain their opaque panel records.

Enable `bedtime`'s `layouts` feature for the ready-made
`bedtime::core::layouts::DocumentLayouts` integration. Install `DocumentLayoutsPlugin::new(your_initial_key)` after the
document host to drive it. It commits pending document
drafts, waits for host-operation barriers, restores document/view bindings, and
uses the normal document focus and retirement lifecycle. `definition_snapshot`
creates its initial snapshot; `attach_retained` adopts a workspace arrangement
without recreating carried documents and terminals. bEd's Startup rule, numbered
shortcuts and builtin arrangements stay in bEd.

Applications with a different navigation model may implement `LayoutIntegration`.
Its context capture, activation, creation, retirement and persistence stages stay
inside one controlled transition. Ordinary workbench requests capture the active
layout epoch; application-wide layout/quit actions use unscoped submission.
