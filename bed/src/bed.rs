//! bEd, the app: Bevy's plugins, bEd's (the `bed` library), and how it runs
//! (low latency: an app that builds on bEd chooses for itself).
use bevy::prelude::*;

fn main() -> AppExit {
    if let Some(exit) = bed::handle_shell_command() {
        return exit;
    }
    App::new()
        .add_plugins((
            bed::default_plugins(),
            bed::BedPlugins,
            // bEd's frames are light: a key shows sooner this way.
            bedtime::ui::LowLatencyPlugin,
        ))
        .run()
}
