#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    if let Err(error) = bed::bed::run() {
        eprintln!("Bed: {error}");
        std::process::exit(1);
    }
}
