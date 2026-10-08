//! Native Workbench coverage for content fading and host isolation.
use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;

const TEXT_RGB: u32 = 0x0099_6633;
const HOST_COLOR: [f32; 4] = [0.8, 0.2, 0.4, 1.0];
const HOST_RGB: u32 = 0x0066_33cc;

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
    let mut theme =
        bed_settings::read_json(&settings.resources_root.join("resources/themes/tokyo.json"))
            .unwrap();
    theme["syntax"]["text"] = json!("#336699");
    bed_settings::write_json(
        &settings.config_dir.join("themes/animation-test.json"),
        &theme,
    )
    .unwrap();
    settings.select_theme("themes/animation-test.json").unwrap();
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

fn host_window(ui: &Ui, name: &str) {
    ui.window(name)
        .position([900.0, 600.0], Condition::Always)
        .size([180.0, 140.0], Condition::Always)
        .flags(WindowFlags::NO_FOCUS_ON_APPEARING | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS)
        .build(|| {
            let origin = ui.cursor_screen_pos();
            ui.get_window_draw_list()
                .add_rect(origin, [origin[0] + 20.0, origin[1] + 20.0], HOST_COLOR)
                .filled(true)
                .build();
        });
}

fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    let ui = context.frame();
    host_window(ui, "Host before Bed");
    workbench.render(ui).unwrap();
    host_window(ui, "Host after Bed");
    drop(context.render_legacy());
}

fn vertex_alphas(context: &Context, name_fragment: &str, rgb: u32) -> Vec<u8> {
    context.binding().with_bound_context(|| unsafe {
        // Native pointers are read only while the context is bound and after
        // RenderPre has finished applying the visual effect.
        let native = &*sys::igGetCurrentContext();
        let mut alphas = Vec::new();
        for index in 0..native.Windows.Size as usize {
            let window = &**native.Windows.Data.add(index);
            if !window.Active || window.Hidden || window.DrawList.is_null() {
                continue;
            }
            if !CStr::from_ptr(window.Name)
                .to_string_lossy()
                .contains(name_fragment)
            {
                continue;
            }
            let vertices = &(*window.DrawList).VtxBuffer;
            for vertex in 0..vertices.Size as usize {
                let color = (*vertices.Data.add(vertex)).col;
                if color & 0x00ff_ffff == rgb {
                    alphas.push((color >> 24) as u8);
                }
            }
        }
        alphas
    })
}

fn text_alphas(context: &Context, view: ViewId) -> Vec<u8> {
    vertex_alphas(context, &format!("##bed_view_{}", view.0), TEXT_RGB)
}

fn assert_opaque(alphas: &[u8]) {
    assert!(!alphas.is_empty(), "expected rendered vertices");
    assert!(alphas.iter().all(|&alpha| alpha == 255), "{alphas:?}");
}

#[test]
fn custom_editor_text_fades_while_host_windows_remain_opaque() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let file = dir.write("fade.txt", b"Custom editor text fades too.");
    let (mut context, mut workbench) = workspace(&dir);
    workbench.open_or_focus(&file).unwrap();
    let view = workbench.active_view().unwrap();
    let mut faded = false;
    for _ in 0..25 {
        frame(&mut context, &mut workbench);
        faded |= text_alphas(&context, view)
            .iter()
            .any(|&alpha| alpha > 0 && alpha < 255);
        assert_opaque(&vertex_alphas(&context, "Host before Bed", HOST_RGB));
        assert_opaque(&vertex_alphas(&context, "Host after Bed", HOST_RGB));
    }
    assert!(faded, "newly visible custom editor text must fade in");
    assert_opaque(&text_alphas(&context, view));
}

#[test]
fn revealing_a_docked_document_restarts_its_content_fade() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let first = dir.write("first.txt", b"First document");
    let second = dir.write("second.txt", b"Second document");
    let (mut context, mut workbench) = workspace(&dir);
    workbench.open_or_focus(&first).unwrap();
    let first_view = workbench.active_view().unwrap();
    for _ in 0..25 {
        frame(&mut context, &mut workbench);
    }
    assert_opaque(&text_alphas(&context, first_view));
    workbench.open_or_focus(&second).unwrap();
    let second_view = workbench.active_view().unwrap();
    for _ in 0..25 {
        frame(&mut context, &mut workbench);
    }
    assert_opaque(&text_alphas(&context, second_view));
    assert!(text_alphas(&context, first_view).is_empty());
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.view_id() == Some(first_view))
        .unwrap();
    assert!(workbench.switch_to_tab(index));
    let mut faded = false;
    for _ in 0..25 {
        frame(&mut context, &mut workbench);
        faded |= text_alphas(&context, first_view)
            .iter()
            .any(|&alpha| alpha > 0 && alpha < 255);
    }
    assert!(faded, "revealing a hidden dock tab must fade its content");
    assert_opaque(&text_alphas(&context, first_view));
}

#[test]
fn disabling_animations_settles_active_motion_and_reenable_does_not_replay_it() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let file = dir.write("toggle.txt", b"Motion preference applies immediately.");
    let (mut context, mut workbench) = workspace(&dir);
    workbench.open_or_focus(&file).unwrap();
    let view = workbench.active_view().unwrap();
    let mut faded = false;
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
        if text_alphas(&context, view)
            .iter()
            .any(|&alpha| alpha > 0 && alpha < 255)
        {
            faded = true;
            break;
        }
    }
    assert!(faded, "the setting must be disabled during active motion");
    workbench.settings.settings["ui_animations"] = json!(false);
    frame(&mut context, &mut workbench);
    assert_opaque(&text_alphas(&context, view));
    workbench.settings.settings["ui_animations"] = json!(true);
    frame(&mut context, &mut workbench);
    assert_opaque(&text_alphas(&context, view));
}
