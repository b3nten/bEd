//! bEd, the app: Bevy's plugins, bEd's (the `bed` library), and how it runs.
use bevy::prelude::*;

fn main() -> AppExit {
    if let Some(exit) = bed::handle_shell_command() {
        return exit;
    }
    App::new()
        .add_plugins((
            bed::default_plugins(),
            bed::BedPlugins,
            // Last, so it sees every schedule the plugins made.
            bed::runtime::RuntimePlugin,
        ))
        .run()
}
