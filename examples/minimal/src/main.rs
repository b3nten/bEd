//! A fresh Bevy app composed from the desktop, workbench and terminal crates.
use bed_desktop::{DesktopPlugins, DesktopSystems, DesktopUi, bevy::prelude::*};
use bed_editor_api::{EditorIntents, HostRequest};
use bed_terminal::TerminalPlugin;
use bed_workbench::{Action, WorkbenchPlugin, WorkbenchRequests, WorkbenchSystems};
use serde_json::Value;

fn main() {
    if let Some(result) = bedterm::run_shell_command(std::env::args_os(), false) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    let mut app = App::new();
    app.add_plugins(DesktopPlugins::default())
        .add_plugins((WorkbenchPlugin, TerminalPlugin))
        .insert_resource(DesktopUi(bed_workbench::render_ui))
        .configure_sets(
            PreUpdate,
            WorkbenchSystems::Requests.in_set(DesktopSystems::AfterUi),
        )
        .add_systems(
            Update,
            |mut intents: ResMut<EditorIntents>, mut requests: ResMut<WorkbenchRequests>| {
                for error in requests.take_errors() {
                    eprintln!("{error}");
                }
                for intent in intents.take() {
                    match intent.request {
                        HostRequest::Notify { message } => eprintln!("{message}"),
                        HostRequest::Invalidate => {} // The desktop redraws continuously.
                        _ => eprintln!("This request requires an editor host."),
                    }
                }
            },
        );

    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(Action::OpenPanel {
            panel_type: bed_terminal::PANEL_ID.into(),
            state: Value::Null,
            area: None,
        });
    app.run();
}
