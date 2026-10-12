fn main() {
    #[cfg(feature = "terminal")]
    if let Some(result) = bedterm::run_shell_command(
        std::env::args_os(),
        std::env::var_os("BEDTERM_SOCKET").is_some(),
    ) {
        if let Err(error) = result {
            eprintln!("bEd shell command: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(feature = "debug")]
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(bed_debug_core::launcher::FLAG))
    {
        if let Err(error) = bed_debug_core::launcher::run(std::env::args_os().skip(2)) {
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
