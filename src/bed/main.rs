use bevy::prelude::*;

fn main() -> AppExit {
    if let Some(exit) = bed::handle_shell_command() {
        return exit;
    }
    App::new()
        .add_plugins((
            bed::default_plugins(),
            bed::BedPlugins,
            bed::runtime::LowLatencyPlugin,
        ))
        .run()
}
