use super::*;
use crate::application::{
    AppOptions, AppRuntimePlugin, RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore,
    runtime::{WorkbenchHostMode, open_file, process_app_requests},
    test_support::TempDir,
};
use bed_api::status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText};
use bed_workbench::{WorkbenchLayout, WorkbenchViewport};
use dear_imgui_rs::{Context, MouseButton, sys};
use std::{
    ffi::CString,
    path::{Path, PathBuf},
};

#[derive(Component, Default)]
struct Feedback(Option<StatusText>);

fn measure(world: &World, ui: &Ui, ctx: StatusContext) -> Option<f32> {
    world
        .get::<Feedback>(ctx.publisher)?
        .0
        .as_ref()
        .map(|text| text.measure(ui))
}

fn content(
    world: &mut World,
    ui: &Ui,
    ctx: StatusDrawContext,
    actions: &mut Vec<bed_api::workbench::WorkbenchAction>,
) {
    if let Some(text) = world
        .get::<Feedback>(ctx.context.publisher)
        .and_then(|feedback| feedback.0.as_ref())
    {
        text.draw(ui, ctx, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, ctx: StatusContext) {
    if let Some(text) = world
        .get::<Feedback>(ctx.publisher)
        .and_then(|feedback| feedback.0.as_ref())
    {
        text.tooltip(ui);
    }
}

struct Fixture {
    context: Context,
    app: App,
    panel: Entity,
}

impl Fixture {
    fn new(dir: &TempDir) -> Self {
        let config = dir.root().join("config");
        let settings = crate::config::Settings::with_paths(
            config.clone(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .to_path_buf()
                .to_path_buf(),
        )
        .unwrap();
        let mut app = App::new();
        app.insert_resource(crate::application::runtime::test_application_paths(&config))
            .insert_non_send(settings)
            .insert_non_send(bed_ui::icons::Icons::default())
            .insert_non_send(WorkspacePersistence::new(
                WorkspaceStore::load(&crate::application::runtime::test_application_paths(
                    &config,
                ))
                .unwrap(),
            ))
            .insert_resource(RemoteHelperDirectory(PathBuf::new()))
            .add_plugins((
                (bevy::MinimalPlugins, bevy::input::InputPlugin),
                AppRuntimePlugin {
                    options: AppOptions {
                        working_directory: Some(dir.root().to_owned()),
                        ..Default::default()
                    },
                },
                bed_plugin_editor::EditorPlugin,
            ));
        app.update();
        let panel = open_file(
            app.world_mut(),
            &dir.write("cursor.rs", b"fn main() {}\n"),
            None,
            false,
            None,
        )
        .unwrap();
        let mut context = Context::create();
        context.set_ini_filename::<PathBuf>(None).unwrap();
        context
            .io_mut()
            .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut fixture = Self {
            context,
            app,
            panel,
        };
        for _ in 0..5 {
            fixture.frame();
        }
        fixture
    }

    fn frame(&mut self) {
        self.app.update();
        self.context
            .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [800.0, 600.0],
                1.0 / 60.0,
            ));
        let ui = self.context.frame();
        if self
            .app
            .world()
            .non_send::<crate::application::ui::AppUi>()
            .host_mode
            == WorkbenchHostMode::Floating
        {
            ui.window("Embedding host")
                .position([60.0, 70.0], dear_imgui_rs::Condition::Always)
                .size([660.0, 440.0], dear_imgui_rs::Condition::Always)
                .build(|| crate::application::draw_ui(self.app.world_mut(), ui).unwrap());
        } else {
            crate::application::draw_ui(self.app.world_mut(), ui).unwrap();
        }
        drop(self.context.render_legacy());
        process_app_requests(self.app.world_mut()).unwrap();
    }

    fn footer(&self) -> ([f32; 2], [f32; 2]) {
        self.context.binding().with_bound_context(|| unsafe {
            let window =
                sys::igFindWindowByName(CString::new("##bed_status_bar").unwrap().as_ptr());
            assert!(!window.is_null());
            (
                [(*window).Pos.x, (*window).Pos.y],
                [(*window).Size.x, (*window).Size.y],
            )
        })
    }

    fn click(&mut self, point: [f32; 2]) {
        self.context.io_mut().add_mouse_pos_event(point);
        self.frame();
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        self.frame();
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        self.frame();
    }
}

#[test]
fn permanent_footer_keeps_canvas_and_focus_fixed_as_contributions_change() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut fixture = Fixture::new(&dir);
    let publisher = fixture
        .app
        .world_mut()
        .spawn((
            Feedback::default(),
            StatusItems(vec![StatusItem {
                id: "test.feedback",
                label: "Feedback",
                priority: 500,
                icon: "diagnostics",
                prefer_icon: false,
                measure,
                draw: content,
                tooltip,
            }]),
        ))
        .id();
    for mode in [WorkbenchHostMode::Fullscreen, WorkbenchHostMode::Floating] {
        fixture
            .app
            .world_mut()
            .non_send_mut::<crate::application::ui::AppUi>()
            .host_mode = mode;
        fixture.frame();
        fixture.frame();
        let footer = fixture.footer();
        let geometry = *fixture
            .app
            .world()
            .get::<bed_api::workbench::PanelGeometry>(fixture.panel)
            .unwrap();
        assert_eq!(
            footer.1[1],
            fixture
                .app
                .world()
                .resource::<WorkbenchViewport>()
                .bottom_inset
        );
        assert!(geometry.content_max[1] <= footer.0[1]);
        for text in [
            Some("Loading definition…".into()),
            Some("Roots Scanned\nvery long paths ".repeat(80)),
            Some("No definition found".into()),
            Some("Request timed out".into()),
            None,
        ] {
            fixture
                .app
                .world_mut()
                .get_mut::<Feedback>(publisher)
                .unwrap()
                .0 = text.map(|text: String| StatusText::new(text, "Full details"));
            fixture.frame();
            assert_eq!(fixture.footer(), footer);
            let now = fixture
                .app
                .world()
                .get::<bed_api::workbench::PanelGeometry>(fixture.panel)
                .unwrap();
            assert_eq!(now.content_min, geometry.content_min);
            assert_eq!(now.content_max, geometry.content_max);
            assert_eq!(
                fixture.app.world().resource::<WorkbenchLayout>().focused,
                Some(fixture.panel)
            );
        }
    }
}

#[test]
fn published_action_uses_registered_open_or_focus_command() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut fixture = Fixture::new(&dir);
    fixture.app.world_mut().spawn((
        Feedback(Some(
            StatusText::new("Open diagnostics", "Existing command routing")
                .command("Show Diagnostics", "bed.editor.show_diagnostics"),
        )),
        StatusItems(vec![StatusItem {
            id: "test.action",
            label: "Action",
            priority: 500,
            icon: "diagnostics",
            prefer_icon: false,
            measure,
            draw: content,
            tooltip,
        }]),
    ));
    fixture.frame();
    let footer = fixture.footer();
    fixture.click([footer.0[0] + 35.0, footer.0[1] + footer.1[1] * 0.5]);
    let first = fixture
        .app
        .world()
        .resource::<WorkbenchLayout>()
        .focused
        .unwrap();
    assert_eq!(
        fixture
            .app
            .world()
            .get::<bed_workbench::PanelRecord>(first)
            .unwrap()
            .panel_type,
        bed_plugin_editor::DIAGNOSTICS_PANEL_TYPE
    );
    fixture.click([footer.0[0] + 35.0, footer.0[1] + footer.1[1] * 0.5]);
    assert_eq!(
        fixture.app.world().resource::<WorkbenchLayout>().focused,
        Some(first)
    );
}

#[test]
fn host_information_shows_names_and_keeps_paths_in_details() {
    let mut workspace = WorkspaceState {
        spec: None,
        project_root: String::new(),
        working_directory: PathBuf::from("/home/user/launcher"),
        error: None,
        close_requested: false,
        windows: vec![],
    };
    assert_eq!(
        host_information(&workspace),
        (
            "launcher".into(),
            "Working directory: /home/user/launcher".into()
        )
    );
    workspace.spec = Some(crate::application::WorkspaceSpec::local("/srv/code"));
    assert_eq!(host_information(&workspace).0, "code");
    workspace.spec = Some(crate::application::WorkspaceSpec {
        name: "Remote project".into(),
        target: bed_plugin_workspaces::WorkspaceTarget::Ssh {
            host: "devbox".into(),
        },
        root: "/srv/code".into(),
    });
    let (summary, detail) = host_information(&workspace);
    assert_eq!(summary, "Remote project");
    assert!(detail.contains("devbox:/srv/code"));
    assert!(!detail.contains("/launcher"));
}
