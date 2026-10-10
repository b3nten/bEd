//! Read a shell process's current directory without touching its input or cwd.
//!
//! This queries the live process, rather than its saved launch directory or a
//! cached `$PWD`, so shell `cd` commands are visible immediately to Promote.

use std::{io, path::PathBuf};

pub fn for_process(pid: u32) -> io::Result<PathBuf> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid shell process ID",
        ));
    }
    let path = process_directory(pid).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Could not read current directory of shell process {pid}: {error}"),
        )
    })?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Shell current directory is not an absolute path",
        ));
    }
    if !path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Shell current directory is no longer available: {}",
                path.display()
            ),
        ));
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn process_directory(pid: u32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
}

#[cfg(target_os = "macos")]
fn process_directory(pid: u32) -> io::Result<PathBuf> {
    use std::{ffi::OsString, mem::MaybeUninit, os::unix::ffi::OsStringExt};

    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    // SAFETY: libc defines this structure to match PROC_PIDVNODEPATHINFO's SDK
    // ABI. The buffer is correctly sized and initialized before the query.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        )
    };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize != size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Process directory query returned incomplete information",
        ));
    }
    // SAFETY: the successful call above populated the complete structure.
    let info = unsafe { info.assume_init() };
    // libc represents MAXPATHLEN as 32 arrays of 32 chars for compatibility.
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .into_iter()
        .flatten()
        .map(|byte| byte as u8)
        .collect();
    let end = bytes.iter().position(|&byte| byte == 0).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Shell current directory path was truncated",
        )
    })?;
    if end == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Shell process has no current directory; it may have exited",
        ));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes[..end].to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Child, Command, Stdio},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bedterm-live-cwd-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn current_process_directory_matches_the_operating_system() {
        assert_eq!(
            for_process(std::process::id()).unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        assert_eq!(
            for_process(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn follows_a_running_shell_cd_and_fails_after_exit() {
        let directory = Directory::new();
        let initial = directory.0.join("initial");
        let target = directory.0.join("target with spaces");
        std::fs::create_dir(&initial).unwrap();
        std::fs::create_dir(&target).unwrap();
        // Pipes synchronize the parent with each builtin cd. The path is an
        // argument, never interpolated shell syntax, and no user rc files run.
        let mut shell = Process(Command::new("/bin/sh")
            .args(["-c", "printf 'ready\\n'; IFS= read -r line; cd \"$1\" || exit 1; printf 'changed\\n'; IFS= read -r line", "bedterm-cwd-test"])
            .arg(&target)
            .current_dir(&initial)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn().unwrap());
        let pid = shell.0.id();
        let mut output = BufReader::new(shell.0.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        assert_eq!(for_process(pid).unwrap(), initial.canonicalize().unwrap());
        shell.0.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "changed\n");
        assert_eq!(for_process(pid).unwrap(), target.canonicalize().unwrap());
        shell.0.kill().unwrap();
        shell.0.wait().unwrap();
        assert!(
            for_process(pid).is_err(),
            "An exited shell must never return its launch directory"
        );
    }
}
