//! Real remote protocol completions through the same permanent-resource editor app.
use bed_document_session::{ByteEdit, ClosePolicy, EditorSession, SessionOptions};
use bed_editor_api::{EditorPanelBinding, PanelTarget};
use bed_editor_workbench::*;
use bed_remote::{LocalBackend, RemoteClient, Request, SshTarget, serve_with};
use bevy::prelude::*;
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
mod support;
use support::TempDir;

fn remote_app(directory: &TempDir, restore: bool) -> App {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.arg("--stdio");
    let client = RemoteClient::launch_command(child).unwrap();
    let root = fs::canonicalize(directory.root()).unwrap();
    let session = EditorSession::with_remote_options(
        SessionOptions {
            autosave: None,
            git: false,
            highlighting: false,
            monitoring: false,
            ..Default::default()
        },
        SshTarget::new("fixture"),
        client,
        root.to_string_lossy().into_owned(),
    )
    .unwrap();
    let mut settings = bed_config::Settings::with_paths(
        directory.path("config"),
        bed_config::Settings::get_app_resources_path(),
    )
    .unwrap();
    settings.settings["autosave"] = json!(false);
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    let mut app = App::new();
    app.world_mut().insert_non_send(session);
    app.insert_non_send(bed_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(
            WorkspaceStore::load(&settings.config_dir).unwrap(),
        ))
        .insert_resource(RemoteHelperDirectory(std::path::PathBuf::new()))
        .insert_non_send(settings);
    app.add_plugins((
        bed_runtime::BedRuntimePlugin,
        EditorWorkbenchPlugin {
            options: EditorWorkbenchOptions {
                working_directory: Some(root),
                restore_standalone: restore,
                ..Default::default()
            },
        },
        bed_editor::EditorPlugin,
    ));
    app.update();
    app
}
fn submit(app: &mut App, action: EditorAction, source: Option<Entity>) -> EditorRequestId {
    let request =
        capture_editor_request_from(app.world(), action, PanelTarget::LastFocused, source);
    submit_editor_request(app.world_mut(), request)
}
fn wait(app: &mut App, mut done: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.update();
        if done(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Remote editor did not complete: {:?}",
            app.world().resource::<EditorWorkspace>().error
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn outcome(app: &mut App, id: EditorRequestId) -> EditorRequestOutcome {
    let mut result = None;
    wait(app, |app| {
        result = poll_editor_request_result(app.world_mut(), id);
        result.is_some()
    });
    result.unwrap().unwrap()
}
fn open(app: &mut App, path: &Path, area: u32) -> Entity {
    let id = request_open(app, path, area);
    app.update();
    assert!(
        poll_editor_request_result(app.world_mut(), id).is_none(),
        "Opening must await real read completion"
    );
    match outcome(app, id) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    }
}
fn request_open(app: &mut App, path: &Path, area: u32) -> EditorRequestId {
    let mut request = capture_editor_request(
        app.world(),
        EditorAction::OpenFileWithViewer {
            path: path.to_owned(),
            viewer: None,
            additional: false,
            move_existing: false,
        },
        PanelTarget::LastFocused,
    );
    request.area = area;
    submit_editor_request(app.world_mut(), request)
}
fn shutdown(app: &mut App) {
    app.world_mut()
        .non_send_mut::<EditorSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}

fn captured_destination_and_cancellation() {
    let directory = TempDir::new();
    let path = directory.write("arrival.txt", b"remote arrival");
    let canceled = directory.write("canceled.txt", b"must not acquire a panel");
    let mut app = remote_app(&directory, false);
    let mut layout = bed_workbench::workspace::tiling::Layout::default();
    let area = layout.split(1, 0, 5000, 1).unwrap();
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling =
        bed_workbench::workspace::tiling_state::TilingState::new(layout);
    let id = request_open(&mut app, &path, area);
    app.update();
    let focus = submit(&mut app, EditorAction::FocusArea(1), None);
    app.update();
    assert!(matches!(
        outcome(&mut app, focus),
        EditorRequestOutcome::Applied
    ));
    let panel = match outcome(&mut app, id) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let saved_id = app.world().get::<EditorPanel>(panel).unwrap().id;
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(saved_id),
        Some(area)
    );
    let document = app
        .world()
        .get::<EditorPanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<EditorSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"remote arrival"
    );
    let view = app.world().get::<EditorPanelBinding>(panel).unwrap().view;
    let mut request = capture_editor_request(
        app.world(),
        EditorAction::OpenFileWithViewer {
            path: path.clone(),
            viewer: None,
            additional: false,
            move_existing: true,
        },
        PanelTarget::LastFocused,
    );
    request.area = 1;
    let moved = submit_editor_request(app.world_mut(), request);
    assert!(
        matches!(outcome(&mut app,moved),EditorRequestOutcome::Opened{panel:opened,..} if opened==panel)
    );
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(saved_id),
        Some(1)
    );
    assert_eq!(
        app.world().get::<EditorPanelBinding>(panel).unwrap().view,
        view
    );
    assert_eq!(
        app.world().non_send::<EditorSession>().view_count(document),
        1
    );

    let id = request_open(&mut app, &canceled, area);
    app.update();
    assert!(poll_editor_request_result(app.world_mut(), id).is_none());
    let mut tiling = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let plan = tiling.layout.plan_join(1, area, [1; 2]).unwrap();
    tiling.remove(saved_id);
    tiling.apply_plan(plan);
    let request = submit(
        &mut app,
        EditorAction::WorkbenchAction(bed_workbench_api::Action::SetLayout {
            tiling: tiling.to_value([]),
            close: vec![panel],
        }),
        None,
    );
    outcome(&mut app, request);
    let mut error = None;
    wait(&mut app, |app| {
        error = poll_editor_request_result(app.world_mut(), id);
        error.is_some()
    });
    assert_eq!(
        error.unwrap().unwrap_err().kind,
        std::io::ErrorKind::NotFound
    );
    wait(&mut app, |app| {
        !app.world()
            .non_send::<EditorSession>()
            .open_pending(&canceled)
    });
    assert!(panel_entities(app.world_mut()).is_empty());
    assert!(
        app.world()
            .non_send::<EditorSession>()
            .document_for_path(&canceled)
            .is_none(),
        "A canceled clean arrival must release its unused document"
    );
    shutdown(&mut app);
}

fn restoration_keeps_reserved_identity_and_group_selection() {
    for (selected, bytes) in [(41, false), (42, false), (41, true), (42, true)] {
        let directory = TempDir::new();
        let path = directory.write("shared.txt", b"shared remote document");
        let mut store = WorkspaceStore::load(&directory.path("config")).unwrap();
        let mut tiling = bed_workbench::workspace::tiling_state::TilingState::default();
        tiling.sync_area(1, vec![41, 42], Some(selected));
        let (panel_type, viewer) = if bytes {
            (bed_editor::HEX_PANEL_TYPE, bed_editor::HEX_VIEWER)
        } else {
            (bed_editor::TEXT_PANEL_TYPE, bed_editor::TEXT_VIEWER)
        };
        store.save_session_layout(None,json!({"panels":[{"id":41,"panel_type":panel_type,"viewer":viewer,"document_kind":if bytes {"bytes"}else{"text"},"path":path,"state":{}},{"id":42,"panel_type":panel_type,"viewer":viewer,"document_kind":if bytes {"bytes"}else{"text"},"path":path,"state":{}}],"tiling":tiling.to_value([41,42]),"focused":selected,"active_document_panel":selected})).unwrap();
        let mut app = remote_app(&directory, true);
        let tabs = panel_entities(app.world_mut());
        assert_eq!(tabs.len(), 2);
        let captured = tabs
            .iter()
            .map(|tab| (*tab, app.world().get::<EditorPanel>(*tab).unwrap().id))
            .collect::<Vec<_>>();
        wait(&mut app, |app| {
            tabs.iter().all(|tab| {
                app.world()
                    .get::<EditorPanelBinding>(*tab)
                    .is_some_and(|binding| binding.document.is_some())
                    && panel_presentation(app.world(), *tab).is_some_and(|surface| {
                        app.world()
                            .get::<bed_editor::EditorPanelState>(surface)
                            .is_some()
                    })
            })
        });
        let group = &app.world().resource::<WorkbenchLayout>().tiling.areas[0];
        assert_eq!(group.tabs, vec![41, 42]);
        assert_eq!(group.selected, Some(selected));
        for (tab, id) in captured {
            assert_eq!(app.world().get::<EditorPanel>(tab).unwrap().id, id);
        }
        let document = app
            .world()
            .get::<EditorPanelBinding>(tabs[0])
            .unwrap()
            .document
            .unwrap();
        assert_eq!(
            app.world()
                .get::<EditorPanelBinding>(tabs[1])
                .unwrap()
                .document,
            Some(document)
        );
        assert_eq!(
            app.world().non_send::<EditorSession>().view_count(document),
            if bytes { 0 } else { 2 }
        );
        assert_eq!(
            app.world()
                .non_send::<EditorSession>()
                .document_kind(document)
                .unwrap(),
            if bytes {
                bed_document_session::DocumentKind::Bytes
            } else {
                bed_document_session::DocumentKind::Text
            }
        );
        let focus = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        assert_eq!(app.world().get::<EditorPanel>(focus).unwrap().id, selected);
        assert_eq!(
            app.world().resource::<EditorNavigation>().active_view,
            app.world().get::<EditorPanelBinding>(focus).unwrap().view
        );
        shutdown(&mut app);
    }
}

fn save_and_close_receipts_wait_for_acknowledged_bytes() {
    let directory = TempDir::new();
    let path = directory.write("save.txt", b"original");
    let mut app = remote_app(&directory, false);
    let panel = open(&mut app, &path, 1);
    let document = app
        .world()
        .get::<EditorPanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<EditorSession>()
        .document_revision(document)
        .unwrap();
    let edit = submit(
        &mut app,
        EditorAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..8,
                bytes: b"acknowledged".to_vec(),
            }],
        },
        Some(panel),
    );
    outcome(&mut app, edit);
    let id = submit(
        &mut app,
        EditorAction::Command(WindowCommand::Save),
        Some(panel),
    );
    app.update();
    assert!(poll_editor_request_result(app.world_mut(), id).is_none());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(matches!(
        outcome(&mut app, id),
        EditorRequestOutcome::Saved { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"acknowledged");
    let revision = app
        .world()
        .non_send::<EditorSession>()
        .document_revision(document)
        .unwrap();
    let edit = submit(
        &mut app,
        EditorAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..12,
                bytes: b"closed after save".to_vec(),
            }],
        },
        Some(panel),
    );
    outcome(&mut app, edit);
    let id = submit(&mut app, EditorAction::ClosePanel(panel), Some(panel));
    app.update();
    assert!(poll_editor_request_result(app.world_mut(), id).is_none());
    assert!(app.world().entities().contains(panel));
    assert!(matches!(
        outcome(&mut app, id),
        EditorRequestOutcome::Closed { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"closed after save");
    assert!(!app.world().entities().contains(panel));
    shutdown(&mut app);
}

fn preview_selections_and_double_clicks_supersede_pending_remote_opens() {
    let directory = TempDir::new();
    let first = directory.write("first.txt", b"first");
    let second = directory.write("second.txt", b"second");
    let mut app = remote_app(&directory, false);
    let preview_request = |app: &mut App, path: &Path| {
        submit(
            app,
            EditorAction::HostRequest(bed_editor_api::HostRequest::PreviewFileIn {
                path: path.to_string_lossy().into_owned(),
                viewer: None,
                target: PanelTarget::LargestArea,
            }),
            None,
        )
    };
    let old = preview_request(&mut app, &first);
    let latest = preview_request(&mut app, &second);
    process_editor_requests(app.world_mut()).unwrap();
    assert!(matches!(
        poll_editor_request_result(app.world_mut(), old)
            .unwrap()
            .unwrap(),
        EditorRequestOutcome::Ignored
    ));
    let panel = match outcome(&mut app, latest) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<EditorPanel>(panel).unwrap().id)
    );
    // A double click before the first read completes requests a kept opening.
    let third = directory.write("third.txt", b"third");
    let preview = preview_request(&mut app, &third);
    let kept = request_open(&mut app, &third, 1);
    process_editor_requests(app.world_mut()).unwrap();
    assert!(matches!(
        poll_editor_request_result(app.world_mut(), preview)
            .unwrap()
            .unwrap(),
        EditorRequestOutcome::Ignored
    ));
    let kept_panel = match outcome(&mut app, kept) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    let last = preview_request(&mut app, &directory.write("last.txt", b"last"));
    outcome(&mut app, last);
    assert!(app.world().entities().contains(kept_panel));
    assert!(!app.world().entities().contains(panel));
    shutdown(&mut app);
}

fn location_navigation_replaces_a_remote_preview_and_finishes_the_jump() {
    let directory = TempDir::new();
    let source_path = directory.write("source.rs", b"// source\nfn source() {}\n");
    let target_path = directory.write("target.rs", "// target\n// 🦀 definition\n".as_bytes());
    let mut app = remote_app(&directory, false);
    let id = submit(
        &mut app,
        EditorAction::HostRequest(bed_editor_api::HostRequest::PreviewFileIn {
            path: source_path.to_string_lossy().into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        }),
        None,
    );
    let source = match outcome(&mut app, id) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    let source_surface = panel_presentation(app.world(), source).unwrap();
    let id = submit(
        &mut app,
        EditorAction::HostRequest(bed_editor_api::HostRequest::NavigateLocation {
            path: target_path.to_string_lossy().into_owned(),
            row: 1,
            utf16_column: 6,
            origin: None,
        }),
        Some(source_surface),
    );
    process_editor_requests(app.world_mut()).unwrap();
    assert!(poll_editor_request_result(app.world_mut(), id).is_none());
    assert!(app.world().entities().contains(source));
    assert_eq!(outcome(&mut app, id), EditorRequestOutcome::Applied);
    let target = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_ne!(target, source);
    assert!(!app.world().entities().contains(source));
    let target_id = app.world().get::<EditorPanel>(target).unwrap().id;
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(target_id)
    );
    let view = app
        .world()
        .get::<EditorPanelBinding>(target)
        .unwrap()
        .view
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<EditorSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 8));
    shutdown(&mut app);
}

fn terminal_file_links_finish_the_jump_after_a_remote_read() {
    let directory = TempDir::new();
    let path = directory.write("terminal.rs", "first\n// 🦀 target\nlast".as_bytes());
    let mut app = remote_app(&directory, false);
    let id = submit(
        &mut app,
        EditorAction::HostRequest(bed_editor_api::HostRequest::OpenTerminalLink {
            path: path.clone(),
            position: Some((1, 8)),
        }),
        None,
    );
    process_editor_requests(app.world_mut()).unwrap();
    assert!(poll_editor_request_result(app.world_mut(), id).is_none());
    let panel = match outcome(&mut app, id) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    let view = app
        .world()
        .get::<EditorPanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<EditorSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 8));
    let id = submit(
        &mut app,
        EditorAction::HostRequest(bed_editor_api::HostRequest::OpenTerminalLink {
            path,
            position: Some((2, 1)),
        }),
        None,
    );
    assert!(
        matches!(outcome(&mut app, id), EditorRequestOutcome::Opened { panel: reused, .. } if reused == panel)
    );
    let snapshot = app
        .world()
        .non_send::<EditorSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (2, 1));
    shutdown(&mut app);
}

fn main() {
    if std::env::args().any(|arg| arg == "--stdio") {
        serve_with(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            |request| {
                if matches!(
                    request,
                    Request::ReadFile { .. } | Request::WriteFile { .. }
                ) {
                    thread::sleep(Duration::from_millis(120));
                }
                LocalBackend.call(request)
            },
        )
        .unwrap();
        return;
    }
    if !cfg!(unix) {
        return;
    }
    captured_destination_and_cancellation();
    restoration_keeps_reserved_identity_and_group_selection();
    save_and_close_receipts_wait_for_acknowledged_bytes();
    preview_selections_and_double_clicks_supersede_pending_remote_opens();
    location_navigation_replaces_a_remote_preview_and_finishes_the_jump();
    terminal_file_links_finish_the_jump_after_a_remote_read();
    println!(
        "remote editor: captured destinations, cancellation, shared restoration, delayed save and close receipts, preview supersession and location navigation passed"
    );
}
