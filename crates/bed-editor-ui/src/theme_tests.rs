use crate::views::{caret_view::CaretView, gutter_view::GutterView, view_layout::ViewLayout};
use bed_document_session::editor::Editor;
use bed_editing::util::color::{blend, contrast_ratio};
use dear_imgui_rs::{Condition, Context, FramePrepareOptions, StyleColor, sys};

fn colors(ui: &dear_imgui_rs::Ui, draw: impl FnOnce()) -> Vec<[f32; 4]> {
    let start = ui.with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
    draw();
    ui.with_bound_context(|| unsafe {
        let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
        (start..vertices.Size)
            .map(|index| {
                let color = (*vertices.Data.add(index as usize)).col;
                std::array::from_fn(|channel| ((color >> (channel * 8)) & 255) as f32 / 255.0)
            })
            .collect()
    })
}

#[test]
fn gutter_and_caret_follow_light_and_dark_host_palettes_with_readable_contrast() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut editor = Editor::new();
    editor.set_content(b"first\nsecond\nthird");
    editor.view.block_input = false;
    editor.view.cursor_blink_time = std::f32::consts::FRAC_PI_8;
    for (background, text) in [
        ([0.96, 0.94, 0.90, 1.0], [0.08, 0.10, 0.12, 1.0]),
        ([0.04, 0.05, 0.08, 1.0], [0.90, 0.91, 0.93, 1.0]),
    ] {
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, background);
        context.style_mut().set_color(StyleColor::Text, text);
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Theme paint fixture")
            .position([0.0, 0.0], Condition::Always)
            .size([500.0, 350.0], Condition::Always)
            .build(|| {
                let layout = ViewLayout {
                    text_pos: [60.0, 60.0],
                    size: [500.0, 350.0],
                    line_height: ui.text_line_height(),
                    ..Default::default()
                };
                let gutter = colors(ui, || {
                    GutterView::draw(
                        ui,
                        &ui.get_window_draw_list(),
                        &editor,
                        &layout,
                        [20.0, 60.0],
                        35.0,
                    )
                });
                assert!(!gutter.is_empty());
                for color in gutter {
                    let rendered = blend(color, background, color[3]);
                    assert!(
                        contrast_ratio(rendered, background) >= 4.45,
                        "line number {color:?} on {background:?}"
                    );
                }
                let caret = colors(ui, || {
                    CaretView::draw(ui, &editor.state, &editor.view, &layout)
                });
                assert!(!caret.is_empty());
                for color in caret {
                    assert!(contrast_ratio(blend(color, background, color[3]), background) >= 4.45);
                    for channel in 0..3 {
                        assert!((color[channel] - text[channel]).abs() < 0.005);
                    }
                }
            });
        drop(context.render_legacy());
    }
}
