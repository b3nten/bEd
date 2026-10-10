//! Exercise service workers against a real framed subprocess without SSH setup.
use bed_files::{
    content_search::{ContentSearch, SearchBuffer},
    file_finder::FileFinder,
};
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;
use bed_remote::{
    RemoteClient, Request, RequestFrame, Response, ResponseFrame, SearchMatch, read_frame,
    write_frame,
};
use std::{
    io, thread,
    time::{Duration, Instant},
};

fn main() {
    if std::env::args().any(|argument| argument == "--stdio") {
        serve_fixture();
        return;
    }
    let client = RemoteClient::launch_local(std::env::current_exe().unwrap()).unwrap();
    let mut finder = FileFinder::new();
    finder.set_remote_client(Some(client.clone()));
    finder.set_project_dir("/remote/project");
    finder.start_background_thread();
    wait_until(|| finder.poll());
    finder.toggle_window();
    assert_eq!(finder.filtered_list[0].full_path, "/remote/project/file.rs");
    assert_eq!(finder.filtered_list[0].relative_path, "file.rs");

    let mut search = ContentSearch::new_lazy();
    search.set_remote_client(Some(client.clone()));
    search.include_ignored = true;
    search.start("/remote/project", "needle", false);
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_none(), "{:?}", search.error);
    assert_eq!(search.results.len(), 1);
    assert_eq!(search.progress.scanned_files, 3);
    assert_eq!(search.progress.discovered_files, 3);
    assert_eq!(search.progress.ignored_paths, 2);
    assert_eq!(search.skipped_files, 1);
    let found = &search.results[0];
    assert_eq!((found.source_row, found.row, found.column), (1, 4, 2));
    assert_eq!(&*found.line, b"\xff needle");
    assert_eq!(found.file.relative_path, "ignored/file.rs");

    let temp = test_support::TempDir::new();
    let root = std::fs::canonicalize(temp.root()).unwrap();
    git2::Repository::init(&root).unwrap();
    temp.write(".gitignore", b"ignored.rs\n");
    let path = temp.write("src/main.rs", b"value=1");
    temp.write("other.rs", b"value=22");
    temp.write("skip.rs", b"value=33");
    temp.write("ignored.rs", b"value=99");
    let buffers = vec![SearchBuffer {
        path: std::fs::canonicalize(path)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
        bytes: Some(std::sync::Arc::from(b"value=42".as_slice())),
    }];
    search.include_ignored = false;
    search.regex = true;
    search.include = "*.rs".into();
    search.exclude = "skip.rs".into();
    search.start_with_buffers(root.to_str().unwrap(), r"value=(\d+)", buffers.clone());
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_none(), "{:?}", search.error);
    assert_eq!(search.results.len(), 2);
    let mut local = ContentSearch::new_lazy();
    local.regex = true;
    local.include = "*.rs".into();
    local.exclude = "skip.rs".into();
    local.start_with_buffers(root.to_str().unwrap(), r"value=(\d+)", buffers);
    wait_until(|| {
        local.poll();
        !local.searching
    });
    assert_eq!(local.results, search.results);
    assert!(
        search
            .results
            .iter()
            .any(|found| &*found.line == b"value=42")
    );
    search.start(root.to_str().unwrap(), "[", false);
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_some());
    search.start("/remote/project", "slow", true);
    thread::sleep(Duration::from_millis(50));
    search.cancel();
    assert!(!search.searching);
    let started = Instant::now();
    drop(search);
    drop(finder);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "remote workers blocked consumer teardown"
    );
    client.disconnect();
    println!(
        "remote finder, byte positions, ignored-file routing, cancellation and teardown passed"
    );
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}

fn serve_fixture() {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut workspace = String::new();
    let mut initial = true;
    while let Some(frame) = read_frame::<_, RequestFrame>(&mut input).unwrap() {
        let response = if matches!(&frame.request, Request::Search {root, ..} if root != "/remote/project")
        {
            bed_remote::LocalBackend.call(frame.request)
        } else {
            Ok(match frame.request {
                Request::Hello { version } => Response::Hello { version },
                Request::ListFiles { root } => Response::Files {
                    paths: vec![format!("{root}/file.rs")],
                },
                Request::WatchWorkspace { root, .. } => {
                    workspace = root;
                    initial = true;
                    Response::WorkspaceWatch { watch_id: 1 }
                }
                Request::PollWorkspace { .. } => {
                    let updates = if initial {
                        initial = false;
                        vec![bed_remote::WorkspaceUpdate {
                            root: workspace.clone(),
                            generation: 1,
                            indexed_files: Some(vec![format!("{workspace}/file.rs")]),
                            ready: true,
                            ..Default::default()
                        }]
                    } else {
                        vec![]
                    };
                    Response::WorkspaceUpdates { updates }
                }
                Request::RefreshWorkspace { .. } => {
                    initial = true;
                    Response::Unit
                }
                Request::WatchDirectory { .. }
                | Request::RefreshWorkspaceDirectory { .. }
                | Request::UnwatchWorkspace { .. } => Response::Unit,
                Request::Search {
                    root,
                    query,
                    options,
                    ..
                } => {
                    if query == "slow" {
                        thread::sleep(Duration::from_secs(2));
                    }
                    Response::Search {
                        matches: vec![SearchMatch {
                            path: format!(
                                "{root}/{}file.rs",
                                if options.include_ignored {
                                    "ignored/"
                                } else {
                                    ""
                                }
                            ),
                            line: 2,
                            editor_row: 5,
                            column: 3,
                            text: "� needle".into(),
                            range: 2..8,
                            line_bytes: b"\xff needle".to_vec(),
                        }],
                        truncated: false,
                        scanned_files: 3,
                        discovered_files: 3,
                        ignored_paths: 2,
                        skipped_files: 1,
                        eligible_buffer_paths: Vec::new(),
                    }
                }
                other => panic!("unexpected fixture request: {other:?}"),
            })
        };
        if write_frame(
            &mut output,
            &ResponseFrame {
                id: frame.id,
                response,
            },
        )
        .is_err()
        {
            break;
        }
    }
}
