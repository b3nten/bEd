#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(bed_debug::launcher::FLAG))
    {
        if let Err(error) = bed_debug::launcher::run(std::env::args_os().skip(2)) {
            eprintln!("bEd debugger launcher: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = bed::bed::run() {
        eprintln!("bEd: {error}");
        std::process::exit(1);
    }
}
