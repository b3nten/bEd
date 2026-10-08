//! Exercise service workers against a real framed subprocess without SSH setup.
use bed_files::{content_search::ContentSearch, file_finder::FileFinder};
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
        let response = match frame.request {
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
                include_ignored,
                ..
            } => {
                if query == "slow" {
                    thread::sleep(Duration::from_secs(2));
                }
                Response::Search {
                    matches: vec![SearchMatch {
                        path: format!(
                            "{root}/{}file.rs",
                            if include_ignored { "ignored/" } else { "" }
                        ),
                        line: 2,
                        editor_row: 5,
                        column: 3,
                        text: "� needle".into(),
                        line_bytes: b"\xff needle".to_vec(),
                    }],
                    truncated: false,
                    scanned_files: 3,
                    discovered_files: 3,
                    ignored_paths: 2,
                    skipped_files: 1,
                }
            }
            other => panic!("unexpected fixture request: {other:?}"),
        };
        if write_frame(
            &mut output,
            &ResponseFrame {
                id: frame.id,
                response: Ok(response),
            },
        )
        .is_err()
        {
            break;
        }
    }
}
