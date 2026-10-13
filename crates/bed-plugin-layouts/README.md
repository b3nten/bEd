# bed-plugin-layouts

Retained layouts for Bevy workbenches. Switching suspends outgoing panels and
resumes their existing entities, widget state, and processes when returning.
The crate does not load settings, select project roots, or write persistence.

`LayoutsPlugin` installs `LayoutSlots<LayoutId, ()>` and the workbench if needed.
`LayoutId::new(number)` accepts any positive `u32`; applications choose their own
layout count. Applications with named layouts can implement `LayoutKey` on an
enum. `LayoutSlot` provides the compatibility keys `1` through `9` and `startup`.

Call `transition` between UI scopes, after finishing application save barriers:

```rust,no_run
use bed_plugin_layouts::{LayoutId, WorkbenchLayouts};
# fn switch(world: &mut bevy::prelude::World, template: &serde_json::Value) {
bed_plugin_layouts::transition::<LayoutId, WorkbenchLayouts>(
    world,
    LayoutId::new(12).unwrap(),
    template,
).unwrap();
# }
```

The selected slot uses its retained live layout first, then its saved snapshot,
then the supplied template. `save(world)` returns
`{"active_layout_slot": number, "layouts": {"number": workbench_snapshot}}`.
Persist that value through the application's storage. Load the application's
chosen active identity and snapshot map through `LayoutSlots::from_saved`, then
restore the selected snapshot with `restore`. Stored panel IDs are remapped when
they collide with panels retained in another slot. Missing features keep their
opaque saved panel state.

`template(world)` captures panel types and geometry without feature state or
transient panels. It does not change live panels.

Applications retaining extra navigation state use `LayoutSlots<Key, Context>`
and one `LayoutIntegration` implementation. That implementation captures and
activates navigation, restores panels, retires newly created panels on failure,
and finishes the transition, including persistence if required. The application
runs preflight before entering the transition. A failed restore or finish rolls
back membership, geometry, and navigation. Successful switches advance
`WorkbenchLayoutEpoch`, allowing requests captured in an earlier activation to
be rejected even after switching away and back to the same numbered layout.
Ordinary `WorkbenchRequests::push` and `submit` capture that activation.
Application-wide layout/quit actions explicitly use `push_unscoped` or
`submit_unscoped` so changing layouts does not discard them.

`snapshot` accepts an application serializer so its persisted panel schema can
remain unchanged. It serializes suspended live layouts without activating them
and preserves saved slots that have not been loaded.
