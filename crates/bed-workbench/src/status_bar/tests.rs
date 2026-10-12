use super::*;
use crate::{WorkbenchPlugin, tests::IMGUI_TEST_LOCK};
use dear_imgui_rs::{Context, FramePrepareOptions, MouseButton};
use serde_json::Value;

#[derive(Component)]
struct Data {
    width: f32,
    enabled: bool,
    clicks: usize,
    drawn: Vec<StatusDrawMode>,
    control: Option<[f32; 2]>,
}
fn measure(world: &World, _: &Ui, ctx: StatusContext) -> Option<f32> {
    let data = world.get::<Data>(ctx.publisher).unwrap();
    data.enabled.then_some(data.width)
}
fn draw(world: &mut World, ui: &Ui, ctx: StatusDrawContext, actions: &mut Vec<Action>) {
    world
        .get_mut::<Data>(ctx.context.publisher)
        .unwrap()
        .drawn
        .push(ctx.mode);
    if ui.button("Increment") {
        world.get_mut::<Data>(ctx.context.publisher).unwrap().clicks += 1;
        actions.push(ctx.context.invoke("test.record", Value::Null));
    }
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    world
        .get_mut::<Data>(ctx.context.publisher)
        .unwrap()
        .control = Some([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]);
}
fn tooltip(_: &World, ui: &Ui, _: StatusContext) {
    ui.text("Current custom content");
}
fn item(id: &'static str, priority: i32, icon: &'static str, prefer_icon: bool) -> StatusItem {
    StatusItem {
        id,
        label: id,
        priority,
        icon,
        prefer_icon,
        measure,
        draw,
        tooltip,
    }
}
fn publisher(app: &mut App, descriptor: StatusItem, width: f32) -> Entity {
    app.world_mut()
        .spawn((
            StatusItems(vec![descriptor]),
            Data {
                width,
                enabled: true,
                clicks: 0,
                drawn: vec![],
                control: None,
            },
        ))
        .id()
}
fn icons(_: &World, key: &str) -> Option<TextureId> {
    (key == "test").then(|| TextureId::new(1))
}
struct Fixture {
    context: Context,
    app: App,
    width: f32,
}
impl Fixture {
    fn new(width: f32) -> Self {
        let mut context = Context::create();
        context
            .set_ini_filename::<std::path::PathBuf>(None)
            .unwrap();
        context
            .io_mut()
            .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut app = App::new();
        app.add_plugins(WorkbenchPlugin);
        Self {
            context,
            app,
            width,
        }
    }
    fn frame(&mut self) {
        self.context
            .prepare_frame(FramePrepareOptions::new([self.width, 300.0], 1.0 / 60.0));
        let ui = self.context.frame();
        self.app
            .world_mut()
            .resource_mut::<WorkbenchViewport>()
            .bottom_inset = status_bar_height(ui);
        crate::render_ui(self.app.world_mut(), ui);
        render_status_bar(
            self.app.world_mut(),
            ui,
            StatusBarHost {
                icon: "test",
                summary: "workspace",
                details: "Workspace path",
            },
            icons,
        );
        drop(self.context.render_legacy());
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
fn arbitrary_publishers_order_and_compress_without_panel_filters() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    let low = publisher(&mut fixture.app, item("test.low", 1, "test", false), 200.0);
    let high = publisher(
        &mut fixture.app,
        item("test.high", 100, "test", false),
        200.0,
    );
    let preferred = publisher(
        &mut fixture.app,
        item("test.preferred", 50, "test", true),
        60.0,
    );
    let missing = publisher(
        &mut fixture.app,
        item("test.missing", 20, "missing", true),
        100.0,
    );
    fixture
        .context
        .prepare_frame(FramePrepareOptions::new([800.0, 300.0], 1.0 / 60.0));
    let ui = fixture.context.frame();
    let mut state = StatusBarState::default();
    let mut entries = collect(fixture.app.world_mut(), ui, icons, &mut state);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.item.id)
            .collect::<Vec<_>>(),
        ["test.high", "test.preferred", "test.missing", "test.low"]
    );
    assert_eq!(entries[1].representation, Representation::Icon);
    assert_eq!(entries[2].representation, Representation::Icon);
    allocate(&mut entries, 400.0, ui.text_line_height(), 20.0);
    assert_eq!(entries[0].representation, Representation::Inline);
    assert_eq!(entries[3].representation, Representation::Icon);
    allocate(&mut entries, 70.0, ui.text_line_height(), 20.0);
    assert!(
        entries
            .iter()
            .any(|entry| entry.representation == Representation::Overflow)
    );
    assert!(total_width(&entries, ui.text_line_height(), 20.0) <= 70.0);
    assert!(entries.iter().all(|entry| entry.context.source.is_none()));
    drop(fixture.context.render_legacy());
    fixture
        .app
        .world_mut()
        .get_mut::<Data>(low)
        .unwrap()
        .enabled = false;
    fixture.app.world_mut().despawn(missing);
    fixture.frame();
    assert!(
        !fixture
            .app
            .world()
            .get::<Data>(high)
            .unwrap()
            .drawn
            .is_empty()
    );
    assert!(
        fixture
            .app
            .world()
            .get::<Data>(preferred)
            .unwrap()
            .drawn
            .is_empty()
    );
}

#[test]
fn status_tooltips_wrap_round_and_animate_for_icons_contents_and_host() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    fn details(_: &World, ui: &Ui, _: StatusContext) {
        api::StatusText::new(
            "Current status details",
            "Location: /workspace/projects/a-long-directory-name/source-file.rs\n".repeat(3),
        )
        .tooltip(ui);
    }
    for (id, priority, prefer_icon) in [("test.compact", 100, true), ("test.inline", 50, false)] {
        let mut descriptor = item(id, priority, "test", prefer_icon);
        descriptor.tooltip = details;
        publisher(&mut fixture.app, descriptor, 100.0);
    }
    for _ in 0..3 {
        fixture.frame();
    }
    let side = fixture
        .app
        .world()
        .resource::<WorkbenchViewport>()
        .bottom_inset
        - PADDING[1] * 2.0;
    for point in [
        [16.0, 285.0],
        [10.0 + side * 2.0 + GAP + ICON_GAP + 10.0, 285.0],
        [770.0, 285.0],
    ] {
        fixture.context.io_mut().add_mouse_pos_event(point);
        let mut alphas = Vec::new();
        for _ in 0..20 {
            fixture.frame();
            let alpha = fixture.context.binding().with_bound_context(|| unsafe {
                let context = &*sys::igGetCurrentContext();
                let window = (0..context.Windows.Size).find_map(|index| {
                    let window = &**context.Windows.Data.add(index as usize);
                    (window.Active
                        && !window.Hidden
                        && window.Flags & sys::ImGuiWindowFlags_Tooltip != 0)
                        .then_some(window)
                })?;
                assert!(
                    window.Size.x > 100.0 && window.Size.x < 550.0,
                    "tooltip width: {}",
                    window.Size.x
                );
                assert!(window.Size.y < 200.0, "tooltip height: {}", window.Size.y);
                assert_eq!(window.WindowRounding, popup_style::POPUP_ROUNDING);
                assert_eq!(window.WindowBorderSize, popup_style::POPUP_BORDER_SIZE);
                assert_eq!(
                    [window.WindowPadding.x, window.WindowPadding.y],
                    [12.0, 10.0]
                );
                assert_eq!(window.ViewportId, (*sys::igGetMainViewport()).ID);
                let draw = &*window.DrawList;
                let alpha = (0..draw.VtxBuffer.Size as usize)
                    .map(|index| (*draw.VtxBuffer.Data.add(index)).col >> 24)
                    .max()
                    .unwrap();
                let footer = &*sys::igFindWindowByName(c"##bed_status_bar".as_ptr());
                let draw = &*footer.DrawList;
                let footer_alpha = (0..draw.VtxBuffer.Size as usize)
                    .map(|index| (*draw.VtxBuffer.Data.add(index)).col >> 24)
                    .max()
                    .unwrap();
                assert_eq!(footer_alpha, 255, "the footer itself must remain opaque");
                Some(alpha)
            });
            if let Some(alpha) = alpha {
                alphas.push(alpha);
            }
        }
        assert!(
            alphas.iter().any(|alpha| *alpha < 200),
            "opening animation missing: {alphas:?}"
        );
        assert_eq!(alphas.last(), Some(&255));
    }
}

#[test]
fn compact_widget_popup_is_interactive_and_withdrawal_closes_it() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    let publisher = publisher(
        &mut fixture.app,
        item("test.widget", 50, "test", true),
        80.0,
    );
    for _ in 0..3 {
        fixture.frame();
    }
    fixture.click([16.0, 285.0]);
    // ImGui resolves auto-sized popup placement on the following frame.
    fixture.frame();
    assert!(
        fixture
            .app
            .world()
            .get::<Data>(publisher)
            .unwrap()
            .drawn
            .contains(&StatusDrawMode::Popup)
    );
    let point = fixture
        .app
        .world()
        .get::<Data>(publisher)
        .unwrap()
        .control
        .unwrap();
    fixture.click(point);
    assert_eq!(
        fixture.app.world().get::<Data>(publisher).unwrap().clicks,
        1
    );
    let queued = fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .take_next()
        .unwrap();
    assert!(
        matches!(queued.action, Action::Invoke(ref invocation) if invocation.command == "test.record" && invocation.source.is_none())
    );
    fixture
        .app
        .world_mut()
        .get_mut::<Data>(publisher)
        .unwrap()
        .enabled = false;
    fixture.frame();
    assert!(
        fixture
            .app
            .world()
            .resource::<StatusBarState>()
            .popup
            .is_none()
    );
    assert!(!crate::frontend_active(fixture.app.world()));
}

#[test]
fn low_priority_item_overflows_before_high_priority_item_compacts() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    publisher(
        &mut fixture.app,
        item("test.high", 300, "test", false),
        100.0,
    );
    publisher(&mut fixture.app, item("test.low", 50, "test", false), 100.0);
    publisher(
        &mut fixture.app,
        item("test.lower", 40, "test", false),
        100.0,
    );
    fixture
        .context
        .prepare_frame(FramePrepareOptions::new([800.0, 300.0], 1.0 / 60.0));
    let ui = fixture.context.frame();
    let mut state = StatusBarState::default();
    let mut entries = collect(fixture.app.world_mut(), ui, icons, &mut state);
    let budget = 100.0 + ui.text_line_height() + ICON_GAP + GAP + 20.0;
    allocate(&mut entries, budget, ui.text_line_height(), 20.0);
    assert_eq!(entries[0].representation, Representation::Inline);
    assert_eq!(entries[1].representation, Representation::Overflow);
    assert_eq!(entries[2].representation, Representation::Overflow);
    drop(fixture.context.render_legacy());
}

#[test]
fn inline_and_overflow_widgets_both_accept_mouse_input() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(290.0);
    let inline = publisher(
        &mut fixture.app,
        item("test.inline", 200, "test", false),
        100.0,
    );
    let hidden = publisher(
        &mut fixture.app,
        item("test.hidden", 50, "test", false),
        1000.0,
    );
    publisher(
        &mut fixture.app,
        item("test.extra", 40, "test", false),
        1000.0,
    );
    for _ in 0..3 {
        fixture.frame();
    }
    let point = fixture
        .app
        .world()
        .get::<Data>(inline)
        .unwrap()
        .control
        .unwrap();
    fixture.click(point);
    assert_eq!(fixture.app.world().get::<Data>(inline).unwrap().clicks, 1);
    assert!(
        fixture
            .app
            .world()
            .get::<Data>(hidden)
            .unwrap()
            .drawn
            .is_empty()
    );
    let side = fixture
        .app
        .world()
        .resource::<WorkbenchViewport>()
        .bottom_inset
        - PADDING[1] * 2.0;
    fixture.click([10.0 + 100.0 + side + ICON_GAP + GAP + 4.0, 285.0]);
    fixture.frame();
    let point = fixture
        .app
        .world()
        .get::<Data>(hidden)
        .unwrap()
        .control
        .unwrap();
    fixture.click(point);
    assert_eq!(fixture.app.world().get::<Data>(hidden).unwrap().clicks, 1);
    assert!(
        fixture
            .app
            .world()
            .get::<Data>(hidden)
            .unwrap()
            .drawn
            .contains(&StatusDrawMode::Popup)
    );
    assert!(
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_next()
            .is_some()
    );
}

#[test]
fn popup_actions_retain_panel_source_and_area_across_focus_changes() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    for prefer_icon in [true, false] {
        let mut fixture = Fixture::new(800.0);
        let owner = fixture
            .app
            .world_mut()
            .spawn(api::PanelPresentation::new(1))
            .id();
        let publisher = publisher(
            &mut fixture.app,
            item("test.target", 50, "test", prefer_icon),
            1000.0,
        );
        fixture
            .app
            .world_mut()
            .entity_mut(publisher)
            .insert(api::PanelOwner(owner));
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchLayout>()
            .focused_area = Some(7);
        for _ in 0..3 {
            fixture.frame();
        }
        fixture.click([16.0, 285.0]);
        fixture.frame();
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchLayout>()
            .focused_area = Some(12);
        fixture.frame();
        let point = fixture
            .app
            .world()
            .get::<Data>(publisher)
            .unwrap()
            .control
            .unwrap();
        fixture.click(point);
        let action = fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_next()
            .unwrap()
            .action;
        let Action::Invoke(invocation) = action else {
            panic!("expected command")
        };
        assert_eq!(invocation.source, Some(publisher));
        assert_eq!(invocation.area, 7);
    }
}

#[test]
fn dismissed_compact_popup_stays_closed_and_invalid_measurement_is_isolated() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    let publisher = publisher(
        &mut fixture.app,
        item("test.compact", 200, "test", true),
        80.0,
    );
    for _ in 0..3 {
        fixture.frame();
    }
    fixture.click([16.0, 285.0]);
    fixture.frame();
    fixture.click([400.0, 100.0]);
    fixture.frame();
    assert!(
        fixture
            .app
            .world()
            .resource::<StatusBarState>()
            .popup
            .is_none()
    );
    let count = fixture
        .app
        .world()
        .get::<Data>(publisher)
        .unwrap()
        .drawn
        .len();
    fixture.frame();
    assert_eq!(
        fixture
            .app
            .world()
            .get::<Data>(publisher)
            .unwrap()
            .drawn
            .len(),
        count
    );
    fixture
        .app
        .world_mut()
        .get_mut::<Data>(publisher)
        .unwrap()
        .width = f32::NAN;
    fixture.frame();
    assert!(
        fixture.app.world().resource::<StatusBarState>().failures[&(publisher, "test.compact")]
            .contains("finite")
    );
    fixture
        .app
        .world_mut()
        .entity_mut(publisher)
        .remove::<StatusItems>();
    fixture.frame();
    assert!(
        fixture
            .app
            .world()
            .resource::<StatusBarState>()
            .failures
            .is_empty()
    );
}

#[test]
fn failing_widget_discards_actions_and_does_not_interrupt_other_items() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new(800.0);
    fn broken(_: &mut World, _: &Ui, ctx: StatusDrawContext, actions: &mut Vec<Action>) {
        actions.push(ctx.context.invoke("test.unwanted", Value::Null));
        unsafe {
            sys::igBeginChild_Str(
                std::ffi::CString::new("damaged").unwrap().as_ptr(),
                sys::ImVec2 { x: 10.0, y: 10.0 },
                0,
                0,
            );
        }
        panic!("broken producer");
    }
    let mut descriptor = item("test.broken", 100, "test", false);
    descriptor.draw = broken;
    let bad = publisher(&mut fixture.app, descriptor, 80.0);
    let healthy = publisher(
        &mut fixture.app,
        item("test.healthy", 50, "test", false),
        100.0,
    );
    fixture.frame();
    assert!(
        fixture
            .app
            .world()
            .resource::<StatusBarState>()
            .failures
            .contains_key(&(bad, "test.broken"))
    );
    assert!(
        !fixture
            .app
            .world()
            .get::<Data>(healthy)
            .unwrap()
            .drawn
            .is_empty()
    );
    assert!(
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_next()
            .is_none()
    );
    fixture.frame();
    assert!(!crate::frontend_active(fixture.app.world()));
}
