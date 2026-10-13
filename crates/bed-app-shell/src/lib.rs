//! Dispatch local shell invocations before a host starts its native application.
//! Plugin arguments are interpreted only by their registered GUI-side owner.
use std::{ffi::OsString, io};

pub fn open_from_shell(args: impl IntoIterator<Item = OsString>) -> io::Result<()> {
    bed_terminal::shell_bridge::invoke("+open", args)
}

/// Any safe `+alias` executable forwards to the private command socket. Historic
/// `bed open` and `bed workspace` entrypoints retain their existing meaning.
pub fn run_shell_command(
    args: impl IntoIterator<Item = OsString>,
    accept_subcommands: bool,
) -> Option<io::Result<()>> {
    let mut args = args.into_iter();
    let executable = args.next()?;
    let name = std::path::Path::new(&executable).file_name()?.to_str()?;
    let alias = if name.starts_with('+') {
        name.to_owned()
    } else if accept_subcommands {
        match args.next()?.to_str()? {
            "open" => "+open".into(),
            "workspace" => "+workspace".into(),
            _ => return None,
        }
    } else {
        return None;
    };
    Some(bed_terminal::shell_bridge::invoke(&alias, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn arbitrary_plugin_aliases_dispatch_and_argument_semantics_belong_to_the_owner() {
        for name in ["+custom", "/tmp/helpers/+ducky", "+w"] {
            let error = run_shell_command(
                [
                    OsString::from(name),
                    OsString::from_vec(b"invalid\0argument".to_vec()),
                ],
                false,
            )
            .unwrap()
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), "Invalid shell argument bytes");
        }
        assert!(
            run_shell_command([OsString::from("bed"), OsString::from("ducky")], true).is_none()
        );
        assert!(
            run_shell_command([OsString::from("bed"), OsString::from("--help")], true).is_none()
        );
    }
    #[test]
    fn legacy_entrypoints_forward_to_registered_aliases_and_gui_flags_are_untouched() {
        for command in ["open", "workspace"] {
            let error = run_shell_command(
                [
                    OsString::from("bed"),
                    OsString::from(command),
                    OsString::from_vec(b"arg\0".to_vec()),
                ],
                true,
            )
            .unwrap()
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(
                run_shell_command([OsString::from("bed"), OsString::from(command)], false)
                    .is_none()
            );
        }
        assert!(run_shell_command([OsString::from("bed")], true).is_none());
    }
}
