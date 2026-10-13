use super::*;
use std::time::{Duration, Instant};

const DEJAVU: &[u8] = include_bytes!("../../../tests/fixtures/fonts/DejaVuSans.ttf");
const SOURCE: &[u8] = include_bytes!("../../../tests/fixtures/fonts/SourceCodePro-Regular.ttf");
const WOFF: &[u8] = include_bytes!("../tests/fixtures/SourceCodePro-Regular.woff");
const WOFF2: &[u8] = include_bytes!("../tests/fixtures/SourceCodePro-Regular.woff2");

fn engine() -> FontEngine {
    FontEngine::new(Arc::from(DEJAVU), 0).unwrap()
}

fn request(mode: Mode) -> RasterRequest {
    RasterRequest {
        mode,
        pixels: [800, 480],
        logical: [800.0, 480.0],
        label_height: 16.0,
        size: 36.0,
        sample: "Office affinity AVATAR\nq\u{301}\nسلام".into(),
        ligatures: true,
        kerning: true,
        direction: 0,
        pan: [0.0; 2],
        glyph_scroll: 0.0,
        selected: 36,
        ink: [0.0, 0.0, 0.0, 1.0],
    }
}

#[test]
fn registered_fonts_use_exact_byte_documents() {
    let mut registry = bed_editor_api::Registry::default();
    registry
        .register_with(PLUGIN_ID, register_metadata)
        .unwrap();
    for path in ["a.TTF", "b.otf", "c.TtC", "d.OTC", "e.WoFf", "f.WOFF2"] {
        let viewer = registry.viewer_for_path(path).unwrap();
        assert_eq!(viewer.id, VIEWER_ID);
        assert_eq!(viewer.document_kind(), Some(DocumentKind::Bytes));
    }
    assert!(registry.viewer_for_path("a.bin").is_none());
    assert!(registry.viewer_for_path("folder.ttf/file").is_none());
}

#[test]
fn reads_real_metadata_and_rasterizes_glyphs_without_changing_the_font() {
    let mut engine = engine();
    assert_eq!(engine.info.family, "DejaVu Sans");
    assert!(engine.info.glyphs.len() > 1000);
    assert_eq!(engine.info.units_per_em, 2048);
    assert!(engine.info.names.iter().any(|(name, _)| name == "License"));
    let id = find_glyph(&engine.info, "A").unwrap();
    assert_eq!(find_glyph(&engine.info, "U+0041"), Some(id));
    let details = engine.details(id).unwrap();
    assert_eq!(details.name, "A");
    assert_eq!(details.codepoint, Some(65));
    assert!(details.advance > 0 && details.size[0] > 0);
    let bitmap = engine.bitmap(id, 48).unwrap();
    assert!(bitmap.size[0] > 0 && bitmap.size[1] > 0);
    assert!(bitmap.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    assert!(Arc::ptr_eq(&bitmap, &engine.bitmap(id, 48).unwrap()));
    let space = find_glyph(&engine.info, "U+0020").unwrap();
    assert!(engine.bitmap(space, 48).unwrap().rgba.is_empty());
}

#[test]
fn harfrust_applies_ligatures_kerning_marks_and_arabic_shaping() {
    let engine = engine();
    let plain = engine.shape("ffi", 48.0, false, true, 0).unwrap();
    let ligature = engine.shape("ffi", 48.0, true, true, 0).unwrap();
    assert_eq!(plain.glyphs.len(), 3);
    assert_eq!(ligature.glyphs.len(), 1);
    assert_eq!(ligature.glyphs[0].cluster, 0);
    assert!(
        engine.shape("AV", 48.0, true, true, 0).unwrap().width
            < engine.shape("AV", 48.0, true, false, 0).unwrap().width
    );
    let marks = engine.shape("q\u{301}", 48.0, true, true, 0).unwrap();
    assert_eq!(marks.glyphs.len(), 2);
    assert_eq!(marks.glyphs[1].advance[0], 0.0);
    assert!(marks.glyphs[1].position[0] < marks.width);
    let arabic = engine.shape("سلام", 48.0, true, true, 0).unwrap();
    assert!(arabic.rtl);
    assert!(arabic.glyphs.len() < 4);
    assert!(arabic.glyphs.iter().all(|g| g.id != 0));
    assert!(
        arabic
            .glyphs
            .windows(2)
            .all(|g| g[0].cluster >= g[1].cluster)
    );
    assert!(
        engine
            .shape("\u{10ffff}", 48.0, true, true, 0)
            .unwrap()
            .glyphs
            .iter()
            .any(|g| g.id == 0)
    );
}

#[test]
fn preview_renders_at_physical_resolution_and_includes_unencoded_glyphs() {
    let mut engine = engine();
    let normal = raster::render(&mut engine, &request(Mode::Preview)).unwrap();
    let mut high_dpi = request(Mode::Preview);
    high_dpi.pixels = [1600, 960];
    let large = raster::render(&mut engine, &high_dpi).unwrap();
    let ink = |r: &Raster| {
        assert_eq!(&r.rgba[..4], &[0; 4], "canvas must start transparent");
        r.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] >= 128)
            .count()
    };
    assert!(ink(&normal) > 100);
    let ratio = ink(&large) as f32 / ink(&normal) as f32;
    assert!((3.5..4.5).contains(&ratio), "Retina ink area ratio {ratio}");
    assert_eq!(normal.missing_glyphs, 0);
    let unencoded = engine
        .info
        .glyphs
        .iter()
        .enumerate()
        .find(|(id, cp)| *id > 100 && cp.is_none())
        .unwrap()
        .0;
    let mut grid = request(Mode::Glyphs);
    grid.glyph_scroll = GridLayout::new(grid.logical, grid.label_height, grid.size)
        .scroll_to(unencoded, engine.info.glyphs.len());
    grid.selected = unencoded as u32;
    let raster = raster::render(&mut engine, &grid).unwrap();
    assert!(raster.selected.codepoint.is_none());
    assert!(ink(&raster) > 100);
    let mut invalid = request(Mode::Preview);
    invalid.pixels = [u32::MAX; 2];
    assert!(raster::render(&mut engine, &invalid).is_err());
}

#[test]
fn glyph_scrolling_moves_pixels_continuously_and_keeps_inspector_fixed() {
    let mut engine = engine();
    for logical in [[800.0, 480.0], [400.0, 480.0]] {
        let mut config = request(Mode::Glyphs);
        config.logical = logical;
        config.pixels = logical.map(|v| v as u32);
        let grid = GridLayout::new(logical, config.label_height, config.size);
        let before = raster::render(&mut engine, &config).unwrap();
        config.glyph_scroll = 17.0;
        let after = raster::render(&mut engine, &config).unwrap();
        let stride = config.pixels[0] as usize * 4;
        let width = grid.viewport[0].round() as usize * 4;
        let height = grid.viewport[1].round() as usize;
        for y in 0..height - 17 {
            assert_eq!(
                &after.rgba[y * stride..y * stride + width],
                &before.rgba[(y + 17) * stride..(y + 17) * stride + width],
                "scrolling must translate the grid by pixels, not replace a page"
            );
        }
        let [x, y, w, h] = grid.detail.map(|v| v.round() as usize);
        for row in y..y + h {
            let start = row * stride + x * 4;
            assert_eq!(
                &after.rgba[start..start + w * 4],
                &before.rgba[start..start + w * 4]
            );
        }
        let scroll = grid.scroll_to(1000, engine.info.glyphs.len()) + 17.0;
        let id = grid
            .glyph_at([10.0, 10.0], scroll, engine.info.glyphs.len())
            .unwrap();
        assert!(grid.visible(scroll, engine.info.glyphs.len()).contains(&id));
        let origin = grid.cell_origin(id, scroll);
        assert!(origin[1] <= 10.0 && origin[1] + grid.cell[1] > 10.0);
        assert!(
            grid.glyph_at([grid.viewport[0], 0.0], scroll, engine.info.glyphs.len())
                .is_none()
        );
        assert!(
            grid.glyph_at([0.0, grid.viewport[1]], scroll, engine.info.glyphs.len())
                .is_none()
        );
        assert_eq!(
            grid.visible(
                grid.scroll_max(engine.info.glyphs.len()),
                engine.info.glyphs.len()
            )
            .end,
            engine.info.glyphs.len()
        );
    }
}

#[test]
fn glyph_canvas_uses_native_wheel_scrollbar_jump_and_selection() {
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    let mut engine = engine();
    let info = Arc::clone(&engine.info);
    let textures = std::collections::HashMap::new();
    let host = HostContext {
        remote: false,
        default_viewers: &Value::Null,
        viewer_menu: None,
        documents: &[],
        active_document: None,
        settings: &Value::Null,
        textures: &textures,
        animations: false,
        workspace: 0,
        diagnostics: &Value::Null,
    };
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut panel = FontPanel::new(DocumentId(1), &json!({"view": 1}));
    let mut initial = request(Mode::Glyphs);
    initial.selected = panel.selected;
    panel.preview = Some(Preview {
        raster: raster::render(&mut engine, &initial).unwrap(),
        request: initial,
        info: Arc::clone(&info),
    });
    let frame = |context: &mut Context, panel: &mut FontPanel| {
        context.prepare_frame(FramePrepareOptions::new([820.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut result = None;
        ui.window("font scroll fixture")
            .position([0.0; 2], Condition::Always)
            .size([820.0, 600.0], Condition::Always)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_SCROLL_WITH_MOUSE
                    | WindowFlags::NO_SCROLLBAR,
            )
            .build(|| {
                panel.glyph_controls(ui, &info);
                let origin = ui.cursor_screen_pos();
                let canvas = panel.glyph_canvas(ui, &host, Some(&info)).unwrap();
                assert_eq!(ui.scroll_y(), 0.0, "the outer panel must stay fixed");
                result = Some((origin, canvas));
            });
        drop(context.render_legacy());
        result.unwrap()
    };
    context.io_mut().add_mouse_pos_event([100.0, 100.0]);
    let (origin, canvas) = frame(&mut context, &mut panel);
    for _ in 0..3 {
        frame(&mut context, &mut panel);
    }
    context.io_mut().add_mouse_wheel_event([0.0, -0.25]);
    frame(&mut context, &mut panel);
    let scrolled = panel.glyph_scroll;
    assert!(
        scrolled > 0.0 && scrolled < 36.0,
        "fractional wheel scroll: {scrolled}"
    );
    for _ in 0..4 {
        frame(&mut context, &mut panel);
    }
    assert_eq!(
        panel.glyph_scroll, scrolled,
        "scroll must not drift each frame"
    );

    panel.reveal_glyph = Some(1000);
    frame(&mut context, &mut panel);
    let jumped = panel.glyph_scroll;
    assert!(jumped > scrolled);
    let scrollbar_x = origin[0] + canvas.size[0] + 4.0;
    context
        .io_mut()
        .add_mouse_pos_event([scrollbar_x, origin[1] + canvas.size[1] * 0.75]);
    frame(&mut context, &mut panel);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut panel);
    context
        .io_mut()
        .add_mouse_pos_event([scrollbar_x, origin[1] + canvas.size[1] * 0.9]);
    frame(&mut context, &mut panel);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut panel);
    assert!(
        panel.glyph_scroll > jumped,
        "dragging the native scrollbar must move the grid"
    );

    let mut config = request(Mode::Glyphs);
    config.logical = canvas.size;
    config.pixels = canvas.pixels;
    config.label_height = 13.0;
    config.glyph_scroll = panel.glyph_scroll;
    let grid = GridLayout::new(config.logical, config.label_height, config.size);
    let expected = grid
        .glyph_at([20.0, 40.0], config.glyph_scroll, info.glyphs.len())
        .unwrap();
    panel.preview = Some(Preview {
        raster: raster::render(&mut engine, &config).unwrap(),
        request: config,
        info: Arc::clone(&info),
    });
    context
        .io_mut()
        .add_mouse_pos_event([origin[0] + 20.0, origin[1] + 40.0]);
    frame(&mut context, &mut panel);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    let (selected_origin, selected_canvas) = frame(&mut context, &mut panel);
    assert_eq!(
        panel.selected, expected as u32,
        "clicks must select the displayed glyph after scrolling"
    );
    let (pending_origin, pending_canvas) = frame(&mut context, &mut panel);
    assert_eq!(
        pending_origin, selected_origin,
        "pending selection must not hide the metrics row"
    );
    assert_eq!(pending_canvas.size, selected_canvas.size);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut panel);
    let state = panel.save_state();
    let mut restored = FontPanel::new(DocumentId(1), &state);
    restored.preview = panel.preview.take();
    frame(&mut context, &mut restored);
    assert_eq!(restored.glyph_scroll, panel.glyph_scroll);
}

// Collection table offsets are absolute from the TTC's beginning, rather than
// relative to each SFNT. This exercises actual collection parsing in both engines.
fn collection() -> Arc<[u8]> {
    let fonts = [DEJAVU, SOURCE];
    let mut bytes = Vec::from(&b"ttcf\0\x01\0\0\0\0\0\x02\0\0\0\0\0\0\0\0"[..]);
    for (index, font) in fonts.into_iter().enumerate() {
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let base = bytes.len();
        bytes[12 + index * 4..16 + index * 4].copy_from_slice(&(base as u32).to_be_bytes());
        let mut sfnt = font.to_vec();
        let tables = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
        for table in 0..tables {
            let offset = 12 + table * 16 + 8;
            let value = u32::from_be_bytes(sfnt[offset..offset + 4].try_into().unwrap());
            sfnt[offset..offset + 4].copy_from_slice(&(value + base as u32).to_be_bytes());
        }
        bytes.extend(sfnt);
    }
    bytes.into()
}

#[test]
fn collections_select_matching_faces_in_freetype_and_harfrust() {
    let bytes = collection();
    let first = FontEngine::new(Arc::clone(&bytes), 0).unwrap();
    let second = FontEngine::new(Arc::clone(&bytes), 1).unwrap();
    assert_eq!(first.info.face_count, 2);
    assert_eq!(first.info.family, "DejaVu Sans");
    assert!(second.info.family.contains("Source Code Pro"));
    assert_eq!(second.info.face_index, 1);
    let first_run = first.shape("ffi", 48.0, true, true, 0).unwrap();
    let second_run = second.shape("ffi", 48.0, true, true, 0).unwrap();
    assert_ne!(first_run.width, second_run.width);
    assert_eq!(FontEngine::new(bytes, u32::MAX).unwrap().info.face_index, 1);
}

#[test]
fn malformed_and_truncated_fonts_fail_cleanly() {
    for bytes in [&b"not a font"[..], &DEJAVU[..128], &[]] {
        assert!(FontEngine::new(Arc::from(bytes), 0).is_err());
    }
}

// Encode the bundled SFNT without changing its tables, so WOFF1 has an
// independent reference for shaping and raster output (including Arabic).
fn woff1(font: &[u8]) -> Arc<[u8]> {
    use std::io::Write;
    let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut output = vec![0; 44 + 20 * tables];
    output[..4].copy_from_slice(b"wOFF");
    output[4..8].copy_from_slice(&font[..4]);
    output[12..14].copy_from_slice(&(tables as u16).to_be_bytes());
    let mut decoded_size = 12 + 16 * tables;
    for (index, entry) in font[12..12 + 16 * tables]
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
    {
        let offset = u32::from_be_bytes(entry[8..12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(entry[12..16].try_into().unwrap()) as usize;
        let data = &font[offset..offset + length];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        let payload = if compressed.len() < data.len() {
            &compressed
        } else {
            data
        };
        let at = output.len() as u32;
        let entry_out = &mut output[44 + 20 * index..64 + 20 * index];
        entry_out[..4].copy_from_slice(&entry[..4]);
        entry_out[4..8].copy_from_slice(&at.to_be_bytes());
        entry_out[8..12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        entry_out[12..16].copy_from_slice(&(length as u32).to_be_bytes());
        entry_out[16..20].copy_from_slice(&entry[4..8]);
        output.extend_from_slice(payload);
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
        decoded_size += (length + 3) & !3;
    }
    let length = output.len() as u32;
    output[8..12].copy_from_slice(&length.to_be_bytes());
    output[16..20].copy_from_slice(&(decoded_size as u32).to_be_bytes());
    output.into()
}

fn assert_same_font(expected: &mut FontEngine, actual: &mut FontEngine) {
    assert_eq!(actual.info.family, expected.info.family);
    assert_eq!(actual.info.style, expected.info.style);
    assert_eq!(actual.info.units_per_em, expected.info.units_per_em);
    assert_eq!(actual.info.glyphs, expected.info.glyphs);
    assert_eq!(actual.info.mappings, expected.info.mappings);
    let sample = "Office affinity AVATAR q\u{301} سلام";
    let shaped = |font: &FontEngine| {
        font.shape(sample, 48.0, true, true, 0)
            .unwrap()
            .glyphs
            .into_iter()
            .map(|g| (g.id, g.cluster, g.position, g.advance))
            .collect::<Vec<_>>()
    };
    assert_eq!(shaped(actual), shaped(expected));
    let expected = raster::render(expected, &request(Mode::Preview)).unwrap();
    let actual = raster::render(actual, &request(Mode::Preview)).unwrap();
    assert_eq!(actual.rgba, expected.rgba);
}

#[test]
fn woff_preserves_sfnt_shaping_raster_and_original_document_bytes() {
    let original: Arc<[u8]> = Arc::from(DEJAVU);
    assert!(Arc::ptr_eq(
        &original,
        &webfont::decode(Arc::clone(&original)).unwrap()
    ));
    let compressed = woff1(DEJAVU);
    assert!(compressed.len() < DEJAVU.len());
    let snapshot = compressed.to_vec();
    let mut webfont = FontEngine::new(Arc::clone(&compressed), 0).unwrap();
    assert_same_font(&mut engine(), &mut webfont);
    assert_eq!(compressed.as_ref(), snapshot);
}

#[test]
fn published_woff_and_transformed_woff2_have_matching_shaping_and_pixels() {
    let mut woff = FontEngine::new(Arc::from(WOFF), 0).unwrap();
    let mut woff2 = FontEngine::new(Arc::from(WOFF2), 0).unwrap();
    assert_eq!(woff2.info.family, "Source Code Pro");
    assert_same_font(&mut woff, &mut woff2);
}

#[test]
fn corrupt_and_oversized_webfonts_fail_before_reaching_freetype() {
    for bytes in [WOFF, WOFF2] {
        for length in [4, 20, 43, bytes.len() / 2, bytes.len() - 1] {
            assert!(webfont::decode(Arc::from(&bytes[..length])).is_err());
        }
        let mut oversized = bytes.to_vec();
        oversized[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(
            webfont::decode(oversized.into())
                .unwrap_err()
                .contains("64 MiB")
        );
        let mut broken = bytes.to_vec();
        broken[12..14].fill(0);
        assert!(webfont::decode(broken.into()).is_err());
        let mut broken = bytes.to_vec();
        broken[bytes.len() / 2..].fill(0xff);
        assert!(FontEngine::new(broken.into(), 0).is_err());
    }
    let mut oversized_table = WOFF.to_vec();
    oversized_table[56..60].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(webfont::decode(oversized_table.into()).is_err());
    let mut invalid_length = WOFF.to_vec();
    invalid_length[52..56].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(webfont::decode(invalid_length.into()).is_err());
}

#[test]
fn worker_reloads_webfonts_after_hex_edits_and_recovers_after_undo() {
    let bytes: Arc<[u8]> = Arc::from(WOFF2);
    let mut panel = FontPanel::new(DocumentId(1), &Value::Null);
    panel.submit((1, 0), &bytes, request(Mode::Preview));
    wait_for_panel(&mut panel, (1, 0));
    assert!(panel.preview.is_some());
    let mut edited = WOFF2.to_vec();
    edited[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    panel.submit((2, 0), &edited.into(), request(Mode::Preview));
    panel.poll((2, 0));
    wait_for_panel(&mut panel, (2, 0));
    assert!(panel.error.is_some());
    panel.submit((3, 0), &bytes, request(Mode::Preview));
    wait_for_panel(&mut panel, (3, 0));
    assert!(panel.error.is_none());
    assert_eq!(
        panel.preview.as_ref().unwrap().info.family,
        "Source Code Pro"
    );
    assert_eq!(bytes.as_ref(), WOFF2);
}

fn wait_for_panel(panel: &mut FontPanel, revision: Revision) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.poll(revision);
        if panel.preview.is_some() || panel.error.is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "font worker timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn worker_invalidates_edited_fonts_and_recovers_after_another_edit() {
    let mut panel = FontPanel::new(DocumentId(1), &Value::Null);
    panel.submit((1, 0), &Arc::from(DEJAVU), request(Mode::Preview));
    wait_for_panel(&mut panel, (1, 0));
    assert!(panel.preview.is_some());
    panel.submit((2, 0), &Arc::from(&b"bad font"[..]), request(Mode::Preview));
    panel.poll((2, 0));
    assert!(panel.preview.is_none());
    wait_for_panel(&mut panel, (2, 0));
    assert!(panel.error.is_some());
    panel.submit((3, 0), &Arc::from(SOURCE), request(Mode::Preview));
    wait_for_panel(&mut panel, (3, 0));
    assert!(panel.error.is_none());
    assert!(
        panel
            .preview
            .as_ref()
            .unwrap()
            .info
            .family
            .contains("Source Code Pro")
    );
}

#[test]
fn delayed_worker_results_do_not_overwrite_newer_scroll_glyph_or_face_choices() {
    let mut panel = FontPanel::new(DocumentId(1), &Value::Null);
    let mut old = request(Mode::Glyphs);
    old.selected = 0;
    panel.submit((1, 0), &Arc::from(DEJAVU), old);
    let result = panel
        .worker
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    panel.worker.receiver = receiver;
    panel.selected = 36;
    panel.glyph_scroll = 32.5;
    sender.send(result).unwrap();
    panel.poll((1, 0));
    assert_eq!(panel.selected, 36);
    assert_eq!(panel.glyph_scroll, 32.5);
    panel.face = 1;
    sender
        .send(JobResult {
            serial: panel.serial,
            revision: (1, 0),
            result: Err("old face failed".into()),
        })
        .unwrap();
    panel.poll((1, 0));
    assert_eq!(panel.face, 1);
    assert!(panel.error.is_none());
}

#[test]
fn restored_state_is_bounded_and_keeps_unicode_sample_text() {
    let text = "🦀".repeat(MAX_SAMPLE_BYTES);
    let state =
        json!({ "sample": text, "size": -999, "face": u64::MAX, "view": 99, "pan": [-10, 1e100] });
    let panel = FontPanel::new(DocumentId(1), &state);
    assert!(panel.sample.len() <= MAX_SAMPLE_BYTES);
    assert!(panel.sample.ends_with('🦀'));
    assert_eq!(panel.size, 8.0);
    assert_eq!(panel.view, 2);
    assert_eq!(panel.pan, [0.0, 1_000_000.0]);
    let restored = FontPanel::new(DocumentId(2), &panel.save_state());
    assert_eq!(restored.save_state(), panel.save_state());
    let panel = FontPanel::new(DocumentId(3), &json!({"sample": "a\0b\n".repeat(300)}));
    assert_eq!(panel.sample.lines().count(), 256);
    assert!(!panel.sample.contains('\0'));
}

#[test]
#[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
fn native_registered_font_renderer_preserves_raster_and_recovers_generation() {
    use bed_ui::gpu::{renderer_device_descriptor, wgpu};
    use std::{future::Future, task::Wake};
    use std::{sync::mpsc, thread};

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        struct ThreadWake(thread::Thread);

        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(Arc::new(ThreadWake(thread::current())));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => thread::park(),
            }
        }
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) =
        block_on(adapter.request_device(&renderer_device_descriptor(&adapter))).unwrap();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut app = bevy::app::App::new();
    app.add_plugins(crate::FontPlugin);
    let entity = app
        .world_mut()
        .spawn(bed_editor_api::EditorPanelBinding {
            input: bed_editor_api::PanelInput::Document(DocumentId(4)),
            document: Some(DocumentId(4)),
            ..Default::default()
        })
        .id();
    let factory = app
        .world()
        .resource::<bed_workbench_api::Registry>()
        .panel(crate::PANEL_ID)
        .unwrap()
        .factory;
    let _frontend = app
        .world_mut()
        .run_system_with(
            factory,
            bed_workbench_api::PanelCreation {
                entity,
                state: Value::Null,
            },
        )
        .unwrap()
        .unwrap();
    let output = {
        let mut states = app.world_mut().non_send_mut::<crate::FontStates>();
        let panel = states.get_mut(entity).unwrap();
        let mut engine = FontEngine::new(
            Arc::from(include_bytes!("../../../tests/fixtures/fonts/DejaVuSans.ttf").as_slice()),
            0,
        )
        .unwrap();
        let request = RasterRequest {
            mode: Mode::Preview,
            pixels: [64, 32],
            logical: [64.0, 32.0],
            label_height: 13.0,
            size: 20.0,
            sample: "A".into(),
            ligatures: true,
            kerning: true,
            direction: 0,
            pan: [0.0; 2],
            glyph_scroll: 0.0,
            selected: 36,
            ink: [0.8, 0.4, 0.2, 1.0],
        };
        let raster = crate::raster::render(&mut engine, &request).unwrap();
        assert!(
            raster
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
        panel.preview = Some(Preview {
            info: Arc::clone(&engine.info),
            raster,
            request,
        });
        panel.canvas = Some([64, 32]);
        panel.canvas_output().unwrap()
    };
    let render = app
        .world()
        .get::<bed_ui::gpu::CanvasRenderer>(entity)
        .unwrap()
        .0;
    let target = RenderTarget::new(&device, output).unwrap();
    let stride = (target.size[0] * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Font canvas readback"),
        size: u64::from(stride) * u64::from(target.size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for generation in [1, 2] {
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = app
            .world_mut()
            .run_system_with(
                render,
                bed_ui::gpu::CanvasRenderInvocation {
                    entity,
                    instance: instance.clone(),
                    adapter: adapter.clone(),
                    device: device.clone(),
                    queue: queue.clone(),
                    target: RenderTarget {
                        texture: target.texture.clone(),
                        view: target.view.clone(),
                        depth: target.depth.clone(),
                        size: target.size,
                    },
                    generation,
                },
            )
            .unwrap()
            .unwrap();
        queue.submit(commands);
        assert_eq!(
            app.world()
                .non_send::<crate::FontStates>()
                .get(entity)
                .unwrap()
                .gpu
                .as_ref()
                .unwrap()
                .generation,
            generation
        );
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(target.size[1]),
                },
            },
            target.texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .expect("readback buffer is mapped");
        let states = app.world().non_send::<crate::FontStates>();
        let expected = &states
            .get(entity)
            .unwrap()
            .preview
            .as_ref()
            .unwrap()
            .raster
            .rgba;
        for row in 0..target.size[1] as usize {
            let actual =
                &mapped[row * stride as usize..row * stride as usize + target.size[0] as usize * 4];
            let expected = &expected
                [row * target.size[0] as usize * 4..(row + 1) * target.size[0] as usize * 4];
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(*expected) <= 1)
            );
        }
        drop(mapped);
        buffer.unmap();
    }
    assert!(block_on(validation.pop()).is_none());
}
