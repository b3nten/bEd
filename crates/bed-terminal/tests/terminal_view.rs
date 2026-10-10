//! Presentation regressions using Bed-authored protocol scenarios.
use bed_terminal::{
    terminal::{SelectionSnap, Terminal, TerminalImage, TerminalImagePlacement},
    terminal_font::TerminalFonts,
    terminal_renderer::TerminalRenderer,
};
use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
static IMGUI_LOCK: Mutex<()> = Mutex::new(());
fn context_and_fonts() -> (Context, TerminalFonts) {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut fonts = TerminalFonts::default();
    fonts.reload(&mut context, root, 16.0).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    (context, fonts)
}
fn draw(
    context: &mut Context,
    renderer: &TerminalRenderer,
    snapshot: &bed_terminal::terminal::TerminalSnapshot,
    cursor_on: bool,
) -> usize {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    let ui = context.frame();
    ui.window("presentation")
        .position([0.0, 0.0], Condition::Always)
        .size([1200.0, 800.0], Condition::Always)
        .build(|| {
            renderer.draw(
                ui,
                snapshot,
                [10.0, 30.0],
                [1190.0, 790.0],
                true,
                cursor_on,
                false,
                true,
            );
        });
    context.render_legacy().draw_data().total_vtx_count()
}
#[test]
fn protocol_scenarios_produce_textured_glyph_geometry() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    for (name, input) in [
        ("plain", "Bed protocol rendering\r\nsecond line"),
        ("styles", "\x1b[1mB\x1b[3mI\x1b[4mU\x1b[0mnormal"),
        ("ansi", "\x1b[31mred\x1b[94mblue\x1b[0m"),
        (
            "rgb",
            "\x1b[38;2;23;91;171mforeground\x1b[48;2;71;19;33mbackground",
        ),
        ("palette", "\x1b]4;2;rgb:23/91/ab\x07\x1b[32mcustom"),
        ("unicode", "文字 e\u{301} 👩\u{200d}💻"),
        ("block cursor", "\x1b[2 qblock"),
        ("underline cursor", "\x1b[4 qunderline"),
        ("beam cursor", "\x1b[6 qbeam"),
    ] {
        let mut term = Terminal::new(80, 24);
        for fragment in input.as_bytes().chunks(5) {
            term.feed(fragment);
        }
        let snapshot = term.snapshot();
        let mut renderer = TerminalRenderer::default();
        renderer
            .prepare(&mut context, &snapshot, &fonts, 1.0)
            .unwrap();
        assert!(
            renderer.stats().rasterized_glyphs > 0,
            "{name} must prepare actual font glyphs"
        );
        assert!(
            draw(&mut context, &renderer, &snapshot, true) > 0,
            "{name} must emit geometry"
        );
        renderer.clear(&mut context);
    }
}
#[test]
fn selection_and_cursor_blink_reuse_shaped_rows_and_uploads() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    let mut term = Terminal::new(40, 8);
    term.feed("e\u{301} accents \x1b[3mitalic\x1b[0m\r\nplain text".as_bytes());
    let mut renderer = TerminalRenderer::default();
    let snapshot = term.snapshot();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    let prepared = renderer.stats();
    draw(&mut context, &renderer, &snapshot, false);
    draw(&mut context, &renderer, &snapshot, true);
    term.select_start(0, 0, SelectionSnap::None);
    term.select_extend(6, 0, false, true);
    let selected = term.snapshot();
    assert!(
        selected
            .selection_text
            .as_ref()
            .unwrap()
            .contains("e\u{301}")
    );
    renderer
        .prepare(&mut context, &selected, &fonts, 1.0)
        .unwrap();
    draw(&mut context, &renderer, &selected, true);
    let after = renderer.stats();
    assert_eq!(after.prepared_rows, prepared.prepared_rows);
    assert_eq!(after.rasterized_glyphs, prepared.rasterized_glyphs);
    assert_eq!(after.glyph_upload_bytes, prepared.glyph_upload_bytes);
    term.feed(b"\x1b[2;1Hchanged");
    renderer
        .prepare(&mut context, &term.snapshot(), &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().prepared_rows, after.prepared_rows + 1);
    renderer.clear(&mut context);
}
#[test]
fn dpi_and_resource_reset_rebuild_textures_without_terminal_reset() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    let mut term = Terminal::new(20, 4);
    term.feed(b"retain this text");
    let snapshot = term.snapshot();
    let mut renderer = TerminalRenderer::default();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    let count = renderer.stats().rasterized_glyphs;
    renderer
        .prepare(&mut context, &snapshot, &fonts, 2.0)
        .unwrap();
    assert!(renderer.stats().rasterized_glyphs > count);
    let count = renderer.stats().rasterized_glyphs;
    renderer.invalidate_textures();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 2.0)
        .unwrap();
    assert!(renderer.stats().rasterized_glyphs > count);
    assert_eq!(snapshot.lines[0].cells[0].text.as_ref(), "r");
    assert!(renderer.stats().glyph_resident_bytes <= 32 * 1024 * 1024);
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.25)
        .unwrap();
    for stride in renderer.metrics() {
        assert!(
            (stride * renderer.density() - (stride * renderer.density()).round()).abs() < 0.0001,
            "fractional DPI uses the integer physical stride reported by the PTY"
        );
    }
    renderer.clear(&mut context);
}
#[test]
fn images_upload_progressively_and_retire_when_their_placement_disappears() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    let mut term = Terminal::new(20, 4);
    let mut snapshot = (*term.snapshot()).clone();
    let image = Arc::new(TerminalImage {
        id: 123,
        revision: 1,
        width: 1536,
        height: 1536,
        rgba: vec![255; 1536 * 1536 * 4].into(),
    });
    snapshot.images.push(TerminalImagePlacement {
        image: image.clone(),
        x: 0.0,
        y: 0.0,
        width: 80.0,
        height: 80.0,
        source_rect: [0.0, 0.0, 1.0, 1.0],
        z_index: 0,
    });
    let mut renderer = TerminalRenderer::default();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert!(renderer.has_pending_uploads());
    assert!(renderer.stats().image_upload_bytes <= 8 * 1024 * 1024);
    draw(&mut context, &renderer, &snapshot, true);
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert!(!renderer.has_pending_uploads());
    assert_eq!(renderer.stats().image_upload_bytes, image.rgba.len() as u64);
    let uploaded = renderer.stats().image_upload_bytes;
    snapshot.images[0].y = 20.0;
    snapshot.images[0].source_rect = [0.25, 0.25, 0.75, 0.75];
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(
        renderer.stats().image_upload_bytes,
        uploaded,
        "moving/cropping an image reuses its textures"
    );
    snapshot.images.clear();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().image_resident_bytes, 0);
    renderer.clear(&mut context);
}

#[test]
fn image_budget_rejection_keeps_text_and_allows_recovery_after_retirement() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    let mut term = Terminal::new(20, 4);
    term.feed(b"text survives");
    let mut snapshot = (*term.snapshot()).clone();
    // Share this source between two distinct images. Either image fits the
    // per-image cap; together they exceed the presentation's residency cap.
    let rgba: Arc<[u8]> = vec![255; 2560 * 5120 * 4].into();
    for id in [1, 2] {
        snapshot.images.push(TerminalImagePlacement {
            image: Arc::new(TerminalImage {
                id,
                revision: 1,
                width: 2560,
                height: 5120,
                rgba: rgba.clone(),
            }),
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 80.0,
            source_rect: [0.0, 0.0, 1.0, 1.0],
            z_index: 0,
        });
    }
    let mut renderer = TerminalRenderer::default();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().rejected_images, 1);
    assert!(renderer.warning().unwrap().contains("budget"));
    assert!(renderer.stats().rasterized_glyphs > 0);
    assert_eq!(renderer.stats().image_resident_bytes, rgba.len());
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(
        renderer.stats().rejected_images,
        1,
        "the same image is reported once"
    );
    snapshot.images.remove(0);
    let uploaded = renderer.stats().image_upload_bytes;
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().image_resident_bytes, rgba.len());
    assert!(
        renderer.stats().image_upload_bytes > uploaded,
        "retiring the first image frees capacity for the second"
    );
    assert!(renderer.warning().is_none());
    renderer.clear(&mut context);
}

#[test]
fn tiny_images_obey_the_texture_count_budget_and_recover_after_retirement() {
    let _guard = IMGUI_LOCK.lock().unwrap();
    let (mut context, fonts) = context_and_fonts();
    let mut terminal = Terminal::new(20, 4);
    terminal.feed(b"text survives");
    let mut snapshot = (*terminal.snapshot()).clone();
    let rgba: Arc<[u8]> = vec![255; 4].into();
    for id in 1..=258 {
        snapshot.images.push(TerminalImagePlacement {
            image: Arc::new(TerminalImage {
                id,
                revision: 1,
                width: 1,
                height: 1,
                rgba: rgba.clone(),
            }),
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            source_rect: [0.0, 0.0, 1.0, 1.0],
            z_index: 0,
        });
    }
    let mut renderer = TerminalRenderer::default();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().image_resident_textures, 256);
    assert_eq!(renderer.stats().image_resident_bytes, 256 * 4);
    assert_eq!(renderer.stats().rejected_images, 2);
    assert!(renderer.warning().unwrap().contains("texture budget"));
    assert!(renderer.stats().rasterized_glyphs > 0);
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().rejected_images, 2);
    snapshot.images.remove(0);
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().image_resident_textures, 256);
    assert!(renderer.warning().is_some());
    snapshot.images.remove(0);
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    assert_eq!(renderer.stats().image_resident_textures, 256);
    assert!(renderer.warning().is_none());
    renderer.clear(&mut context);
    assert_eq!(renderer.stats().image_resident_textures, 0);
}
