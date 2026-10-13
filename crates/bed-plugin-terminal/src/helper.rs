//! Dispatch plugin-owned local aliases before starting the native application.
use std::{ffi::OsString, io};

/// Any `+alias` executable forwards opaque arguments to its registered GUI owner.
pub fn run_shell_command(args: impl IntoIterator<Item = OsString>) -> Option<io::Result<()>> {
    let mut args = args.into_iter();
    let executable = args.next()?;
    let alias = std::path::Path::new(&executable).file_name()?.to_str()?;
    if !alias.starts_with('+') {
        return None;
    }
    Some(bed_terminal::shell_bridge::invoke(alias, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn arbitrary_plugin_aliases_forward_opaque_arguments() {
        for name in ["+custom", "/tmp/helpers/+ducky", "+w"] {
            let error = run_shell_command([
                OsString::from(name),
                OsString::from_vec(b"invalid\0argument".to_vec()),
            ])
            .unwrap()
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), "Invalid shell argument bytes");
        }
        for argument in ["open", "workspace", "ducky", "--help"] {
            assert!(
                run_shell_command([OsString::from("example"), OsString::from(argument)]).is_none()
            );
        }
        assert!(run_shell_command([OsString::from("example")]).is_none());
    }
}
