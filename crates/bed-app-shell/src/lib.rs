//! Private shell helper dispatch for ordinary bEd terminal panels.
use std::{ffi::OsString, io, path::PathBuf};

pub fn open_from_shell(args: impl IntoIterator<Item = OsString>) -> io::Result<()> {
    bed_terminal::shell_bridge::open(args.into_iter().map(PathBuf::from).collect())
}

/// Dispatch private shell helpers, or explicit subcommands when enabled.
/// The application uses this before starting the native host.
pub fn run_shell_command(
    args: impl IntoIterator<Item = OsString>,
    accept_subcommands: bool,
) -> Option<io::Result<()>> {
    let mut args = args.into_iter();
    let executable = args.next()?;
    let command = match std::path::Path::new(&executable).file_name()?.to_str()? {
        "+open" | "+o" => "open",
        "+workspace" | "+w" => "workspace",
        "+ducky" => "ducky",
        _ if accept_subcommands => match args.next()?.to_str()? {
            "open" => "open",
            "workspace" => "workspace",
            _ => return None,
        },
        _ => return None,
    };
    Some(match command {
        "open" => open_from_shell(args),
        "workspace" => {
            if args.next().is_some() {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: +workspace (opens the calling shell's current directory)",
                ))
            } else {
                bed_terminal::shell_bridge::workspace()
            }
        }
        "ducky" => {
            if args.next().is_some() {
                Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: +ducky"))
            } else {
                bed_terminal::shell_bridge::ducky()
            }
        }
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ducky_helper_takes_no_arguments_and_remains_a_private_command() {
        let error = run_shell_command(["/tmp/helpers/+ducky", "file"].map(OsString::from), false)
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "usage: +ducky");
        assert!(run_shell_command(["bed", "ducky"].map(OsString::from), true).is_none());
    }

    #[test]
    fn open_helpers_share_argument_validation_and_workspace_takes_no_arguments() {
        for name in ["/tmp/helpers/+open", "/tmp/helpers/+o"] {
            let error = run_shell_command([OsString::from(name)], false)
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("open requires"));
        }
        for args in [
            vec!["+workspace", "file"],
            vec!["/tmp/helpers/+w", "file"],
            vec!["bed", "workspace", "file"],
        ] {
            let error = run_shell_command(args.into_iter().map(OsString::from), true)
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("usage: +workspace"));
        }
        for (args, accept_subcommands) in [
            (vec!["bed", "open"], false),
            (vec!["bed", "workspace"], false),
            (vec!["+upgrade"], false),
            (vec!["+u"], false),
            (vec!["bed", "upgrade"], true),
            (vec!["bed", "--help"], true),
            (vec!["bed"], true),
        ] {
            assert!(
                run_shell_command(args.into_iter().map(OsString::from), accept_subcommands)
                    .is_none()
            );
        }
    }
}
