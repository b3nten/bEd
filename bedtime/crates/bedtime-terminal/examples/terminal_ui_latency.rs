//! Measure UI-side snapshot handoff and viewport preparation during PTY floods:
//! `cargo run --release -p bedtime-terminal --example terminal_ui_latency`.
//! This excludes window drawing, GPU submission, and BedTerminal's cwd queries.
use bedtime_terminal::{
    terminal::Terminal,
    terminal_font::TerminalFonts,
    terminal_pty::{PtyOptions, TerminalPty, TerminalShell, WindowSize},
    terminal_view::TerminalView,
};
use dear_imgui_rs::Context;
use std::{
    io::{self, Write},
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn main() -> io::Result<()> {
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("--idle-child") {
        loop {
            thread::park();
        }
    }
    if mode.as_deref() == Some("--flood-child") {
        let mut output = io::BufWriter::new(io::stdout().lock());
        for index in 0u64.. {
            writeln!(
                output,
                "\x1b[32m{index:016x}\x1b[0m compiling terminal workload https://example.com/source/{index:016x}"
            )?;
        }
        return Ok(());
    }

    let mut context = Context::create();
    let mut fonts = TerminalFonts::default();
    fonts.reload(&mut context, Path::new(env!("CARGO_MANIFEST_DIR")), 16.0)?;
    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
    for (label, count, visible, flood) in [
        ("idle", 1, 1, false),
        ("one flood, visible", 1, 1, true),
        ("four floods, one visible", 4, 1, true),
        ("four floods, all hidden", 4, 0, true),
        ("four floods, all visible", 4, 4, true),
    ] {
        let mut sessions = Vec::new();
        for _ in 0..count {
            let options = PtyOptions {
                shell: Some(TerminalShell::new(
                    &executable,
                    vec![
                        if flood {
                            "--flood-child"
                        } else {
                            "--idle-child"
                        }
                        .into(),
                    ],
                )),
                ..Default::default()
            };
            let pty = TerminalPty::spawn_terminal(
                &options,
                WindowSize {
                    num_cols: 160,
                    num_lines: 48,
                    cell_width: 8,
                    cell_height: 16,
                },
                Terminal::new(160, 48),
            )?;
            let snapshot = pty.snapshot();
            sessions.push((pty, snapshot, TerminalView::default()));
        }
        let mut handoff = Vec::new();
        let mut prepare = Vec::new();
        let mut total = Vec::new();
        let mut updates = 0;
        // Warm caches for 30 frames, then measure 180 frames at roughly 60 Hz.
        // Keep the previous UI snapshot alive until replacement, as production does.
        for frame in 0..210 {
            let started = Instant::now();
            for (pty, snapshot, _) in &mut sessions {
                let next = pty.snapshot();
                if frame >= 30 && next.revision != snapshot.revision {
                    updates += 1;
                }
                *snapshot = next;
                pty.request_snapshot();
                if let Some(event) = pty.poll().into_iter().next() {
                    return Err(io::Error::other(format!(
                        "Unexpected worker event: {event:?}"
                    )));
                }
            }
            let received = Instant::now();
            for (_, snapshot, view) in sessions.iter_mut().take(visible) {
                view.prepare_frame(&mut context, snapshot, &fonts, 1.0)?;
            }
            let finished = Instant::now();
            if frame >= 30 {
                handoff.push(received - started);
                prepare.push(finished - received);
                total.push(finished - started);
            }
            thread::sleep(Duration::from_secs_f64(1.0 / 60.0).saturating_sub(started.elapsed()));
        }
        println!("{label}: 160x48, {updates} snapshot updates");
        for (name, mut samples) in [("handoff", handoff), ("prepare", prepare), ("total", total)] {
            samples.sort_unstable();
            println!(
                "  {name}: p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms",
                samples[samples.len() / 2].as_secs_f64() * 1000.0,
                samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0,
                samples[samples.len() * 99 / 100].as_secs_f64() * 1000.0,
                samples.last().unwrap().as_secs_f64() * 1000.0,
            );
        }
        let closing = Instant::now();
        for (pty, _, view) in &mut sessions {
            pty.shutdown();
            view.clear(&mut context);
        }
        println!(
            "  shutdown: {:.3}ms",
            closing.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
