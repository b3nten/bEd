//! The default application publishes help through the same factories users open.
#[path = "support/editor_host.rs"]
mod editor_host;
#[path = "support/temp_dir.rs"]
#[allow(dead_code)]
mod temp_dir;

use bed::application::{AppOptions, AppRuntimePlugin};
use bed_api::documents::PanelInput;
use bevy::prelude::*;
use serde_json::Value;
use {
    bed_workbench::DOCS_PANEL_ID, bed_workbench::SHOW_DOCS_COMMAND,
    bed_workbench::WorkbenchRequests, bed_workbench::api,
};

#[test]
fn builtins_publish_usage_notes_and_show_docs_is_available_and_reuses_the_panel() {
    let directory = temp_dir::TempDir::new();
    let mut app = App::new();
    editor_host::install(
        &mut app,
        bed::config::Settings::with_paths(
            directory.path("config"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap(),
    )
    .unwrap();
    app.add_plugins((
        (bevy::MinimalPlugins, bevy::input::InputPlugin),
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(directory.root().to_owned()),
                ..Default::default()
            },
        },
    ))
    .add_plugins(bed::builtins::BedBuiltinPlugins);
    app.update();
    assert!(
        bed::application::application_commands(app.world())
            .iter()
            .any(|command| command.id == SHOW_DOCS_COMMAND)
    );
    let panels = app
        .world()
        .resource::<api::WorkbenchRegistry>()
        .panels()
        .iter()
        .map(|panel| panel.id)
        .collect::<Vec<_>>();
    let mut docs_panel = None;
    for kind in panels {
        let panel = bed::application::open_panel(
            app.world_mut(),
            kind,
            PanelInput::None,
            Value::Null,
            None,
            None,
            false,
        )
        .unwrap();
        let frontend = bed::application::panel_presentation(app.world(), panel).unwrap();
        let docs = app
            .world()
            .get::<api::PanelDocumentation>(frontend)
            .unwrap_or_else(|| panic!("{kind} must publish usage notes"));
        assert!(
            docs.sections.iter().any(|section| !section.body.is_empty()),
            "{kind} must publish useful content"
        );
        if kind == DOCS_PANEL_ID {
            docs_panel = Some(panel);
        }
    }
    let invocation = bed_workbench::capture_command(app.world(), SHOW_DOCS_COMMAND, Value::Null);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(api::WorkbenchAction::Invoke(invocation));
    bed::application::process_app_requests(app.world_mut()).unwrap();
    assert_eq!(
        app.world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused,
        docs_panel
    );
}
