//! Foreground command names, read on the PTY worker rather than the UI thread.

pub(crate) fn program_name(program: &str) -> String {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .trim_start_matches('-');
    name.strip_suffix(".exe").unwrap_or(name).to_owned()
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn is_interpreter(name: &str) -> bool {
    matches!(name, "node" | "nodejs" | "ruby" | "perl" | "php") || name.starts_with("python")
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn command_name(name: &str, arguments: &[String]) -> String {
    let name = program_name(name);
    // JavaScript/Python command launchers retain the interpreter as their OS
    // process name. Name those sessions after the script (for example codex).
    if is_interpreter(&name) {
        let mut arguments = arguments.iter().skip(1);
        while let Some(argument) = arguments.next() {
            if matches!(
                argument.as_str(),
                "-e" | "-p" | "-c" | "-m" | "--eval" | "--print"
            ) {
                break;
            }
            if matches!(
                argument.as_str(),
                "-r" | "--require" | "--loader" | "--import" | "-W" | "-X"
            ) {
                arguments.next();
                continue;
            }
            if argument.starts_with('-') {
                continue;
            }
            let script = program_name(argument);
            return script
                .strip_suffix(".js")
                .or_else(|| script.strip_suffix(".mjs"))
                .or_else(|| script.strip_suffix(".cjs"))
                .or_else(|| script.strip_suffix(".py"))
                .unwrap_or(&script)
                .to_owned();
        }
    }
    name
}

#[cfg(target_os = "macos")]
pub(crate) fn process_name(pid: libc::pid_t) -> Option<String> {
    let mut name = [0u8; 1024];
    // SAFETY: proc_name writes at most the supplied buffer length.
    let length = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    if length <= 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&name[..length as usize]);
    let name = name.trim_end_matches('\0');
    let arguments = if is_interpreter(name) {
        process_arguments(pid).unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(command_name(name, &arguments))
}

#[cfg(target_os = "macos")]
fn process_arguments(pid: libc::pid_t) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut length = 0usize;
    // SAFETY: the MIB and length pointer are valid; the first query only measures.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length <= std::mem::size_of::<libc::c_int>()
        || length > 1024 * 1024
    {
        return None;
    }
    let mut bytes = vec![0u8; length];
    // SAFETY: sysctl receives an allocated buffer with its exact capacity.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    bytes.truncate(length);
    macos_arguments(&bytes)
}

#[cfg(any(target_os = "macos", test))]
fn macos_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    let header: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    let count = i32::from_ne_bytes(header);
    if !(1..=4096).contains(&count) {
        return None;
    }
    // KERN_PROCARGS2 contains argc, executable path, NUL padding, argv, env.
    let bytes = bytes.get(4..)?;
    let path_end = bytes.iter().position(|byte| *byte == 0)?;
    let bytes = bytes.get(path_end + 1..)?;
    let argv_start = bytes.iter().position(|byte| *byte != 0)?;
    Some(
        bytes[argv_start..]
            .split(|byte| *byte == 0)
            .take(count as usize)
            .map(|value| String::from_utf8_lossy(value).into_owned())
            .collect(),
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn process_name(pid: libc::pid_t) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let arguments = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|value| !value.is_empty())
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(command_name(name.trim_end(), &arguments))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
pub(crate) fn process_name(_: libc::pid_t) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn native_argv_lookup_reads_current_process_arguments() {
        assert_eq!(
            process_arguments(std::process::id() as libc::pid_t).unwrap(),
            std::env::args().collect::<Vec<_>>()
        );
    }

    #[test]
    fn command_names_use_launchers_and_preserve_native_process_names() {
        assert_eq!(program_name("/bin/-zsh"), "zsh");
        assert_eq!(program_name("C:\\Windows\\cmd.exe"), "cmd");
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            command_name("node", &args(&["node", "/opt/bin/codex"])),
            "codex"
        );
        assert_eq!(
            command_name(
                "node",
                &args(&["node", "--require", "setup", "/opt/bin/tool.js"])
            ),
            "tool"
        );
        assert_eq!(
            command_name("python3", &args(&["python3", "-c", "print('hi')"])),
            "python3"
        );
        assert_eq!(
            command_name("zsh", &args(&["-zsh", "+o", "PROMPT_SP"])),
            "zsh"
        );
    }

    #[test]
    fn macos_argv_skips_executable_padding_and_environment() {
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend(b"/usr/bin/node\0\0\0node\0/opt/bin/codex\0\0TOKEN=private\0");
        assert_eq!(
            macos_arguments(&bytes).unwrap(),
            ["node", "/opt/bin/codex", ""]
        );
        assert!(macos_arguments(&[]).is_none());
    }
}
