use bedside_table::configure;
use bedtime::{DesktopOptions, LaunchOutcome};
use std::{ffi::OsString, io, path::PathBuf};

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
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "bedside-table [DATABASE ...] [--config-dir PATH]\n\nBrowse local SQLite tables, views and schema; run read-only SQL.\nQuery Save writes to the application's metadata database.\n\nAutomation: --allow-automation [--approve-automation=CLIENT_NAME] [--automation-socket PATH]"
                );
                return Ok(0);
            }
            Some("--config-dir") => config = Some(path_argument(arguments.next(), "--config-dir")?),
            Some(argument) if argument.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unknown option: {argument}"),
                ));
            }
            _ => databases.push(PathBuf::from(argument)),
        }
    }
    let mut options = DesktopOptions {
        config_directory: config,
        ..Default::default()
    };
    options.desktop.window.resolution = (1100, 760).into();
    let mut app = launch.build(options, |app| configure(app, databases))?;
    let exit = app.run();
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
