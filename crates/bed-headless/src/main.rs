#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Bed's native helper supports macOS and Linux only.");

use std::{
    io,
    process::{Command, ExitCode, Stdio},
};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("bed-headless: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<u8> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["--protocol-version"] {
        println!("{}", bed_remote::PROTOCOL_VERSION);
        return Ok(0);
    }
    if arguments == ["--stdio"] {
        bed_remote::serve_with(io::stdin().lock(), io::stdout().lock(), dispatch)?;
        return Ok(0);
    }
    if arguments.first().is_some_and(|argument| argument == "exec") {
        return exec(&arguments[1..]);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "usage: bed-headless --protocol-version | --stdio | exec --cwd DIR [--candidate PROGRAM ...] -- PROGRAM ARGS",
    ))
}

fn dispatch(request: bed_remote::Request) -> Result<bed_remote::Response, bed_remote::RemoteError> {
    use bed_remote::{
        ErrorKind, LocalBackend, MAX_FRAME_BYTES, RemoteError, Request, Response, SearchMatch,
    };
    let Request::Search {
        root,
        query,
        case_sensitive,
        include_ignored,
        max_results,
    } = request
    else {
        return LocalBackend.call(request);
    };
    let Response::Path { path: root } = LocalBackend.call(Request::Canonicalize {
        root,
        path: ".".into(),
        allow_missing: false,
    })?
    else {
        unreachable!()
    };
    let snapshot = bed_files::content_search::search_project_snapshot(
        &root,
        query.as_bytes(),
        case_sensitive,
        include_ignored,
        max_results.min(100_000),
    );
    if let Some(error) = snapshot.error {
        return Err(RemoteError::new(ErrorKind::Other, error));
    }
    let mut matches = Vec::new();
    let mut budget = 0_usize;
    let mut truncated = snapshot.limit_reached;
    for item in snapshot.matches {
        budget = budget
            .saturating_add(item.line.len().saturating_mul(12))
            .saturating_add(item.file.full_path.len().saturating_mul(6))
            .saturating_add(512);
        if budget > MAX_FRAME_BYTES / 2 {
            truncated = true;
            break;
        }
        matches.push(SearchMatch {
            path: item.file.full_path,
            line: item.source_row as usize + 1,
            column: item.column as usize + 1,
            editor_row: item.row as usize + 1,
            line_bytes: item.line.to_vec(),
            text: String::from_utf8_lossy(&item.line).into_owned(),
        });
    }
    Ok(Response::Search {
        matches,
        truncated,
        scanned_files: snapshot.progress.scanned_files,
        discovered_files: snapshot.progress.discovered_files,
        ignored_paths: snapshot.progress.ignored_paths,
        skipped_files: snapshot.skipped_files,
    })
}

fn exec(arguments: &[String]) -> io::Result<u8> {
    let mut cwd = None;
    let mut candidates = Vec::new();
    let mut index = 0;
    while index < arguments.len() && arguments[index] != "--" {
        match arguments[index].as_str() {
            "--cwd" | "--candidate" if index + 1 < arguments.len() => {
                if arguments[index] == "--cwd" {
                    cwd = Some(arguments[index + 1].clone());
                } else {
                    candidates.push(arguments[index + 1].clone());
                }
                index += 2;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid exec option",
                ));
            }
        }
    }
    if arguments.get(index).map(String::as_str) != Some("--") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "exec requires -- before command",
        ));
    }
    let mut command_arguments = &arguments[index + 1..];
    if candidates.is_empty() {
        let (program, rest) = command_arguments.split_first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "exec requires a command")
        })?;
        candidates.push(program.clone());
        command_arguments = rest;
    }
    let cwd =
        cwd.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "exec requires --cwd DIR"))?;
    let mut last_error = None;
    for program in candidates {
        let mut command = Command::new(&program);
        command
            .args(command_arguments)
            .current_dir(&cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if let Some(path) = child_path(&program) {
            command.env("PATH", path);
        }
        match command.spawn() {
            Ok(mut child) => {
                return Ok(child
                    .wait()?
                    .code()
                    .and_then(|code| code.try_into().ok())
                    .unwrap_or(1));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no executable candidate found")
    }))
}

fn child_path(program: &str) -> Option<std::ffi::OsString> {
    let mut paths = Vec::new();
    if let Some(parent) = std::path::Path::new(program)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        paths.push(parent.to_path_buf());
    }
    if let Some(cargo) = std::env::var_os("CARGO_HOME") {
        paths.push(std::path::PathBuf::from(cargo).join("bin"));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(std::path::PathBuf::from(home).join(".cargo/bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(paths).ok()
}
