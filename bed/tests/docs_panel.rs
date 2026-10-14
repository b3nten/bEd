//! The default application publishes help through the same factories users open.
#[path = "support/application_requests.rs"]
mod application_requests;

#[path = "support/editor_host.rs"]
mod editor_host;
#[path = "../../bedtime/tests/support/temp_dir.rs"]
#[allow(dead_code)]
mod temp_dir;

use bed::application::{AppOptions, AppRuntimePlugin};
use bedtime_api::documents::PanelInput;
use bevy::prelude::*;
use serde_json::Value;
use {
    bedtime_workbench::DOCS_PANEL_ID, bedtime_workbench::SHOW_DOCS_COMMAND,
    bedtime_workbench::WorkbenchRequests, bedtime_workbench::api,
};

#[test]
fn builtins_publish_usage_notes_and_show_docs_is_available_and_reuses_the_panel() {
    let directory = temp_dir::TempDir::new();
    let mut app = App::new();
    editor_host::install(
        &mut app,
        bed::config::AppConfig::with_paths(
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
        application_requests::application_commands(app.world())
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
        let panel = application_requests::open_panel(
            app.world_mut(),
            kind,
            PanelInput::None,
            Value::Null,
            None,
            None,
            false,
        )
        .unwrap();
        let frontend = bedtime_core::panel_presentation(app.world(), panel).unwrap();
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
    let invocation =
        bedtime_workbench::capture_command(app.world(), SHOW_DOCS_COMMAND, Value::Null);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(api::WorkbenchAction::Invoke(invocation));
    application_requests::process_app_requests(app.world_mut()).unwrap();
    assert_eq!(
        app.world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .focused,
        docs_panel
    );
}
