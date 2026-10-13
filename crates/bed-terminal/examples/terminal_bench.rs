//! Reproducible terminal parser throughput and resize benchmark.
use bed_terminal::terminal::Terminal;
use std::time::Instant;

fn main() {
    let line =
        b"\x1b[32m0123456789 terminal output benchmark abcdefghijklmnopqrstuvwxyz\x1b[0m\r\n";
    let data = line.repeat(4096);
    let mut terminal = Terminal::new(120, 40);
    let started = Instant::now();
    for _ in 0..32 {
        std::hint::black_box(terminal.feed(&data));
    }
    let elapsed = started.elapsed();
    println!(
        "parse_bytes={} parse_ms={:.3} mib_per_second={:.2}",
        data.len() * 32,
        elapsed.as_secs_f64() * 1000.0,
        (data.len() * 32) as f64 / 1048576.0 / elapsed.as_secs_f64()
    );
    let started = Instant::now();
    for cols in (80..=160).cycle().take(200) {
        terminal.resize(cols, 40);
    }
    println!(
        "resize_200_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
