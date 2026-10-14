//! Standalone trampoline for adapter integration tests and embedding hosts.
fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(bedtime_debugger::launcher::FLAG)) {
        eprintln!("Expected {}", bedtime_debugger::launcher::FLAG);
        std::process::exit(2);
    }
    if let Err(error) = bedtime_debugger::launcher::run(args) {
        eprintln!("Debugger launcher: {error}");
        std::process::exit(1);
    }
}
