//! Filtering and timings for executables that also serve as subprocess fixtures.
//! Keep these tests sequential: some own the process-global ImGui context.
use std::{io::Write, time::Instant};

pub fn run(tests: &[(&str, fn())]) {
    let mut filters = Vec::new();
    let mut skips = Vec::new();
    let mut exact = false;
    let mut list = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--exact" => exact = true,
            "--list" => list = true,
            "--nocapture" | "--show-output" | "--test-threads=1" => {}
            "--skip" => skips.push(arguments.next().expect("--skip requires a test name")),
            "--test-threads" => {
                assert_eq!(
                    arguments.next().as_deref(),
                    Some("1"),
                    "this fixture runner requires --test-threads=1"
                );
            }
            "--help" | "-h" => {
                println!(
                    "[FILTER ...] [--exact] [--skip FILTER] [--list] [--nocapture] [--test-threads=1]"
                );
                return;
            }
            _ if argument.starts_with('-') => panic!("unsupported test argument: {argument}"),
            _ => filters.push(argument),
        }
    }
    let matches = |name: &str, filter: &String| {
        if exact {
            name == filter
        } else {
            name.contains(filter.as_str())
        }
    };
    let selected: Vec<_> = tests
        .iter()
        .filter(|(name, _)| {
            (filters.is_empty() || filters.iter().any(|filter| matches(name, filter)))
                && !skips.iter().any(|filter| matches(name, filter))
        })
        .collect();
    if list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        println!("\n{} tests", selected.len());
        return;
    }
    let started = Instant::now();
    println!("running {} tests", selected.len());
    for (name, test) in &selected {
        print!("test {name} ... ");
        std::io::stdout().flush().unwrap();
        let started = Instant::now();
        test();
        println!("ok ({:.2}s)", started.elapsed().as_secs_f64());
    }
    println!(
        "\ntest result: ok. {} passed; {} filtered out; finished in {:.2}s",
        selected.len(),
        tests.len() - selected.len(),
        started.elapsed().as_secs_f64()
    );
}
