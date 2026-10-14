use bedtime_remote::{
    Request, RequestFrame, Response, ResponseFrame, SearchMatch, read_frame, write_frame,
};
use std::{io, thread, time::Duration};
pub fn run() {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut workspace = String::new();
    let mut initial = true;
    while let Some(frame) = read_frame::<_, RequestFrame>(&mut input).unwrap() {
        let response = if matches!(&frame.request,
            Request::Search {root, ..} | Request::ReadFile {root, ..}
            | Request::WriteFile {root, ..} | Request::Canonicalize {root, ..}
            if root != "/remote/project")
        {
            bedtime_remote::LocalBackend.call(frame.request)
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
                        vec![bedtime_remote::WorkspaceUpdate {
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
