use super::*;
use bed_core::editor_commands::CursorReveal;
use dear_imgui_rs::{Context, FramePrepareOptions};

#[derive(Clone, Copy, Debug, Default)]
struct ScrollSample {
    position: [f32; 2],
    maximum: [f32; 2],
    viewport: [f32; 2],
    content: [f32; 2],
    line_height: f32,
    scrollbar_rect: [[f32; 2]; 2],
    scrollbar_active: bool,
}

impl ScrollSample {
    fn centered_y(self, row: i32) -> f32 {
        (row as f32 * self.line_height - (self.viewport[1] - self.line_height) * 0.5)
            .clamp(0.0, self.maximum[1])
    }
}

fn context() -> Context {
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

fn sample(ui: &Ui, parent: &str, child: &str, line_height: f32) -> ScrollSample {
    ui.with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        for index in 0..native.Windows.Size {
            let window_ptr = *native.Windows.Data.add(index as usize);
            let window = &*window_ptr;
            let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
            if name.contains(parent) && name.contains(child) {
                let scrollbar = sys::igGetWindowScrollbarRect(window_ptr, sys::ImGuiAxis_Y);
                return ScrollSample {
                    position: [window.Scroll.x, window.Scroll.y],
                    maximum: [window.ScrollMax.x, window.ScrollMax.y],
                    viewport: [window.Size.x, window.Size.y],
                    content: [window.ContentSize.x, window.ContentSize.y],
                    line_height,
                    scrollbar_rect: [
                        [scrollbar.Min.x, scrollbar.Min.y],
                        [scrollbar.Max.x, scrollbar.Max.y],
                    ],
                    scrollbar_active: native.ActiveId != 0
                        && native.ActiveId
                            == sys::igGetWindowScrollbarID(window_ptr, sys::ImGuiAxis_Y),
                };
            }
        }
        panic!("missing child {child} of {parent}");
    })
}

struct ScrollFixture {
    context: Context,
    editor: Editor,
    frame: EditorFrame,
    input: EditorInput,
    focus_other_window: bool,
    native_scroll_override: Option<f32>,
}

impl ScrollFixture {
    // Callers hold IMGUI_TEST_LOCK for the fixture's entire lifetime.
    fn new(lines: usize, columns: usize) -> Self {
        let context = context();
        let mut editor = Editor::new();
        editor.set_content(vec!["x".repeat(columns); lines].join("\n").as_bytes());
        editor.view.request_focus = true;
        let frame = EditorFrame::new(&mut editor.view_context());
        let mut fixture = Self {
            context,
            editor,
            frame,
            input: EditorInput::default(),
            focus_other_window: false,
            native_scroll_override: None,
        };
        for _ in 0..3 {
            fixture.render(1.0 / 60.0);
        }
        fixture
    }

    fn render(&mut self, delta: f32) -> ScrollSample {
        self.context
            .prepare_frame(FramePrepareOptions::new([640.0, 480.0], delta));
        let ui = self.context.frame();
        ui.window("Navigation scroll fixture")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_SAVED_SETTINGS)
            .build(|| {
                if let Some(y) = self.native_scroll_override.take() {
                    ui.with_bound_context(|| unsafe {
                        let native = &*sys::igGetCurrentContext();
                        for index in 0..native.Windows.Size {
                            let window = *native.Windows.Data.add(index as usize);
                            let name = std::ffi::CStr::from_ptr((*window).Name).to_string_lossy();
                            if name.contains("Navigation scroll fixture")
                                && name.contains("##editor")
                            {
                                sys::igSetScrollY_WindowPtr(window, y);
                            }
                        }
                    });
                }
                assert!(
                    self.frame
                        .run(ui, &mut self.editor.view_context(), &mut self.input)
                        .is_empty()
                );
            });
        if self.focus_other_window {
            ui.window("Structure navigation fixture")
                .position([645.0, 0.0], Condition::Always)
                .size([100.0; 2], Condition::Always)
                .build(|| {
                    ui.set_window_focus(None);
                    ui.text("Structure");
                });
        }
        let sample = sample(
            ui,
            "Navigation scroll fixture",
            "##editor",
            self.frame.layout.line_height,
        );
        // Sample the viewport actually rendered, not the deferred SetScroll target.
        assert_eq!(self.editor.view.scroll_position, sample.position);
        drop(self.context.render_legacy());
        sample
    }

    fn assert_stays_at(&mut self, expected: [f32; 2]) {
        for delta in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 120.0, 0.25] {
            assert_eq!(
                self.render(delta).position,
                expected,
                "no residual scroll motion"
            );
        }
    }

    fn settle_navigation(&mut self) -> ScrollSample {
        let mut sample = ScrollSample::default();
        for _ in 0..30 {
            sample = self.render(1.0 / 60.0);
        }
        sample
    }

    fn start_navigation(&mut self, row: i32, column: i32) -> ScrollSample {
        self.editor.view.request_cursor_center(row, column);
        let mut sample = ScrollSample::default();
        for _ in 0..4 {
            sample = self.render(1.0 / 60.0);
        }
        assert!(sample.position[1] > 0.0);
        assert!(sample.position[1] < sample.centered_y(row) - 1.0);
        sample
    }
}

/// An ordinary ImGui child is the wheel-behavior oracle. Its geometry matches
/// the text child, without invoking any editor scrolling or navigation code.
struct NativeChild {
    context: Context,
    geometry: ScrollSample,
}

impl NativeChild {
    fn new(geometry: ScrollSample) -> Self {
        let mut fixture = Self {
            context: context(),
            geometry,
        };
        for _ in 0..3 {
            fixture.render(1.0 / 60.0);
        }
        fixture
    }

    fn render(&mut self, delta: f32) -> ScrollSample {
        self.context
            .prepare_frame(FramePrepareOptions::new([640.0, 480.0], delta));
        let ui = self.context.frame();
        ui.window("Native scroll reference")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_SAVED_SETTINGS)
            .build(|| {
                let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
                let _rounding = ui.push_style_var(StyleVar::ChildRounding(0.0));
                let _border = ui.push_style_var(StyleVar::ChildBorderSize(1.0));
                let _scrollbar =
                    ui.push_style_var(StyleVar::ScrollbarSize(ui.current_font_size() * 0.6));
                ui.with_bound_context(|| unsafe {
                    sys::igSetNextWindowContentSize(self.geometry.content.into());
                });
                ui.child_window("##reference")
                    .size(self.geometry.viewport)
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR | WindowFlags::NO_NAV_INPUTS)
                    .build(ui, || {
                        ui.dummy([0.0; 2]);
                    });
            });
        let sample = sample(
            ui,
            "Native scroll reference",
            "##reference",
            self.geometry.line_height,
        );
        drop(self.context.render_legacy());
        sample
    }

    fn scroll_to(&mut self, position: [f32; 2]) {
        self.context.binding().with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            for index in 0..native.Windows.Size {
                let window = *native.Windows.Data.add(index as usize);
                let name = std::ffi::CStr::from_ptr((*window).Name).to_string_lossy();
                if name.contains("Native scroll reference") && name.contains("##reference") {
                    sys::igSetScrollX_WindowPtr(window, position[0]);
                    sys::igSetScrollY_WindowPtr(window, position[1]);
                }
            }
        });
        let applied = self.render(1.0 / 60.0).position;
        for axis in 0..2 {
            assert!(
                (applied[axis] - position[axis]).abs() < 1.0,
                "native SetScroll can truncate subpixel targets"
            );
        }
    }
}

fn prepare_wheel(context: &mut Context, shift: bool, macos_policy: bool) {
    context.binding().with_bound_context(|| unsafe {
        (*sys::igGetIO_Nil()).ConfigMacOSXBehaviors = macos_policy;
    });
    context.io_mut().add_mouse_pos_event([200.0, 150.0]);
    if shift {
        context.io_mut().add_key_event(Key::ModShift, true);
        context.io_mut().add_key_event(Key::LeftShift, true);
    }
}

fn assert_native_wheel_parity(events: &[[f32; 2]], delta: f32, shift: bool, macos_policy: bool) {
    let (geometry, actual) = {
        let mut editor = ScrollFixture::new(500, 300);
        prepare_wheel(&mut editor.context, shift, macos_policy);
        editor.render(delta);
        let geometry = editor.render(delta);
        let mut actual = Vec::new();
        for &wheel in events {
            editor.context.io_mut().add_mouse_wheel_event(wheel);
            actual.push(editor.render(delta).position);
        }
        for _ in 0..4 {
            actual.push(editor.render(delta).position);
        }
        (geometry, actual)
    };
    let mut reference = NativeChild::new(geometry);
    prepare_wheel(&mut reference.context, shift, macos_policy);
    reference.render(delta);
    let baseline = reference.render(delta);
    assert_eq!(
        baseline.maximum, geometry.maximum,
        "reference content/viewport geometry must match"
    );
    let mut expected = Vec::new();
    for &wheel in events {
        reference.context.io_mut().add_mouse_wheel_event(wheel);
        expected.push(reference.render(delta).position);
    }
    for _ in 0..4 {
        expected.push(reference.render(delta).position);
    }
    assert_eq!(
        actual, expected,
        "wheel distance and frame timing must match a native child"
    );
    if events
        .iter()
        .any(|wheel| wheel[0].abs().max(wheel[1].abs()) >= 0.04)
    {
        assert!(
            expected.iter().any(|&position| position != [0.0; 2]),
            "the native reference must actually scroll"
        );
    }
    let idle = &actual[actual.len() - 4..];
    assert!(
        idle.windows(2).all(|pair| pair[0] == pair[1]),
        "wheel must not coast after input"
    );
}

#[test]
fn vertical_and_horizontal_wheel_distance_and_timing_match_native_children() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for fps in [30, 60, 120] {
        let delta = 1.0 / fps as f32;
        assert_native_wheel_parity(&[[0.0, -1.0], [0.0, -2.0], [0.0, 1.0]], delta, false, false);
        assert_native_wheel_parity(&[[-1.0, 0.0], [-2.0, 0.0], [1.0, 0.0]], delta, false, false);
    }
}

#[test]
fn repeated_fractional_and_subpixel_wheel_input_match_native_children() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for wheel in [[0.0, -0.04], [-0.04, 0.0], [0.0, -0.005]] {
        assert_native_wheel_parity(&[wheel; 20], 1.0 / 120.0, false, false);
    }
}

#[test]
fn shift_wheel_uses_imgui_platform_policy() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for macos_policy in [false, true] {
        assert_native_wheel_parity(
            &[[0.0, -1.0], [0.0, -2.0], [0.0, 1.0]],
            1.0 / 60.0,
            true,
            macos_policy,
        );
    }
}

#[test]
fn centered_navigation_passes_through_intermediate_frames_then_stays_centered() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture
        .editor
        .commands()
        .set_cursor(350, 0, false, CursorReveal::Center);
    let requested = fixture.render(1.0 / 60.0);
    assert_eq!(requested.position, [0.0; 2]);
    let target = requested.centered_y(350);
    let mut previous = 0.0;
    let mut intermediate_frames = 0;
    for _ in 0..30 {
        let sample = fixture.render(1.0 / 60.0);
        assert!(sample.position[1] >= previous);
        assert!(sample.position[1] <= target + 1.0);
        intermediate_frames +=
            usize::from(sample.position[1] > 1.0 && sample.position[1] < target - 1.0);
        previous = sample.position[1];
    }
    assert!(intermediate_frames >= 3);
    assert!((previous - target).abs() <= 1.0);
    assert_eq!(
        (fixture.editor.view.row, fixture.editor.view.column),
        (350, 0)
    );
    assert!(!fixture.editor.view.ensure_cursor_visible.horizontal);
    assert!(!fixture.editor.view.ensure_cursor_visible.vertical);
    fixture.assert_stays_at([0.0, previous]);
}

#[test]
fn navigation_progress_depends_on_elapsed_time_across_frame_rates() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fractions = Vec::new();
    for fps in [30, 60, 120] {
        let mut fixture = ScrollFixture::new(500, 8);
        fixture.editor.view.request_cursor_center(350, 0);
        let delta = 1.0 / fps as f32;
        for _ in 0..=fps / 10 {
            fixture.render(delta);
        }
        // Align the rendered native positions at100ms after the request frame;
        // SetScroll applies the previous frame's requested position.
        let sample = fixture.render(delta);
        fractions.push(sample.position[1] / sample.centered_y(350));
        let applied = fixture.settle_navigation();
        assert!((applied.position[1] - applied.centered_y(350)).abs() <= 1.0);
    }
    let minimum = fractions.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = fractions.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(minimum > 0.5 && maximum < 0.99, "{fractions:?}");
    assert!(maximum - minimum < 0.02, "{fractions:?}");
}

#[test]
fn a_slow_opening_frame_does_not_skip_the_navigation_animation() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.editor.view.request_cursor_center(350, 0);
    assert_eq!(fixture.render(0.25).position, [0.0; 2]);
    assert_eq!(fixture.render(1.0 / 60.0).position, [0.0; 2]);
    let intermediate = fixture.render(1.0 / 60.0);
    assert!(intermediate.position[1] > 0.0);
    assert!(intermediate.position[1] < intermediate.centered_y(350) - 1.0);
    let applied = fixture.settle_navigation();
    assert!((applied.position[1] - applied.centered_y(350)).abs() <= 1.0);
}

#[test]
fn disabling_navigation_animations_finishes_the_active_jump_and_new_jumps_are_immediate() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.start_navigation(350, 0);
    fixture.frame.navigation_animations = false;
    fixture.render(1.0 / 60.0);
    let applied = fixture.render(1.0 / 60.0);
    assert!((applied.position[1] - applied.centered_y(350)).abs() <= 1.0);
    fixture.assert_stays_at(applied.position);
    fixture.editor.view.request_cursor_center(60, 0);
    fixture.render(1.0 / 60.0);
    let applied = fixture.render(1.0 / 60.0);
    assert!((applied.position[1] - applied.centered_y(60)).abs() <= 1.0);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn exact_navigation_spring_matches_fixed_and_irregular_frame_cadences() {
    let mut reference = SmoothScroll::new([10.0, 20.0], [250.0, 5000.0]);
    reference.advance(0.1);
    for frames in [3, 6, 12, 24] {
        let mut scroll = SmoothScroll::new([10.0, 20.0], [250.0, 5000.0]);
        for _ in 0..frames {
            scroll.advance(0.1 / frames as f32);
        }
        for axis in 0..2 {
            assert!((scroll.position[axis] - reference.position[axis]).abs() < 0.01);
            assert!((scroll.velocity[axis] - reference.velocity[axis]).abs() < 0.1);
        }
    }
    let mut irregular = SmoothScroll::new([10.0, 20.0], [250.0, 5000.0]);
    for delta in [0.011, 0.008, 0.031, 0.05] {
        irregular.advance(delta);
    }
    assert!((irregular.position[1] - reference.position[1]).abs() < 0.01);
    assert!((irregular.velocity[1] - reference.velocity[1]).abs() < 0.1);
}

#[test]
fn wheel_interrupts_active_navigation_and_then_matches_native_child_motion() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for wheel in [[0.0, 1.0], [-1.0, 0.0]] {
        let (geometry, queued_position, actual) = {
            let mut fixture = ScrollFixture::new(500, 300);
            prepare_wheel(&mut fixture.context, false, false);
            fixture.render(1.0 / 60.0);
            fixture.render(1.0 / 60.0);
            let geometry = fixture.start_navigation(400, 220);
            let queued_position = fixture.frame.smooth_scroll.as_ref().unwrap().last_requested;
            fixture.context.io_mut().add_mouse_wheel_event(wheel);
            let mut actual = vec![fixture.render(1.0 / 60.0).position];
            assert!(fixture.frame.smooth_scroll.is_none());
            fixture.context.io_mut().add_mouse_wheel_event(wheel);
            actual.push(fixture.render(1.0 / 60.0).position);
            for _ in 0..4 {
                actual.push(fixture.render(1.0 / 60.0).position);
            }
            fixture.assert_stays_at(*actual.last().unwrap());
            (geometry, queued_position, actual)
        };
        let mut reference = NativeChild::new(geometry);
        prepare_wheel(&mut reference.context, false, false);
        reference.render(1.0 / 60.0);
        reference.render(1.0 / 60.0);
        // The untouched axis can apply an already queued SetScroll at child
        // Begin. Native wheel owns its axis from the last displayed position;
        // the other axis must stay fixed from this first manual frame onward.
        let mut initial = geometry.position;
        let untouched_axis = if wheel[0] == 0.0 { 0 } else { 1 };
        initial[untouched_axis] = queued_position[untouched_axis];
        reference.scroll_to(initial);
        reference.context.io_mut().add_mouse_wheel_event(wheel);
        let mut expected = vec![reference.render(1.0 / 60.0).position];
        reference.context.io_mut().add_mouse_wheel_event(wheel);
        expected.push(reference.render(1.0 / 60.0).position);
        for _ in 0..4 {
            expected.push(reference.render(1.0 / 60.0).position);
        }
        assert_eq!(
            actual, expected,
            "manual wheel input must immediately follow native timing and distance"
        );
    }
}

#[test]
fn a_new_navigation_target_preserves_velocity_then_smoothly_reverses() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.start_navigation(400, 0);
    let scroll = fixture.frame.smooth_scroll.as_ref().unwrap();
    let velocity = scroll.velocity[1];
    let position = scroll.position[1];
    assert!(velocity > 0.0);
    fixture.editor.view.request_cursor_center(60, 0);
    fixture.render(0.00001);
    let retargeted = fixture.frame.smooth_scroll.as_ref().unwrap();
    assert!(retargeted.target[1] < position);
    assert!(retargeted.position[1] > position);
    assert!((retargeted.velocity[1] / velocity - 1.0).abs() < 0.01);
    let applied = fixture.settle_navigation();
    assert!((applied.position[1] - applied.centered_y(60)).abs() <= 1.0);
    assert_eq!(fixture.editor.view.row, 60);
    assert!(fixture.editor.view.pending_cursor_center.is_none());
    fixture.assert_stays_at(applied.position);
}

#[test]
fn navigation_animates_while_another_panel_has_focus() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.focus_other_window = true;
    fixture.render(1.0 / 60.0);
    fixture.start_navigation(350, 0);
    let applied = fixture.settle_navigation();
    assert!((applied.position[1] - applied.centered_y(350)).abs() <= 1.0);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn explicit_scroll_replaces_navigation_without_resuming_it() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 300);
    fixture.start_navigation(400, 0);
    fixture.editor.view.request_scroll(125.0, 275.0);
    fixture.render(1.0 / 60.0);
    let applied = fixture.render(1.0 / 60.0);
    assert_eq!(applied.position, [125.0, 275.0]);
    assert!(fixture.editor.view.requested_scroll.is_none());
    fixture.assert_stays_at(applied.position);

    // A host/minimap request also wins when a new reveal is pending in the
    // same frame, without a delayed snap back to the caret on the next frame.
    fixture.editor.view.request_cursor_center(250, 200);
    fixture.editor.view.request_scroll(25.0, 300.0);
    fixture.render(1.0 / 60.0);
    let applied = fixture.render(1.0 / 60.0);
    assert_eq!(applied.position, [25.0, 300.0]);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn native_host_scroll_override_remains_in_control_after_navigation() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.start_navigation(400, 0);
    fixture.native_scroll_override = Some(275.0);
    let applied = fixture.render(1.0 / 60.0);
    assert_eq!(applied.position, [0.0, 275.0]);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn dragging_the_native_scrollbar_takes_control_until_after_release() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture
        .context
        .io_mut()
        .set_config_scrollbar_scroll_by_page(false);
    let before = fixture.start_navigation(400, 0);
    let [min, max] = before.scrollbar_rect;
    let point = [(min[0] + max[0]) * 0.5, min[1] + (max[1] - min[1]) * 0.25];
    fixture.context.io_mut().add_mouse_pos_event(point);
    fixture.render(1.0 / 60.0);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
    let grabbed = fixture.render(1.0 / 60.0);
    assert!(grabbed.scrollbar_active);
    fixture
        .context
        .io_mut()
        .add_mouse_pos_event([point[0], point[1] + 70.0]);
    let dragged = fixture.render(1.0 / 60.0);
    assert!(dragged.scrollbar_active);
    assert!(dragged.position[1] > grabbed.position[1] + 100.0);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
    let released = fixture.render(1.0 / 60.0);
    assert!(!released.scrollbar_active);
    assert_eq!(released.position, dragged.position);
    fixture.assert_stays_at(released.position);
}

#[test]
fn caret_movement_reveals_the_cursor_on_the_next_native_frame() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture
        .editor
        .commands()
        .set_cursor(350, 0, false, CursorReveal::Ensure);
    // A caret can be off-screen after manual scrolling. Reveal only the new
    // location produced by the keyboard input below.
    fixture.editor.view.ensure_cursor_visible.horizontal = false;
    fixture.editor.view.ensure_cursor_visible.vertical = false;
    fixture.context.io_mut().add_key_event(Key::DownArrow, true);
    let requested = fixture.render(1.0 / 60.0);
    assert_eq!(requested.position, [0.0; 2]);
    fixture
        .context
        .io_mut()
        .add_key_event(Key::DownArrow, false);
    let applied = fixture.render(1.0 / 60.0);
    assert_eq!(fixture.editor.view.row, 351);
    let reveal_y = 353.0 * applied.line_height - applied.viewport[1];
    assert!((applied.position[1] - reveal_y).abs() <= 2.0);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn ordinary_caret_movement_cancels_active_navigation_and_reveals_immediately() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.start_navigation(350, 0);
    fixture.context.io_mut().add_key_event(Key::DownArrow, true);
    fixture.render(1.0 / 60.0);
    assert!(fixture.frame.smooth_scroll.is_none());
    fixture
        .context
        .io_mut()
        .add_key_event(Key::DownArrow, false);
    let applied = fixture.render(1.0 / 60.0);
    assert_eq!(fixture.editor.view.row, 351);
    let reveal_y = 353.0 * applied.line_height - applied.viewport[1];
    assert!((applied.position[1] - reveal_y).abs() <= 2.0);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn centered_navigation_clamps_at_document_end_and_short_documents_stay_put() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    {
        let mut fixture = ScrollFixture::new(160, 8);
        fixture.editor.view.request_cursor_center(159, 0);
        fixture.render(1.0 / 60.0);
        let applied = fixture.settle_navigation();
        assert!(applied.maximum[1] > 0.0);
        assert!((applied.position[1] - applied.maximum[1]).abs() <= 1.0);
        fixture.assert_stays_at(applied.position);
    }
    {
        let mut fixture = ScrollFixture::new(3, 8);
        fixture.editor.view.request_cursor_center(999, 0);
        fixture.render(1.0 / 60.0);
        let applied = fixture.render(1.0 / 60.0);
        assert_eq!(fixture.editor.view.row, 2);
        assert_eq!(applied.position, [0.0; 2]);
        assert_eq!(applied.maximum[1], 0.0);
    }
}

#[test]
fn centered_navigation_smoothly_reveals_both_axes_of_a_long_line() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(200, 240);
    fixture
        .editor
        .commands()
        .set_cursor(130, 220, false, CursorReveal::Center);
    fixture.render(1.0 / 60.0);
    fixture.render(1.0 / 60.0);
    let intermediate = fixture.render(1.0 / 60.0);
    assert!(intermediate.position[0] > 0.0);
    assert!(intermediate.position[1] > 0.0);
    let applied = fixture.settle_navigation();
    assert!(applied.position[0] > 0.0);
    assert!(applied.position[0] > intermediate.position[0]);
    assert!((applied.position[1] - applied.centered_y(130)).abs() <= 1.0);
    assert!(!fixture.editor.view.ensure_cursor_visible.horizontal);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn ordinary_caret_reveal_scrolls_long_lines_without_residual_motion() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(200, 240);
    fixture
        .editor
        .commands()
        .set_cursor(0, 220, false, CursorReveal::Ensure);
    let requested = fixture.render(1.0 / 60.0);
    assert_eq!(requested.position, [0.0; 2]);
    let applied = fixture.render(1.0 / 60.0);
    assert!(applied.position[0] > 0.0);
    assert_eq!(applied.position[1], 0.0);
    assert!(!fixture.editor.view.ensure_cursor_visible.horizontal);
    fixture.assert_stays_at(applied.position);
}

#[test]
fn native_wheel_input_takes_precedence_over_pending_navigation() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    prepare_wheel(&mut fixture.context, false, false);
    fixture.render(1.0 / 60.0);
    fixture.render(1.0 / 60.0);
    fixture.editor.view.request_cursor_center(350, 0);
    fixture.context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
    let scrolled = fixture.render(1.0 / 60.0);
    assert!(scrolled.position[1] > 0.0);
    assert!(scrolled.position[1] < scrolled.centered_y(350) * 0.1);
    assert_eq!(fixture.editor.view.row, 350);
    fixture.assert_stays_at(scrolled.position);
}

#[test]
fn minimap_click_and_drag_apply_on_the_next_native_frame_and_stop_on_release() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = ScrollFixture::new(500, 8);
    fixture.frame.minimap_enabled = true;
    for _ in 0..3 {
        fixture.render(1.0 / 60.0);
    }
    assert!(fixture.frame.layout.minimap_width > 0.0);
    fixture.start_navigation(400, 0);
    let min = fixture.frame.layout.minimap_min;
    let max = fixture.frame.layout.minimap_max;
    let point = [(min[0] + max[0]) * 0.5, min[1] + (max[1] - min[1]) * 0.5];
    fixture.context.io_mut().add_mouse_pos_event(point);
    fixture.render(1.0 / 60.0);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
    let pressed = fixture.render(1.0 / 60.0);
    assert!(
        fixture.frame.smooth_scroll.is_none(),
        "minimap click cancels active navigation"
    );
    let clicked = fixture.render(1.0 / 60.0);
    assert!(clicked.position[1] > 100.0);
    assert_ne!(clicked.position, pressed.position);
    fixture
        .context
        .io_mut()
        .add_mouse_pos_event([point[0], point[1] + 30.0]);
    let moving = fixture.render(1.0 / 60.0);
    assert_eq!(moving.position, clicked.position);
    let dragged = fixture.render(1.0 / 60.0);
    assert!(dragged.position[1] > clicked.position[1]);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
    let released = fixture.render(1.0 / 60.0);
    assert_eq!(released.position, dragged.position);
    fixture.assert_stays_at(released.position);
}
