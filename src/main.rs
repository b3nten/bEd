fn main() {
    #[cfg(feature = "terminal")]
    if let Some(result) = bed_plugin_terminal::run_helper(
        std::env::args_os(),
        std::env::var_os("BEDTERM_SOCKET").is_some(),
    ) {
        if let Err(error) = result {
            eprintln!("bEd shell command: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(feature = "debugger")]
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(bed_debugger::launcher::FLAG))
    {
        if let Err(error) = bed_debugger::launcher::run(std::env::args_os().skip(2)) {
            eprintln!("bEd debugger launcher: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = bed::app::run() {
        eprintln!("bEd: {error}");
        std::process::exit(1);
    }
}
