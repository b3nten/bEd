use bedside_table::{configure, smoke};
use bedtime::{DesktopOptions, LaunchOutcome};
use std::{ffi::OsString, io, path::PathBuf, sync::atomic::Ordering};

fn main() {
    match run() {
        Ok(exit) => std::process::exit(exit),
        Err(error) => {
            eprintln!("bedside-table: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> io::Result<i32> {
    let launch = match bedtime::awaken("bedside-table", "Bedside Table")?.dispatch()? {
        LaunchOutcome::Handled(exit) => return Ok(exit),
        LaunchOutcome::Desktop(launch) => launch,
    };
    let mut arguments = launch.arguments().iter();
    let mut config = None;
    let mut databases = Vec::new();
    let mut capture = None;
    let mut smoke_mode = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "bedside-table [DATABASE ...] [--config-dir PATH]\n\nBrowse local SQLite tables, views and schema; run read-only SQL.\nQuery Save writes to the application's metadata database.\n\nNative proof: --smoke / --smoke-reopen --config-dir PATH [--capture-frame PNG]"
                );
                return Ok(0);
            }
            Some("--config-dir") => config = Some(path_argument(arguments.next(), "--config-dir")?),
            Some("--capture-frame") => {
                capture = Some(path_argument(arguments.next(), "--capture-frame")?)
            }
            Some("--smoke") => smoke_mode = Some(false),
            Some("--smoke-reopen") => smoke_mode = Some(true),
            Some(argument) if argument.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unknown option: {argument}"),
                ));
            }
            _ => databases.push(PathBuf::from(argument)),
        }
    }
    if let Some(reopen) = smoke_mode {
        let directory = config.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Native smokes require an explicit --config-dir",
            )
        })?;
        if !reopen {
            if directory.join("metadata.sqlite").exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Initial native smoke requires a fresh config directory",
                ));
            }
            databases.extend(smoke::seed_databases(&directory.join("smoke-fixtures"))?);
        } else if !directory.join("metadata.sqlite").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Restart smoke requires metadata from the initial smoke",
            ));
        }
    }
    let mut options = DesktopOptions {
        config_directory: config,
        ..Default::default()
    };
    options.desktop.window.resolution = (1100, 760).into();
    let mut app = launch.build(options, |app| configure(app, databases))?;
    let verified = smoke_mode.map(|reopen| smoke::install(&mut app, reopen, capture));
    let exit = app.run();
    if let Some(verified) = verified {
        assert!(
            verified.load(Ordering::SeqCst),
            "Bedside stopped before completing its native smoke"
        );
        assert!(!exit.is_error());
        println!(
            "bedside-table native {} smoke PASS",
            if smoke_mode == Some(true) {
                "restart"
            } else {
                "initial"
            }
        );
    }
    Ok(if exit.is_error() { 1 } else { 0 })
}

fn path_argument(argument: Option<&OsString>, option: &str) -> io::Result<PathBuf> {
    argument.map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{option} requires a path"),
        )
    })
}
