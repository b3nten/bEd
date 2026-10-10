//! Reproduce row preparation costs with `cargo run --release -p bed-terminal
//! --example terminal_render_baseline`. The pre-migration renderer measured
//! 82.44 us/full 160x48 preparation (7,922 DrawOps), on the implementation host.
use bed_terminal::{
    terminal::Terminal, terminal_font::TerminalFonts, terminal_renderer::TerminalRenderer,
};
use dear_imgui_rs::Context;
use std::{
    hint::black_box,
    path::Path,
    time::{Duration, Instant},
};
fn main() {
    let mut context = Context::create();
    let mut fonts = TerminalFonts::default();
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    fonts.reload(&mut context, root, 16.0).unwrap();
    let mut terminal = Terminal::new(160, 48);
    terminal.feed(
        &"The quick brown fox jumps over the lazy dog 0123456789\r\n"
            .repeat(60)
            .into_bytes(),
    );
    let snapshot = terminal.snapshot();
    let mut renderer = TerminalRenderer::default();
    let cold = Instant::now();
    renderer
        .prepare(&mut context, &snapshot, &fonts, 1.0)
        .unwrap();
    println!(
        "cold viewport+glyph preparation: {:.2} us",
        cold.elapsed().as_secs_f64() * 1e6
    );
    let before = renderer.stats();
    let start = Instant::now();
    for _ in 0..5000 {
        renderer
            .prepare(&mut context, black_box(&snapshot), &fonts, 1.0)
            .unwrap();
    }
    let after = renderer.stats();
    println!(
        "unchanged preparation: {:.2} us/frame; {} rows shaped, {} bytes uploaded; 160x48, 5000 frames",
        start.elapsed().as_secs_f64() * 1e6 / 5000.,
        after.prepared_rows - before.prepared_rows,
        after.glyph_upload_bytes - before.glyph_upload_bytes
    );
    let mut changed = Duration::ZERO;
    for index in 0..1000 {
        terminal.feed(format!("\x1b[1;1Hchanged row {:04}", index).as_bytes());
        let snapshot = terminal.snapshot();
        let start = Instant::now();
        renderer
            .prepare(&mut context, &snapshot, &fonts, 1.0)
            .unwrap();
        changed += start.elapsed();
    }
    println!(
        "single changed row preparation: {:.2} us/frame; 1000 frames",
        changed.as_secs_f64() * 1e6 / 1000.
    );
    renderer.clear(&mut context);
}
