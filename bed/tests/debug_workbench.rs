#[path = "support/application_requests.rs"]
mod application_requests;
use application_requests::*;

#[path = "support/editor_host.rs"]
mod editor_host;
#[path = "../../bedtime/tests/support/temp_dir.rs"]
mod temp_dir;

use bed::application::*;
use bed::config::AppConfig;
use bedtime_api::{documents::PanelBinding, workbench::PanelTarget};
use bedtime_documents::DocumentSession;
use bedtime_plugin_debugger::{DebugState, DebuggerPlugin};
use bevy::prelude::*;
use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
use std::time::{Duration, Instant};
use temp_dir::TempDir;
use {
    bedtime_workbench::PanelPresentation, bedtime_workbench::WorkbenchAction,
    bedtime_workbench::WorkbenchLayout, bedtime_workbench::WorkbenchRequests,
};

#[test]
fn debugger_start_saves_and_runs_and_source_navigation_replaces_previews() {
    let dir = TempDir::new();
    let root = dir.root().canonicalize().unwrap();
    let main = dir.write("main.rs", b"fn main() {\n    let number = 42;\n}\n");
    let mut app = App::new();
    editor_host::install(
        &mut app,
        AppConfig::with_paths(dir.path("config"), env!("CARGO_MANIFEST_DIR").into()).unwrap(),
    )
    .unwrap();
    app.add_plugins((
        (bevy::MinimalPlugins, bevy::input::InputPlugin),
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(root.clone()),
                initial_layout: Some(LayoutSlot::Layout1),
                ..Default::default()
            },
        },
        bedtime_plugin_editor::EditorPlugin,
        DebuggerPlugin,
    ));
    app.update();
    let source_area = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .layout
        .largest_area();
    let source = open_file(app.world_mut(), &main, None, false, Some(source_area)).unwrap();
    let binding = app.world().get::<PanelBinding>(source).unwrap().clone();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(binding.view.unwrap(), |commands| {
            commands.set_cursor(
                1,
                0,
                false,
                bedtime_editing::text_commands::CursorReveal::Center,
            )
        })
        .unwrap();
    // Publish command availability for the newly opened source view.
    app.update();
    dispatch_app_plugin_command(
        app.world_mut(),
        bedtime_plugin_debugger::BREAKPOINT_COMMAND,
        PanelTarget::LastFocused,
    )
    .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DebugState>()
            .0
            .source
            .breakpoints
            .len(),
        1
    );

    // Layout 1 keeps Debug below the source area, without requiring a workspace.
    let debug = app
        .world_mut()
        .query::<(Entity, &PanelRecord)>()
        .iter(app.world())
        .find(|(entity, record)| {
            bedtime_workbench::owns_panel(app.world(), *entity)
                && record.panel_type == bedtime_plugin_debugger::PANEL_ID
        })
        .unwrap()
        .0;
    let debug_id = app.world().get::<PanelPresentation>(debug).unwrap().id;
    let debug_area = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(debug_id)
        .unwrap();
    assert_ne!(debug_area, source_area);
    let debug_surface = panel_presentation(app.world(), debug).unwrap();
    {
        let mut debugger = app.world_mut().non_send_mut::<DebugState>();
        debugger.0.profiles = vec![bedtime_debugger::DebugProfile {
            name: "Build fixture".into(),
            program: dir.path("fixture").to_string_lossy().into_owned(),
            build_command: "printf started > launch.started; exit 7".into(),
            ..Default::default()
        }];
        debugger.0.selected = 0;
        debugger.0.configure = false;
    }
    let saved = b"fn main() {\n    let number = 43;\n}\n";
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .replace_content(binding.document.unwrap(), saved)
        .unwrap();
    publish_app_state(app.world_mut());

    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut actions = Vec::new();
    for frame in 0..3 {
        context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Debug fixture")
            .position([0.0; 2], Condition::Always)
            .size([1000.0, 700.0], Condition::Always)
            .build(|| {
                actions.extend(
                    bedtime_workbench::render_panel(app.world_mut(), debug_surface, ui).unwrap(),
                );
                if frame == 0 {
                    ui.with_bound_context(|| unsafe {
                        dear_imgui_rs::sys::igActivateItemByID(ui.get_id("Start").raw());
                    });
                }
            });
        let _ = context.render_legacy();
    }
    let [WorkbenchAction::Invoke(invocation)] = actions.as_slice() else {
        panic!("Start must emit one captured command: {actions:?}");
    };
    assert_eq!(invocation.source, Some(debug_surface));
    assert_eq!(invocation.area, debug_area);
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(actions.pop().unwrap());
    process_app_requests(app.world_mut()).unwrap();
    assert_eq!(std::fs::read(&main).unwrap(), saved);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.world().non_send::<DebugState>().0.error.is_none() {
        assert!(
            Instant::now() < deadline,
            "Start did not pass the save barrier and run the build"
        );
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read(dir.path("launch.started")).unwrap(),
        b"started"
    );
    assert!(
        app.world()
            .non_send::<DebugState>()
            .0
            .error
            .as_ref()
            .unwrap()
            .contains("Build failed")
    );
    assert!(app.world().resource::<ApplicationState>().error.is_none());

    for _ in 0..3 {
        dispatch_app_plugin_command(
            app.world_mut(),
            bedtime_plugin_debugger::SHOW_COMMAND,
            PanelTarget::LargestArea,
        )
        .unwrap();
    }
    let debug_count = app
        .world_mut()
        .query::<(Entity, &PanelRecord)>()
        .iter(app.world())
        .filter(|(entity, record)| {
            bedtime_workbench::owns_panel(app.world(), *entity)
                && record.panel_type == bedtime_plugin_debugger::PANEL_ID
        })
        .count();
    assert_eq!(
        debug_count, 1,
        "launch and inspection must reuse the Debug tab"
    );

    // Session updates choose a live source area and replace the previous preview.
    let mut previous = None;
    for name in ["first.rs", "second.rs", "third.rs"] {
        let path = dir.write(name, "// target\nfn α() {}\n".as_bytes());
        app.world_mut().non_send_mut::<DebugState>().0.navigation =
            Some((path.to_string_lossy().into_owned(), 1, 5));
        app.update();
        assert!(app.world().resource::<ApplicationState>().error.is_none());
        let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        let id = app.world().get::<PanelPresentation>(panel).unwrap().id;
        let layout = app.world().resource::<WorkbenchLayout>();
        assert_eq!(
            layout.tiling.area_for(id),
            Some(layout.tiling.layout.largest_area())
        );
        assert_eq!(
            layout
                .tiling
                .areas
                .iter()
                .find(|area| area.area == source_area)
                .unwrap()
                .preview,
            Some(id)
        );
        if let Some(previous) = previous {
            assert!(!app.world().entities().contains(previous));
        }
        let view = app
            .world()
            .get::<PanelBinding>(panel)
            .unwrap()
            .view
            .unwrap();
        let cursor = app
            .world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap();
        assert_eq!(
            (cursor.primary().head_row, cursor.primary().head_column),
            (1, 5)
        );
        assert!(app.world().entities().contains(source));
        previous = Some(panel);
    }

    // Breakpoint-list navigation also previews in the editor area, even though
    // its command originates in the Debug area.
    let path = dir.write("breakpoint.rs", b"fn breakpoint() {}\n");
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(
            bedtime_workbench::CommandInvocation {
                command: "bedtime.debug.action".into(),
                source: Some(debug_surface),
                area: debug_area,
                arguments: serde_json::to_value(
                    bedtime_plugin_debugger::fixtures::DebuggerAction::Navigate(
                        path.to_string_lossy().into_owned(),
                        0,
                    ),
                )
                .unwrap(),
            },
        ));
    process_app_requests(app.world_mut()).unwrap();
    assert!(app.world().resource::<ApplicationState>().error.is_none());
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let id = app.world().get::<PanelPresentation>(panel).unwrap().id;
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(id),
        Some(source_area)
    );
    assert!(!app.world().entities().contains(previous.unwrap()));
    assert!(app.world().entities().contains(source));

    // Editing a preview keeps it, so subsequent stepping cannot discard work.
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .replace_content(document, b"fn breakpoint() { /* unsaved */ }\n")
        .unwrap();
    let next = dir.write("after_edit.rs", b"fn after_edit() {}\n");
    app.world_mut().non_send_mut::<DebugState>().0.navigation =
        Some((next.to_string_lossy().into_owned(), 0, 0));
    app.update();
    assert!(app.world().resource::<ApplicationState>().error.is_none());
    assert!(app.world().entities().contains(panel));
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
    let layout = app.world().resource::<WorkbenchLayout>();
    assert_ne!(
        layout
            .tiling
            .areas
            .iter()
            .find(|area| area.area == source_area)
            .unwrap()
            .preview,
        Some(id)
    );
}
