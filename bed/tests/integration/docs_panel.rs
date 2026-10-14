//! The default application publishes help through the same factories users open.
use crate::application_requests;

use crate::editor_host;
use crate::temp_dir;

use bed::application::{AppOptions, AppRuntimePlugin};
use bedtime_api::documents::PanelInput;
use bevy::prelude::*;
use serde_json::Value;
use {bedtime_workbench::SHOW_DOCS_COMMAND, bedtime_workbench::api};

#[test]
fn builtins_publish_usage_notes() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
    }
}
