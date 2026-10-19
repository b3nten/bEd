//! A minimal host for protocol clients; no window, renderer or workbench.
use bedtime_automation2::AutomationPlugin;
use bevy::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins((MinimalPlugins, AutomationPlugin::stdio()))
        .run()
}
