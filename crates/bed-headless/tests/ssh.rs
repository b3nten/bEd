//! Opt-in real SSH validation. The only writes are inside a unique child of
//! BED_TEST_SSH_ROOT, removed when the test finishes.
use bed_remote::{
    RemoteClient, Request, Response, SshTarget, prepare_ssh_target, remote_command, shell_quote,
};
use std::{
    io,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    client: RemoteClient,
    base: String,
    root: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.client.call(Request::Remove {
            root: self.base.clone(),
            path: self.root.clone(),
            is_directory: true,
        });
    }
}

fn git(target: &SshTarget, root: &str, arguments: &[&str]) {
    let mut remote = vec![
        target.agent.clone(),
        "exec".into(),
        "--cwd".into(),
        root.into(),
        "--".into(),
        "git".into(),
    ];
    remote.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    let output = Command::new("ssh")
        .args(["-T", "--", &target.host])
        .arg(remote_command(&remote).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "remote Git: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires BED_TEST_SSH_HOST, BED_TEST_SSH_AGENT and BED_TEST_SSH_ROOT"]
fn ssh_workspace_files_search_and_git() {
    let target = SshTarget {
        host: std::env::var("BED_TEST_SSH_HOST").expect("BED_TEST_SSH_HOST"),
        agent: std::env::var("BED_TEST_SSH_AGENT").expect("BED_TEST_SSH_AGENT"),
    };
    let base = std::env::var("BED_TEST_SSH_ROOT").expect("BED_TEST_SSH_ROOT");
    let client = RemoteClient::launch_ssh(&target).expect("connect to real SSH helper");
    let Response::Path { path: base } = client
        .call(Request::Canonicalize {
            root: base,
            path: ".".into(),
            allow_missing: false,
        })
        .unwrap()
    else {
        panic!()
    };
    let root = format!(
        "{base}/client-smoke-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    client
        .call(Request::CreateDirectory {
            root: base.clone(),
            path: root.clone(),
        })
        .unwrap();
    let fixture = Fixture { client, base, root };
    let original = b"\xef\xbb\xbfhello ssh\r\n";
    let Response::Written { baseline } = fixture
        .client
        .call(Request::WriteFile {
            root: fixture.root.clone(),
            path: "text.txt".into(),
            bytes: original.to_vec(),
            baseline: None,
        })
        .unwrap()
    else {
        panic!()
    };
    let Response::File {
        bytes,
        baseline: read_baseline,
        ..
    } = fixture
        .client
        .call(Request::ReadFile {
            root: fixture.root.clone(),
            path: "text.txt".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(bytes, original);
    assert_eq!(read_baseline, baseline);
    git(&target, &fixture.root, &["init", "-q"]);
    git(&target, &fixture.root, &["add", "text.txt"]);
    git(
        &target,
        &fixture.root,
        &[
            "-c",
            "user.name=Bed",
            "-c",
            "user.email=bed@example.test",
            "commit",
            "-qm",
            "SSH smoke fixture",
        ],
    );
    fixture
        .client
        .call(Request::WriteFile {
            root: fixture.root.clone(),
            path: "text.txt".into(),
            bytes: b"external ssh edit\r\n".to_vec(),
            baseline: Some(baseline.clone()),
        })
        .unwrap();
    assert_eq!(
        fixture
            .client
            .call(Request::WriteFile {
                root: fixture.root.clone(),
                path: "text.txt".into(),
                bytes: b"stale local buffer".to_vec(),
                baseline: Some(baseline)
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let Response::File { bytes, .. } = fixture
        .client
        .call(Request::ReadFile {
            root: fixture.root.clone(),
            path: "text.txt".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(bytes, b"external ssh edit\r\n");
    let Response::Search { matches, .. } = fixture
        .client
        .call(Request::Search {
            root: fixture.root.clone(),
            query: "ssh".into(),
            options: bed_remote::SearchOptions {
                case_sensitive: false,
                include_ignored: false,
                ..Default::default()
            },
            buffer_paths: Vec::new(),
            max_results: 10,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].column, 10);
    assert_eq!(
        fixture
            .client
            .call(Request::GitBaseline {
                root: fixture.root.clone(),
                path: "text.txt".into()
            })
            .unwrap(),
        Response::GitBaseline {
            bytes: Some(original.to_vec())
        }
    );
    let Response::GitStatus { entries } = fixture
        .client
        .call(Request::GitStatus {
            root: fixture.root.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].worktree_status, 'M');
    fixture
        .client
        .call(Request::Rename {
            root: fixture.root.clone(),
            from: "text.txt".into(),
            to: "renamed.txt".into(),
        })
        .unwrap();
    let Response::Files { paths } = fixture
        .client
        .call(Request::ListFiles {
            root: fixture.root.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(paths.iter().any(|path| path.ends_with("/renamed.txt")));
}

#[test]
#[ignore = "requires BED_TEST_SSH_HOST and BED_TEST_SSH_HELPERS_DIR with native Linux helpers"]
fn ssh_managed_helper_first_install_and_cache_reuse() {
    use std::{
        fs,
        io::Write,
        path::{Path, PathBuf},
    };
    struct Cleanup {
        host: String,
        cache: String,
        local: PathBuf,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if !self.cache.is_empty() {
                let executable = shell_quote(&format!("{}/bed-headless", self.cache)).unwrap();
                let directory = shell_quote(&self.cache).unwrap();
                let _ = Command::new("ssh")
                    .args([
                        "-T",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=10",
                        "--",
                        &self.host,
                    ])
                    .arg(format!("rm -f {executable} && rmdir {directory}"))
                    .status();
            }
            let _ = fs::remove_dir_all(&self.local);
        }
    }
    let target = SshTarget::new(std::env::var("BED_TEST_SSH_HOST").expect("BED_TEST_SSH_HOST"));
    let source =
        PathBuf::from(std::env::var("BED_TEST_SSH_HELPERS_DIR").expect("BED_TEST_SSH_HELPERS_DIR"));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let local = std::env::temp_dir().join(format!(
        "bed-live-deployment-{}-{stamp}",
        std::process::id()
    ));
    let mut cleanup = Cleanup {
        host: target.host.clone(),
        cache: String::new(),
        local: local.clone(),
    };
    for triple in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
        let binary = source.join(triple).join("bed-headless");
        if !binary.is_file() {
            continue;
        }
        let destination = local.join(triple).join("bed-headless");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(binary, &destination).unwrap();
        // ELF permits harmless trailing bytes. A unique content address gives
        // this test its own first-install cache, leaving every user cache alone.
        fs::OpenOptions::new()
            .append(true)
            .open(destination)
            .unwrap()
            .write_all(format!("\nbed-live-deployment-{stamp}\n").as_bytes())
            .unwrap();
    }
    let prepared = prepare_ssh_target(&target, &local).unwrap();
    assert!(prepared.agent.contains("/.cache/bed/helpers/"));
    cleanup.cache = Path::new(&prepared.agent)
        .parent()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let inode = |path: &str| {
        let output = Command::new("ssh")
            .args(["-T", "--", &target.host])
            .arg(format!("stat -c %i {}", shell_quote(path).unwrap()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let first_inode = inode(&prepared.agent);
    assert_eq!(prepare_ssh_target(&target, &local).unwrap(), prepared);
    assert_eq!(
        inode(&prepared.agent),
        first_inode,
        "cache reuse replaced the installed executable"
    );
    let client = RemoteClient::launch_ssh(&prepared).unwrap();
    let root = std::env::var("BED_TEST_SSH_ROOT").unwrap_or_else(|_| ".".into());
    assert!(matches!(
        client
            .call(Request::Canonicalize {
                root,
                path: ".".into(),
                allow_missing: false
            })
            .unwrap(),
        Response::Path { .. }
    ));
    client.disconnect();
}
