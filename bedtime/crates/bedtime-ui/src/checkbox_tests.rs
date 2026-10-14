use dear_imgui_rs::Context;
use std::path::PathBuf;

#[test]
fn smaller_checkbox_keeps_its_full_click_target_and_label_layout() {
    use dear_imgui_rs::{Condition, FramePrepareOptions, MouseButton, StyleColor, sys};
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let background = [0.2, 0.4, 0.6, 1.0];
    context
        .style_mut()
        .set_color(StyleColor::FrameBg, background);
    context.style_mut().set_frame_rounding(0.0);
    context.style_mut().set_frame_border_size(0.0);
    let packed = unsafe { sys::igColorConvertFloat4ToU32(background.into()) };
    let mut checked = false;
    let draw = |context: &mut Context, checked: &mut bool| {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut row = ([0.0; 2], [0.0; 2]);
        let mut height = 0.0;
        ui.window("Checkbox size")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                height = ui.frame_height();
                ui.checkbox("Option", checked);
                row = ui.item_rect();
            });
        let square = ui.with_bound_context(|| unsafe {
            let window = &*sys::igFindWindowByName(c"Checkbox size".as_ptr());
            let buffer = &(*window.DrawList).VtxBuffer;
            let vertices = std::slice::from_raw_parts(buffer.Data, buffer.Size as usize);
            let mut min = [f32::INFINITY; 2];
            let mut max = [f32::NEG_INFINITY; 2];
            for vertex in vertices.iter().filter(|v| v.col == packed) {
                min[0] = min[0].min(vertex.pos.x);
                min[1] = min[1].min(vertex.pos.y);
                max[0] = max[0].max(vertex.pos.x);
                max[1] = max[1].max(vertex.pos.y);
            }
            (min, max)
        });
        drop(context.render_legacy());
        (row, square, height)
    };
    draw(&mut context, &mut checked);
    let ((min, max), (square_min, square_max), height) = draw(&mut context, &mut checked);
    for axis in 0..2 {
        assert!((square_max[axis] - square_min[axis] - height * 0.7).abs() < 0.01);
        assert!((square_min[axis] - min[axis] - height * 0.15).abs() < 0.01);
    }
    assert!((max[1] - min[1] - height).abs() < 0.01);
    // Click inside the original square, outside the smaller painted square.
    context
        .io_mut()
        .add_mouse_pos_event([min[0] + 1.0, min[1] + height * 0.5]);
    draw(&mut context, &mut checked);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    draw(&mut context, &mut checked);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    let (row, _, _) = draw(&mut context, &mut checked);
    assert!(checked);
    assert_eq!(row, (min, max));
}
