//! Translated from ned editor/views/view_layout.h and GUI measurement helpers.
use dear_imgui_rs::Ui;

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewLayout {
    pub pane_pos: [f32; 2],
    pub pane_size: [f32; 2],
    pub size: [f32; 2],
    pub line_height: f32,
    pub total_height: f32,
    pub editor_top_margin: f32,
    pub text_left_margin: f32,
    pub text_pos: [f32; 2],
    pub rainbow_mode: bool,
    pub minimap_width: f32,
    pub minimap_min: [f32; 2],
    pub minimap_max: [f32; 2],
}

/// Raw byte spans used by upstream's LineColumnX and ColumnAtX helpers.
/// Their decoder also measures stray continuation bytes, unlike TextView.
pub fn glyph_spans(line: &[u8]) -> impl Iterator<Item = (usize, usize, &[u8])> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset >= line.len() {
            return None;
        }
        let start = offset;
        offset += 1;
        if line[start] != b'\t' && line[start] & 0x80 != 0 {
            while offset < line.len() && line[offset] & 0xc0 == 0x80 {
                offset += 1;
            }
        }
        Some((start, offset, &line[start..offset]))
    })
}

pub fn glyph_advance(ui: &Ui, glyph: &str) -> f32 {
    glyph_advance_bytes(ui, glyph.as_bytes())
}

pub fn glyph_advance_bytes(ui: &Ui, bytes: &[u8]) -> f32 {
    ui.with_bound_context(|| unsafe {
        // SAFETY: the current Ui owns the active font; CalcTextSizeA reads the
        // bounded borrowed range synchronously, including invalid UTF-8.
        dear_imgui_rs::sys::ImFont_CalcTextSizeA(
            dear_imgui_rs::sys::igGetFont(),
            dear_imgui_rs::sys::igGetFontSize(),
            f32::MAX,
            0.0,
            bytes.as_ptr().cast(),
            bytes.as_ptr().add(bytes.len()).cast(),
            std::ptr::null_mut(),
        )
        .x
    })
}

pub fn measure_glyph(ui: &Ui, glyph: &str, x: f32, origin: f32) -> f32 {
    measure_glyph_bytes(ui, glyph.as_bytes(), x, origin)
}

fn measure_glyph_bytes(ui: &Ui, glyph: &[u8], x: f32, origin: f32) -> f32 {
    if glyph.first() == Some(&b'\t') {
        let space = glyph_advance(ui, " ");
        let visual_column = ((x - origin) / space) as i32;
        (((visual_column / 4) + 1) * 4 - visual_column) as f32 * space
    } else {
        glyph_advance_bytes(ui, glyph)
    }
}

pub fn line_column_x(ui: &Ui, line: &[u8], column: i32, origin: f32) -> f32 {
    let mut x = origin;
    for (start, _, glyph) in glyph_spans(line) {
        if start >= column.max(0) as usize {
            break;
        }
        x += measure_glyph_bytes(ui, glyph, x, origin);
    }
    x
}

pub fn column_at_x(ui: &Ui, line: &[u8], click_x: f32) -> i32 {
    let mut best = 0;
    let mut best_distance = click_x.abs();
    let mut x = 0.0;
    for (_, next, glyph) in glyph_spans(line) {
        x += measure_glyph_bytes(ui, glyph, x, 0.0);
        let distance = (click_x - x).abs();
        if distance < best_distance {
            best = next as i32;
            best_distance = distance;
        }
        if x >= click_x {
            break;
        }
    }
    best
}

pub fn rainbow_color(time: f32) -> [f32; 4] {
    let t = time * 2.0;
    [
        t.sin() * 0.5 + 0.5,
        (t + 2.0944).sin() * 0.5 + 0.5,
        (t + 4.1888).sin() * 0.5 + 0.5,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    #[test]
    fn raw_measurement_preserves_decoder_tabs_controls_and_byte_columns() {
        let spans: Vec<_> = glyph_spans(b"A\x80B").collect();
        assert_eq!(
            spans,
            vec![(0, 1, &b"A"[..]), (1, 2, &b"\x80"[..]), (2, 3, &b"B"[..])]
        );
        assert_eq!(
            glyph_spans(b"\x80\x81A").next(),
            Some((0, 2, &b"\x80\x81"[..]))
        );
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Measurements")
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                // A bounded NUL range follows the native fallback glyph path,
                // rather than Rust string/C-string truncation.
                assert!(glyph_advance_bytes(ui, b"\0") > 0.0);
                let a = glyph_advance_bytes(ui, b"A");
                let invalid = glyph_advance_bytes(ui, b"\x80");
                assert!(invalid > 0.0);
                assert_eq!(line_column_x(ui, b"A\x80B", 2, 0.0), a + invalid);
                assert_eq!(column_at_x(ui, b"A\x80B", a + invalid), 2);
                let space = glyph_advance(ui, " ");
                assert_eq!(line_column_x(ui, b" \tX", 2, 0.0), space * 4.0);
                assert_eq!(
                    line_column_x(ui, "🙂X".as_bytes(), 1, 0.0),
                    glyph_advance(ui, "🙂")
                );
            });
        drop(context.render_legacy());
    }
}
