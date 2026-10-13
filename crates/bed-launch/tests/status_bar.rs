use bed_api::status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText};
use bed_desktop::dear_imgui_rs;
use bed_workbench::{WorkbenchLayout, WorkbenchViewport};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use dear_imgui_rs::{Context, MouseButton, sys};
use std::{ffi::CString, path::PathBuf};
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;
use test_support::TempDir;
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[derive(Clone, Copy, PartialEq, Eq)]
enum HostMode {
    Fullscreen,
    Floating,
}
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
    mode: HostMode,
}

impl Fixture {
    fn new(dir: &TempDir) -> Self {
        let mut app = App::new();
        app.init_resource::<bed_desktop::DesktopFrame>()
            .insert_resource(bed_api::files::FileContext::local(dir.root()).unwrap())
            .add_plugins((
                bevy::MinimalPlugins,
                bevy::input::InputPlugin,
                bed_core::DocumentHostPlugin,
                bed_plugin_editor::EditorPlugin,
            ));
        bed_core::settings::initialize_settings(app.world_mut(), None).unwrap();
        bed_core::settings::finalize_settings(app.world_mut()).unwrap();
        app.update();
        let panel = bed_core::open_file(
            app.world_mut(),
            &dir.write("cursor.rs", b"fn main() {}\n"),
            None,
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
            mode: HostMode::Fullscreen,
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
        let mode = self.mode;
        let world = self.app.world_mut();
        let draw = |world: &mut World, ui: &Ui| {
            bed_core::prepare_workbench_ui(world);
            let bounds = if mode == HostMode::Floating {
                Some((ui.cursor_screen_pos(), ui.content_region_avail()))
            } else {
                None
            };
            let mut viewport = world.resource_mut::<WorkbenchViewport>();
            viewport.bounds = bounds;
            viewport.bottom_inset = bed_workbench::status_bar_height(ui);
            bed_workbench::render_ui(world, ui);
            bed_workbench::render_status_bar(
                world,
                ui,
                bed_workbench::StatusBarHost {
                    icon: "bed.core.default",
                    summary: "Minimal workbench",
                    details: "",
                },
                |_, _| None,
            );
            bed_core::finish_workbench_ui(world);
        };
        if mode == HostMode::Floating {
            ui.window("Embedding host")
                .position([60.0, 70.0], dear_imgui_rs::Condition::Always)
                .size([660.0, 440.0], dear_imgui_rs::Condition::Always)
                .build(|| draw(world, ui));
        } else {
            draw(world, ui);
        }
        drop(self.context.render_legacy());
        bed_core::resolve_documents(self.app.world_mut());
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
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
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
    for mode in [HostMode::Fullscreen, HostMode::Floating] {
        fixture.mode = mode;
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
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
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
