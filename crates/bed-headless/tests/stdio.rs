use bed_remote::{
    ErrorKind, PROTOCOL_VERSION, RemoteClient, Request, RequestFrame, Response, ResponseFrame,
    read_frame, write_frame,
};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed-headless-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn root(&self) -> String {
        self.0.to_str().unwrap().into()
    }
    fn client(&self) -> RemoteClient {
        RemoteClient::launch_local(env!("CARGO_BIN_EXE_bed-headless")).unwrap()
    }
    fn git(&self, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn subprocess_directory_listings_include_git_ignore_metadata() {
    let temp = Temp::new();
    temp.git(&["init", "-q"]);
    fs::write(temp.0.join(".gitignore"), b"*.log\n").unwrap();
    fs::write(temp.0.join("drop.log"), b"").unwrap();
    fs::write(temp.0.join("keep.txt"), b"").unwrap();
    let client = temp.client();
    let Response::Directory { entries, warning } = client
        .call(Request::ReadDirectory {
            root: temp.root(),
            path: ".".into(),
            classify_gitignored: true,
        })
        .unwrap()
    else {
        panic!("expected directory listing")
    };
    assert_eq!(warning, None);
    assert!(
        entries
            .iter()
            .find(|entry| entry.name == "drop.log")
            .unwrap()
            .is_gitignored
    );
    assert!(
        !entries
            .iter()
            .find(|entry| entry.name == "keep.txt")
            .unwrap()
            .is_gitignored
    );
    client.disconnect();
}

#[test]
fn subprocess_file_information_detects_content_and_returns_mime() {
    let temp = Temp::new();
    fs::write(temp.0.join("image.txt"), b"\x89PNG\r\n\x1a\n").unwrap();
    let client = temp.client();
    let Response::FileInfo { info } = client
        .call(Request::FileInfo {
            root: temp.root(),
            path: "image.txt".into(),
        })
        .unwrap()
    else {
        panic!("expected file information");
    };
    assert_eq!(info.file_type, "PNG image");
    assert_eq!(info.mime_type.as_deref(), Some("image/png"));
    assert_eq!(info.size, 8);
    client.disconnect();
}

#[test]
fn subprocess_roundtrip_concurrent_calls_and_disconnect() {
    let temp = Temp::new();
    let client = temp.client();
    client
        .call(Request::CreateDirectory {
            root: temp.root(),
            path: "folder".into(),
        })
        .unwrap();
    let bytes = b"\xef\xbb\xbfhello\r\n";
    let Response::Written { baseline } = client
        .call(Request::WriteFile {
            root: temp.root(),
            path: "folder/a".into(),
            bytes: bytes.to_vec(),
            baseline: None,
        })
        .unwrap()
    else {
        panic!()
    };
    let mut workers = Vec::new();
    for _ in 0..4 {
        let client = client.clone();
        let root = temp.root();
        workers.push(std::thread::spawn(move || {
            client
                .call(Request::ReadFile {
                    root,
                    path: "folder/a".into(),
                })
                .unwrap()
        }));
    }
    for worker in workers {
        let Response::File {
            bytes: read,
            baseline: read_baseline,
            ..
        } = worker.join().unwrap()
        else {
            panic!()
        };
        assert_eq!(read, bytes);
        assert_eq!(read_baseline, baseline);
    }
    client
        .call(Request::Rename {
            root: temp.root(),
            from: "folder/a".into(),
            to: "folder/b".into(),
        })
        .unwrap();
    assert!(temp.0.join("folder/b").is_file());
    client
        .call(Request::Remove {
            root: temp.root(),
            path: "folder".into(),
            is_directory: true,
        })
        .unwrap();
    client.disconnect();
    assert!(!client.is_connected());
    assert_eq!(
        client
            .call(Request::ListFiles { root: temp.root() })
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[test]
fn subprocess_version_negotiation_and_eof_exit() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bed-headless"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    write_frame(
        &mut input,
        &RequestFrame {
            id: 72,
            request: Request::Hello {
                version: PROTOCOL_VERSION + 1,
            },
        },
    )
    .unwrap();
    let frame: ResponseFrame = read_frame(&mut output).unwrap().unwrap();
    assert_eq!(frame.id, 72);
    assert_eq!(
        frame.response.unwrap_err().kind,
        ErrorKind::UnsupportedVersion
    );
    write_frame(
        &mut input,
        &RequestFrame {
            id: 73,
            request: Request::Hello {
                version: PROTOCOL_VERSION,
            },
        },
    )
    .unwrap();
    let frame: ResponseFrame = read_frame(&mut output).unwrap().unwrap();
    assert_eq!(frame.id, 73);
    assert_eq!(
        frame.response.unwrap(),
        Response::Hello {
            version: PROTOCOL_VERSION
        }
    );
    drop(input);
    assert!(child.wait().unwrap().success());
    assert!(
        read_frame::<_, ResponseFrame>(&mut output)
            .unwrap()
            .is_none()
    );
}

#[test]
fn subprocess_search_preserves_editor_byte_semantics_and_ignore_toggle() {
    let temp = Temp::new();
    temp.git(&["init", "-q"]);
    fs::write(temp.0.join(".gitignore"), "ignored\n").unwrap();
    fs::write(
        temp.0.join("text"),
        b"\xef\xbb\xbf\xc3\xa9 MATCH\r\nnext\rmatch\n",
    )
    .unwrap();
    fs::write(temp.0.join("ignored"), b"match").unwrap();
    let client = temp.client();
    let Response::Search {
        matches,
        ignored_paths,
        scanned_files,
        ..
    } = client
        .call(Request::Search {
            root: temp.root(),
            query: "match".into(),
            options: bed_remote::SearchOptions {
                case_sensitive: false,
                include_ignored: false,
                ..Default::default()
            },
            buffer_paths: Vec::new(),
            max_results: 100,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert_eq!(
        (matches[0].line, matches[0].column, matches[0].editor_row),
        (1, 4, 1)
    );
    assert_eq!(matches[0].line_bytes, "é MATCH".as_bytes());
    assert_eq!(matches[1].line, 3);
    assert!(ignored_paths > 0);
    assert!(scanned_files > 0);
    let Response::Search { matches, .. } = client
        .call(Request::Search {
            root: temp.root(),
            query: "match".into(),
            options: bed_remote::SearchOptions {
                case_sensitive: false,
                include_ignored: true,
                ..Default::default()
            },
            buffer_paths: Vec::new(),
            max_results: 100,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(matches.len(), 3);
}

#[test]
fn subprocess_git_baseline_and_status_handle_subdirectory_roots() {
    let temp = Temp::new();
    temp.git(&["init", "-q"]);
    fs::create_dir(temp.0.join("src")).unwrap();
    fs::write(temp.0.join("src/a"), b"committed").unwrap();
    temp.git(&["add", "src/a"]);
    temp.git(&[
        "-c",
        "user.name=Bed",
        "-c",
        "user.email=bed@example.test",
        "commit",
        "-qm",
        "fixture",
    ]);
    fs::write(temp.0.join("src/a"), b"changed").unwrap();
    let root = temp.0.join("src").to_str().unwrap().to_owned();
    let client = temp.client();
    let Response::FileInfo { info } = client
        .call(Request::FileInfo {
            root: root.clone(),
            path: "a".into(),
        })
        .unwrap()
    else {
        panic!("expected file metadata over stdio")
    };
    assert_eq!(info.size, 7);
    assert_eq!(info.git.as_deref(), Some("Working tree: modified"));
    assert!(info.modified_unix_seconds.is_some());
    assert_eq!(
        client
            .call(Request::GitBaseline {
                root: root.clone(),
                path: "a".into()
            })
            .unwrap(),
        Response::GitBaseline {
            bytes: Some(b"committed".to_vec())
        }
    );
    let Response::GitStatus { entries } = client.call(Request::GitStatus { root }).unwrap() else {
        panic!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, temp.0.join("src/a").to_str().unwrap());
    assert_eq!(entries[0].worktree_status, 'M');
}

#[cfg(unix)]
#[test]
fn exec_forwards_cwd_stdio_and_falls_back_between_candidates() {
    let temp = Temp::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bed-headless"))
        .args([
            "exec",
            "--cwd",
            &temp.root(),
            "--candidate",
            "/bed-no-such-program",
            "--candidate",
            "/bin/sh",
            "--",
            "-c",
            "pwd; cat; exit 7",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"literal input\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\nliteral input\n", temp.root())
    );
}

#[test]
fn subprocess_regex_filters_and_buffer_eligibility_share_project_policy() {
    let temp = Temp::new();
    temp.git(&["init", "-q"]);
    fs::write(temp.0.join(".gitignore"), b"ignored.rs\n").unwrap();
    fs::write(temp.0.join("a.rs"), b"value=1").unwrap();
    fs::write(temp.0.join("b.rs"), b"value=22").unwrap();
    fs::write(temp.0.join("skip.rs"), b"value=33").unwrap();
    fs::write(temp.0.join("ignored.rs"), b"value=99").unwrap();
    let client = temp.client();
    let buffer = temp.0.join("a.rs").to_str().unwrap().to_owned();
    let Response::Search {
        matches,
        eligible_buffer_paths,
        ..
    } = client
        .call(Request::Search {
            root: temp.root(),
            query: r"value=(\d+)".into(),
            options: bed_remote::SearchOptions {
                regex: true,
                include: "*.rs".into(),
                exclude: "skip.rs".into(),
                ..Default::default()
            },
            buffer_paths: vec![
                buffer.clone(),
                temp.0.join("ignored.rs").to_str().unwrap().into(),
            ],
            max_results: 100,
        })
        .unwrap()
    else {
        panic!("expected search results")
    };
    assert_eq!(eligible_buffer_paths, vec![buffer]);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].range, 0..8);
    assert!(matches[0].path.ends_with("/b.rs"));
    client.disconnect();
}

#[cfg(unix)]
#[test]
fn project_checks_do_not_block_file_requests_and_cancel_over_protocol() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let temp = Temp::new();
    fs::write(temp.0.join("file.txt"), b"saved bytes").unwrap();
    let client = temp.client();
    let Response::CheckStarted { job_id } = client
        .call(Request::CheckStart {
            root: temp.root(),
            program: "sh".into(),
            arguments: vec!["-c".into(), "touch ready; sleep 30 & wait".into()],
            directory: ".".into(),
        })
        .unwrap()
    else {
        panic!("Expected check job")
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !temp.0.join("ready").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        client
            .call(Request::ReadFile {
                root: temp.root(),
                path: "file.txt".into()
            })
            .unwrap(),
        Response::File { .. }
    ));
    client.call(Request::CheckCancel { job_id }).unwrap();
    loop {
        match client.call(Request::CheckPoll { job_id }).unwrap() {
            Response::CheckJob {
                result: Some(result),
            } => {
                assert!(result.unwrap_err().contains("canceled"));
                break;
            }
            Response::CheckJob { result: None } => {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            _ => panic!("Unexpected check response"),
        }
    }
    client.call(Request::CheckRelease { job_id }).unwrap();
    assert!(client.call(Request::CheckPoll { job_id }).is_err());
    client.disconnect();
}
