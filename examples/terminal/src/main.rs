//! A complete desktop with one optional terminal panel.
use bed_plugin_terminal::TerminalPlugin;
use serde_json::Value;
use {bed_core::BedCorePlugin, bed_core::bevy::prelude::*};
use {bed_workbench::WorkbenchAction, bed_workbench::WorkbenchRequests};

fn main() {
    let mut app = App::new();
    app.add_plugins((BedCorePlugin::default(), TerminalPlugin));

    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::OpenPanel {
            panel_type: bed_plugin_terminal::PANEL_ID.into(),
            state: Value::Null,
            area: None,
        });
    app.run();
}
