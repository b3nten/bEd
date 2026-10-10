//! Standalone trampoline for adapter integration tests and embedding hosts.
fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(bed_debug::launcher::FLAG)) {
        eprintln!("Expected {}", bed_debug::launcher::FLAG);
        std::process::exit(2);
    }
    if let Err(error) = bed_debug::launcher::run(args) {
        eprintln!("Debugger launcher: {error}");
        std::process::exit(1);
    }
}
