//! Exercise navigation through the Workbench and native ImGui scroll frames.
use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;

fn workspace(dir: &TempDir) -> (Context, Workbench) {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    for key in [
        "terminal_visible",
        "sidebar_visible",
        "treesitter",
        "git_changed_lines",
        "minimap",
        "rainbow",
    ] {
        settings.settings[key] = json!(false);
    }
    settings.terminal_visible = false;
    let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Floating)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    (context, workbench)
}

fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    drop(context.render_legacy());
}

fn scroll(workbench: &Workbench, view: ViewId) -> f32 {
    workbench
        .session
        .view_snapshot(view)
        .unwrap()
        .scroll_position[1]
}

fn centered_scroll(context: &Context, workbench: &Workbench, view: ViewId, row: i32) -> f32 {
    let line_height = workbench
        .tabs
        .iter()
        .find_map(|tab| {
            tab.panel
                .editor()
                .filter(|editor| editor.id() == view)
                .map(|editor| editor.presentation().layout.line_height)
        })
        .unwrap();
    context.binding().with_bound_context(|| unsafe {
        // Inspect native child geometry only while its owning context is bound.
        let native = &*sys::igGetCurrentContext();
        let fragment = format!("##bed_view_{}", view.0);
        for index in 0..native.Windows.Size as usize {
            let window = &**native.Windows.Data.add(index);
            let name = CStr::from_ptr(window.Name).to_string_lossy();
            if name.contains(&fragment) && name.contains("##editor") {
                return (row as f32 * line_height - (window.Size.y - line_height) * 0.5)
                    .clamp(0.0, window.ScrollMax.y.max(0.0));
            }
        }
        panic!("missing editor child window for {view:?}");
    })
}

fn assert_smooth_center(
    context: &mut Context,
    workbench: &mut Workbench,
    view: ViewId,
    row: i32,
    baseline: f32,
) {
    let samples: Vec<_> = (0..24)
        .map(|_| {
            frame(context, workbench);
            scroll(workbench, view)
        })
        .collect();
    let target = centered_scroll(context, workbench, view, row);
    assert!(
        (target - baseline).abs() > 100.0,
        "fixture must scroll a substantial distance"
    );
    let range = baseline.min(target) + 1.0..baseline.max(target) - 1.0;
    assert!(
        samples
            .iter()
            .filter(|&&position| range.contains(&position))
            .count()
            >= 2,
        "explicit navigation must pass through intermediate positions: {samples:?}, target={target}"
    );
    let direction = (target - baseline).signum();
    assert!(
        samples
            .windows(2)
            .all(|pair| direction * (pair[1] - pair[0]) >= -0.1),
        "scroll must approach its destination without reversing: {samples:?}"
    );
    assert!(
        (samples.last().unwrap() - target).abs() < 1.0,
        "navigation must finish centered: {samples:?}, target={target}"
    );
}

fn assert_immediate_center(
    context: &mut Context,
    workbench: &mut Workbench,
    view: ViewId,
    row: i32,
    baseline: f32,
) {
    // Navigation queues SetScrollY while drawing. ImGui applies that target
    // when the child begins on the following frame, just like other panels.
    frame(context, workbench);
    frame(context, workbench);
    let target = centered_scroll(context, workbench, view, row);
    assert!(
        (target - baseline).abs() > 100.0,
        "fixture must scroll a substantial distance"
    );
    let settled = scroll(workbench, view);
    assert!(
        (settled - target).abs() < 1.0,
        "navigation must settle on the next native frame: position={settled}, target={target}"
    );
    frame(context, workbench);
    assert!(
        (scroll(workbench, view) - settled).abs() < 0.1,
        "settled navigation must not continue moving"
    );
}

#[test]
fn file_location_navigation_animates_new_files_and_existing_tabs() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("long.txt", "line\n".repeat(500).as_bytes());
    let other = dir.write("other.txt", b"Another document\n");
    let (mut context, mut workbench) = workspace(&dir);
    workbench
        .navigate_file(path.to_str().unwrap(), 250, 2, false)
        .unwrap();
    let view = workbench.active_view().unwrap();
    let caret = workbench.session.view_snapshot(view).unwrap();
    assert_eq!((caret.row, caret.column), (250, 2));
    assert_smooth_center(&mut context, &mut workbench, view, 250, 0.0);

    workbench.open_or_focus(&other).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let baseline = scroll(&workbench, view);
    workbench
        .navigate_file(path.to_str().unwrap(), 400, 1, false)
        .unwrap();
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.editor().is_some())
            .count(),
        2
    );
    assert_smooth_center(&mut context, &mut workbench, view, 400, baseline);
}

#[test]
fn structure_navigation_animates_only_the_last_shared_document_view() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let source = format!(
        "{}fn destination() {{}}\n{}",
        "// line\n".repeat(250),
        "// line\n".repeat(250)
    );
    let path = dir.write("long.rs", source.as_bytes());
    let (mut context, mut workbench) = workspace(&dir);
    workbench.set_project(dir.root()).unwrap();
    workbench.open_or_focus(&path).unwrap();
    let first = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::Structure).unwrap();
    frame(&mut context, &mut workbench);
    workbench.refresh_plugins().unwrap();
    let target = workbench.modules.frame.context().active().unwrap().clone();
    let jump = bed_plugin_structure::presentation::StructureJump {
        key: bed_highlight::outline::OutlineKey {
            document: target.id,
            generation: target.revision.0,
            revision: target.revision.1,
            path: target.path,
            language_id: target.language_id,
        },
        offset: source.find("destination").unwrap(),
    };
    let request = bed_plugin_structure::StructurePlugin::navigation_request(
        jump,
        &workbench.modules.frame.context(),
    )
    .unwrap();
    workbench.modules.requests.push(request);
    workbench.process_plugin_requests().unwrap();
    assert_eq!(workbench.active_view(), Some(second));
    assert_eq!(workbench.session.view_snapshot(second).unwrap().row, 250);
    assert_smooth_center(&mut context, &mut workbench, second, 250, 0.0);
    let first_caret = workbench.session.view_snapshot(first).unwrap();
    assert_eq!((first_caret.row, first_caret.column), (0, 0));
    assert_eq!(first_caret.scroll_position[1], 0.0);
}

#[test]
fn ui_animations_toggle_controls_explicit_navigation_motion() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("long.txt", "line\n".repeat(500).as_bytes());
    let (mut context, mut workbench) = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    for (animations, row) in [(false, 200), (true, 350), (false, 100)] {
        let baseline = scroll(&workbench, view);
        workbench.settings.settings["ui_animations"] = json!(animations);
        workbench
            .navigate_file(path.to_str().unwrap(), row, 0, false)
            .unwrap();
        if animations {
            assert_smooth_center(&mut context, &mut workbench, view, row, baseline);
        } else {
            assert_immediate_center(&mut context, &mut workbench, view, row, baseline);
        }
    }
}
