//! Real remote protocol completions through the same permanent-resource editor app.
use bed::application::*;
use bed_api::{documents::PanelBinding, workbench::PanelTarget};
use bed_documents::{ByteEdit, ClosePolicy, DocumentSession, SessionOptions};
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
#[path = "support/temp_dir.rs"]
mod support;
use support::TempDir;

fn remote_app(directory: &TempDir, restore: bool) -> App {
    remote_app_with_target(directory, restore, SshTarget::new("fixture"))
}

fn remote_app_with_target(directory: &TempDir, restore: bool, target: SshTarget) -> App {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.arg("--stdio");
    let client = RemoteClient::launch_command(child).unwrap();
    let root = fs::canonicalize(directory.root()).unwrap();
    let session = DocumentSession::with_remote_options(
        SessionOptions {
            autosave: None,
            git: false,
            highlighting: false,
            monitoring: false,
            ..Default::default()
        },
        target,
        client,
        root.to_string_lossy().into_owned(),
    )
    .unwrap();
    let settings = bed::config::Settings::with_paths(
        directory.path("config"),
        bed::config::Settings::get_app_resources_path(),
    )
    .unwrap();
    let workspace = if restore {
        WorkspaceStore::load(
            &bed_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
                .unwrap(),
        )
        .unwrap()
        .last_workspace()
    } else {
        None
    };
    let mut app = App::new();
    app.world_mut().insert_non_send(session);
    app.insert_resource(
        bed_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
            .unwrap(),
    )
    .insert_non_send(bed_ui::icons::Icons::default())
    .insert_non_send(WorkspacePersistence::new(
        WorkspaceStore::load(
            &bed_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
                .unwrap(),
        )
        .unwrap(),
    ))
    .insert_resource(RemoteHelperDirectory(std::path::PathBuf::new()))
    .insert_non_send(settings);
    app.add_plugins((
        (bevy::MinimalPlugins, bevy::input::InputPlugin),
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(root),
                workspace,
                ..Default::default()
            },
        },
        bed_plugin_editor::EditorPlugin,
    ));
    app.update();
    bed_core::settings::apply_settings_request(
        app.world_mut(),
        &bed_api::settings::SettingsRequest::batch(vec![
            bed_api::settings::SettingsChange::set("bed.documents", "autosave", false),
            bed_api::settings::SettingsChange::set("bed.editor", "treesitter", false),
            bed_api::settings::SettingsChange::set("bed.editor", "git_changed_lines", false),
        ]),
    )
    .unwrap();
    bed::config::sync_view(app.world_mut()).unwrap();
    bed_core::settings::configure_document_settings(app.world_mut()).unwrap();
    app
}

fn submit(app: &mut App, action: AppAction, source: Option<Entity>) -> AppRequestId {
    let request = capture_app_request_from(app.world(), action, PanelTarget::LastFocused, source);
    submit_app_request(app.world_mut(), request)
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
            app.world().resource::<WorkspaceState>().error
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn outcome(app: &mut App, id: AppRequestId) -> AppRequestOutcome {
    let mut result = None;
    wait(app, |app| {
        result = poll_app_request_result(app.world_mut(), id);
        result.is_some()
    });
    result.unwrap().unwrap()
}

fn open(app: &mut App, path: &Path, area: u32) -> Entity {
    let id = request_open(app, path, area);
    app.update();
    assert!(
        poll_app_request_result(app.world_mut(), id).is_none(),
        "Opening must await real read completion"
    );
    match outcome(app, id) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    }
}

fn request_open(app: &mut App, path: &Path, area: u32) -> AppRequestId {
    let mut request = capture_app_request(
        app.world(),
        AppAction::OpenFileWithViewer {
            path: path.to_owned(),
            viewer: None,
            additional: false,
            move_existing: false,
        },
        PanelTarget::LastFocused,
    );
    request.area = area;
    submit_app_request(app.world_mut(), request)
}

fn shutdown(app: &mut App) {
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}

fn service_publication_retains_the_authenticated_ssh_target() {
    let directory = TempDir::new();
    let target = SshTarget {
        host: "authenticated-fixture".into(),
        agent: "/custom/installed/bed-remote-agent".into(),
    };
    let mut app = remote_app_with_target(&directory, false, target.clone());
    let saved = WorkspaceSpec {
        name: "Saved fixture".into(),
        target: WorkspaceTarget::Ssh {
            host: "configured-alias".into(),
        },
        root: directory.root().to_string_lossy().into_owned(),
    };
    app.world_mut().resource_mut::<WorkspaceState>().spec = Some(saved.clone());
    let client = app
        .world()
        .non_send::<DocumentSession>()
        .remote_client()
        .unwrap();
    app.insert_resource(bed_files::FileServices::remote(target.host.clone(), client));
    configure_feature_targets(app.world_mut());
    for _ in 0..3 {
        app.update();
        let context = app.world().resource::<bed_files::FileContext>();
        let services = app.world().resource::<bed_files::FileServices>();
        assert_eq!(services.ssh_target(), Some(&target));
        assert_eq!(&context.provider, services.provider_id());
        assert!(services.for_context(context).connected());
        assert_eq!(
            app.world().resource::<WorkspaceState>().spec,
            Some(saved.clone())
        );
    }
    shutdown(&mut app);
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
    let focus = submit(&mut app, AppAction::FocusArea(1), None);
    app.update();
    assert!(matches!(
        outcome(&mut app, focus),
        AppRequestOutcome::Applied
    ));
    let panel = match outcome(&mut app, id) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let saved_id = app.world().get::<PanelPresentation>(panel).unwrap().id;
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(saved_id),
        Some(area)
    );
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"remote arrival"
    );
    let view = app.world().get::<PanelBinding>(panel).unwrap().view;
    let mut request = capture_app_request(
        app.world(),
        AppAction::OpenFileWithViewer {
            path: path.clone(),
            viewer: None,
            additional: false,
            move_existing: true,
        },
        PanelTarget::LastFocused,
    );
    request.area = 1;
    let moved = submit_app_request(app.world_mut(), request);
    assert!(
        matches!(outcome(&mut app,moved),AppRequestOutcome::Opened{panel:opened,..} if opened==panel)
    );
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(saved_id),
        Some(1)
    );
    assert_eq!(app.world().get::<PanelBinding>(panel).unwrap().view, view);
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );

    let id = request_open(&mut app, &canceled, area);
    app.update();
    assert!(poll_app_request_result(app.world_mut(), id).is_none());
    let mut tiling = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let plan = tiling.layout.plan_join(1, area, [1; 2]).unwrap();
    tiling.remove(saved_id);
    tiling.apply_plan(plan);
    let request = submit(
        &mut app,
        AppAction::WorkbenchAction(bed_api::workbench::WorkbenchAction::SetLayout {
            tiling: tiling.to_value([]),
            close: vec![panel],
        }),
        None,
    );
    outcome(&mut app, request);
    let mut error = None;
    wait(&mut app, |app| {
        error = poll_app_request_result(app.world_mut(), id);
        error.is_some()
    });
    assert_eq!(
        error.unwrap().unwrap_err().kind,
        std::io::ErrorKind::NotFound
    );
    wait(&mut app, |app| {
        !app.world()
            .non_send::<DocumentSession>()
            .open_pending(&canceled)
    });
    assert!(panel_entities(app.world_mut()).is_empty());
    assert!(
        app.world()
            .non_send::<DocumentSession>()
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
        let mut store = WorkspaceStore::load(
            &bed_api::launch::ApplicationPaths::new("bed", Some(directory.path("config")), None)
                .unwrap(),
        )
        .unwrap();
        let spec = store
            .record_workspace(WorkspaceSpec {
                name: "Remote fixture".into(),
                target: WorkspaceTarget::Ssh {
                    host: "fixture".into(),
                },
                root: fs::canonicalize(directory.root())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            })
            .unwrap();
        let mut tiling = bed_workbench::workspace::tiling_state::TilingState::default();
        tiling.sync_area(1, vec![41, 42], Some(selected));
        let (panel_type, viewer) = if bytes {
            (
                bed_plugin_editor::HEX_PANEL_TYPE,
                bed_plugin_editor::HEX_VIEWER,
            )
        } else {
            (
                bed_plugin_editor::TEXT_PANEL_TYPE,
                bed_plugin_editor::TEXT_VIEWER,
            )
        };
        let saved_layout = json!({"version":2,"panels":[{"id":41,"panel_type":panel_type,"viewer":viewer,"document_kind":if bytes {"bytes"}else{"text"},"path":path,"state":{}},{"id":42,"panel_type":panel_type,"viewer":viewer,"document_kind":if bytes {"bytes"}else{"text"},"path":path,"state":{}}],"tiling":tiling.to_value([41,42]),"focused":selected,"active_document_panel":selected});
        store
            .save_session_layout(
                &spec,
                json!({"layouts":{"1":saved_layout},"active_layout_slot":1}),
            )
            .unwrap();
        let mut app = remote_app(&directory, true);
        let tabs = panel_entities(app.world_mut());
        assert_eq!(tabs.len(), 2);
        let captured = tabs
            .iter()
            .map(|tab| (*tab, app.world().get::<PanelPresentation>(*tab).unwrap().id))
            .collect::<Vec<_>>();
        wait(&mut app, |app| {
            tabs.iter().all(|tab| {
                app.world()
                    .get::<PanelBinding>(*tab)
                    .is_some_and(|binding| binding.document.is_some())
                    && panel_presentation(app.world(), *tab).is_some_and(|surface| {
                        app.world()
                            .get::<bed_plugin_editor::EditorPanelState>(surface)
                            .is_some()
                    })
            })
        });
        let group = &app.world().resource::<WorkbenchLayout>().tiling.areas[0];
        assert_eq!(group.tabs, vec![41, 42]);
        assert_eq!(group.selected, Some(selected));
        for (tab, id) in captured {
            assert_eq!(app.world().get::<PanelPresentation>(tab).unwrap().id, id);
        }
        let document = app
            .world()
            .get::<PanelBinding>(tabs[0])
            .unwrap()
            .document
            .unwrap();
        assert_eq!(
            app.world().get::<PanelBinding>(tabs[1]).unwrap().document,
            Some(document)
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .view_count(document),
            if bytes { 0 } else { 2 }
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .document_kind(document)
                .unwrap(),
            if bytes {
                bed_documents::DocumentKind::Bytes
            } else {
                bed_documents::DocumentKind::Text
            }
        );
        let focus = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        assert_eq!(
            app.world().get::<PanelPresentation>(focus).unwrap().id,
            selected
        );
        assert_eq!(
            app.world().resource::<DocumentNavigation>().active_view,
            app.world().get::<PanelBinding>(focus).unwrap().view
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
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let edit = submit(
        &mut app,
        AppAction::ApplyEdits {
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
        AppAction::Command(WindowCommand::Save),
        Some(panel),
    );
    app.update();
    assert!(poll_app_request_result(app.world_mut(), id).is_none());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(matches!(
        outcome(&mut app, id),
        AppRequestOutcome::Saved { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"acknowledged");
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let edit = submit(
        &mut app,
        AppAction::ApplyEdits {
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
    let id = submit(&mut app, AppAction::ClosePanel(panel), Some(panel));
    app.update();
    assert!(poll_app_request_result(app.world_mut(), id).is_none());
    assert!(app.world().entities().contains(panel));
    assert!(matches!(
        outcome(&mut app, id),
        AppRequestOutcome::Closed { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"closed after save");
    assert!(!app.world().entities().contains(panel));
    shutdown(&mut app);
}

fn submit_delayed_save(app: &mut App, panel: Entity, bytes: &[u8]) -> AppRequestId {
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let edit = submit(
        app,
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..snapshot.bytes.len(),
                bytes: bytes.to_vec(),
            }],
        },
        Some(panel),
    );
    outcome(app, edit);
    submit(app, AppAction::SaveDocuments(vec![document]), Some(panel))
}

#[cfg(feature = "workspaces")]
fn workspace_snapshot_waits_for_an_earlier_write_without_committing_drafts() {
    let directory = TempDir::new();
    let path = directory.write("snapshot.txt", b"original");
    let mut app = remote_app(&directory, false);
    let panel = open(&mut app, &path, 1);
    let session = app.world().non_send::<DocumentSession>().session_id();
    let save = submit_delayed_save(&mut app, panel, b"before snapshot");
    process_app_requests(app.world_mut()).unwrap();
    let frontend = panel_presentation(app.world(), panel).unwrap();
    #[derive(Component)]
    struct PendingDraft;
    let commit =
        app.world_mut()
            .register_system(|In(_): In<bed_api::workbench::PanelActionInvocation>| {
                Err::<bool, String>("snapshot must retain this draft".into())
            });
    app.world_mut()
        .entity_mut(frontend)
        .insert((PendingDraft, bed_api::workbench::PanelActions(commit)));
    let workspace = submit(
        &mut app,
        AppAction::PluginCommand(bed_plugin_workspaces::SAVE_COMMAND.into()),
        Some(panel),
    );
    let later = submit(
        &mut app,
        AppAction::Settings(bed_api::settings::SettingsRequest::new(
            "bed.workbench",
            "tab_bar_add_button",
            true,
        )),
        None,
    );
    process_app_requests(app.world_mut()).unwrap();
    assert!(poll_app_request_result(app.world_mut(), save).is_none());
    assert!(poll_app_request_result(app.world_mut(), workspace).is_none());
    assert!(poll_app_request_result(app.world_mut(), later).is_none());
    assert!(app.world().resource::<WorkspaceState>().spec.is_none());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(
        !app.world()
            .resource::<bed_api::settings::SettingsSnapshot>()
            .get("bed.workbench", "tab_bar_add_button")
            .unwrap()
            .as_bool()
            .unwrap()
    );
    assert!(matches!(
        outcome(&mut app, workspace),
        AppRequestOutcome::Applied
    ));
    assert!(matches!(
        outcome(&mut app, save),
        AppRequestOutcome::Saved { .. }
    ));
    assert!(matches!(
        outcome(&mut app, later),
        AppRequestOutcome::Applied
    ));
    assert!(
        app.world()
            .resource::<bed_api::settings::SettingsSnapshot>()
            .get("bed.workbench", "tab_bar_add_button")
            .unwrap()
            .as_bool()
            .unwrap()
    );
    assert_eq!(fs::read(&path).unwrap(), b"before snapshot");
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(app.world().get::<PendingDraft>(frontend).is_some());
    assert!(app.world().resource::<WorkspaceState>().spec.is_some());
    assert!(!bed_core::operation_barrier_active(app.world()));
    shutdown(&mut app);
}

#[cfg(feature = "workspaces")]
fn workspace_replacement_waits_for_an_earlier_write() {
    let directory = TempDir::new();
    let path = directory.write("transition.txt", b"original");
    let local = directory.path("local");
    fs::create_dir_all(&local).unwrap();
    let local = fs::canonicalize(local).unwrap();
    let mut app = remote_app(&directory, false);
    let spec = app
        .world_mut()
        .resource_mut::<bed_plugin_workspaces::WorkspaceRepositoryResource>()
        .0
        .commit(
            WorkspaceSpec::local(local.to_string_lossy()),
            bed_plugin_workspaces::WorkspaceSnapshot::default(),
        )
        .unwrap();
    let panel = open(&mut app, &path, 1);
    let session = app.world().non_send::<DocumentSession>().session_id();
    let save = submit_delayed_save(&mut app, panel, b"before replacement");
    let replacement = submit(
        &mut app,
        AppAction::Workspace(bed_plugin_workspaces::WorkspaceAction::OpenWorkspace(
            spec.clone(),
        )),
        None,
    );
    process_app_requests(app.world_mut()).unwrap();
    assert!(poll_app_request_result(app.world_mut(), save).is_none());
    assert!(poll_app_request_result(app.world_mut(), replacement).is_none());
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(app.world().entities().contains(panel));
    assert!(app.world().non_send::<DocumentSession>().is_remote());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(matches!(
        outcome(&mut app, replacement),
        AppRequestOutcome::Applied
    ));
    assert!(matches!(
        outcome(&mut app, save),
        AppRequestOutcome::Saved { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"before replacement");
    assert_ne!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(!app.world().entities().contains(panel));
    assert!(!app.world().non_send::<DocumentSession>().is_remote());
    assert_eq!(
        app.world().resource::<WorkspaceState>().spec.as_ref(),
        Some(&spec)
    );
    assert!(!bed_core::operation_barrier_active(app.world()));
    shutdown(&mut app);
}

fn quit_waits_for_an_earlier_write() {
    let directory = TempDir::new();
    let path = directory.write("quit.txt", b"original");
    let mut app = remote_app(&directory, false);
    let panel = open(&mut app, &path, 1);
    let save = submit_delayed_save(&mut app, panel, b"before quit");
    let quit = submit(&mut app, AppAction::Command(WindowCommand::Quit), None);
    process_app_requests(app.world_mut()).unwrap();
    assert!(poll_app_request_result(app.world_mut(), save).is_none());
    assert!(poll_app_request_result(app.world_mut(), quit).is_none());
    assert!(app.world().entities().contains(panel));
    assert!(!app.world().resource::<WorkspaceState>().close_requested);
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(matches!(
        outcome(&mut app, quit),
        AppRequestOutcome::Closed { .. }
    ));
    assert!(matches!(
        outcome(&mut app, save),
        AppRequestOutcome::Saved { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), b"before quit");
    assert!(!app.world().entities().contains(panel));
    assert!(app.world().resource::<WorkspaceState>().close_requested);
    assert!(!bed_core::operation_barrier_active(app.world()));
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
            AppAction::HostAction(bed_api::features::HostAction::PreviewFileIn {
                path: path.to_string_lossy().into_owned(),
                viewer: None,
                target: PanelTarget::LargestArea,
            }),
            None,
        )
    };
    let old = preview_request(&mut app, &first);
    let latest = preview_request(&mut app, &second);
    process_app_requests(app.world_mut()).unwrap();
    assert!(matches!(
        poll_app_request_result(app.world_mut(), old)
            .unwrap()
            .unwrap(),
        AppRequestOutcome::Ignored
    ));
    let panel = match outcome(&mut app, latest) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(panel).unwrap().id)
    );
    // A double click before the first read completes requests a kept opening.
    let third = directory.write("third.txt", b"third");
    let preview = preview_request(&mut app, &third);
    let kept = request_open(&mut app, &third, 1);
    process_app_requests(app.world_mut()).unwrap();
    assert!(matches!(
        poll_app_request_result(app.world_mut(), preview)
            .unwrap()
            .unwrap(),
        AppRequestOutcome::Ignored
    ));
    let kept_panel = match outcome(&mut app, kept) {
        AppRequestOutcome::Opened { panel, .. } => panel,
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
        AppAction::HostAction(bed_api::features::HostAction::PreviewFileIn {
            path: source_path.to_string_lossy().into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        }),
        None,
    );
    let source = match outcome(&mut app, id) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    let source_surface = panel_presentation(app.world(), source).unwrap();
    let id = submit(
        &mut app,
        AppAction::HostAction(bed_api::features::HostAction::NavigateLocation {
            path: target_path.to_string_lossy().into_owned(),
            row: 1,
            utf16_column: 6,
            origin: None,
        }),
        Some(source_surface),
    );
    process_app_requests(app.world_mut()).unwrap();
    assert!(poll_app_request_result(app.world_mut(), id).is_none());
    assert!(app.world().entities().contains(source));
    assert_eq!(outcome(&mut app, id), AppRequestOutcome::Applied);
    let target = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    assert_ne!(target, source);
    assert!(!app.world().entities().contains(source));
    let target_id = app.world().get::<PanelPresentation>(target).unwrap().id;
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(target_id)
    );
    let view = app
        .world()
        .get::<PanelBinding>(target)
        .unwrap()
        .view
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
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
        AppAction::HostAction(bed_api::features::HostAction::OpenTerminalLink {
            path: path.clone(),
            position: Some((1, 8)),
        }),
        None,
    );
    process_app_requests(app.world_mut()).unwrap();
    assert!(poll_app_request_result(app.world_mut(), id).is_none());
    let panel = match outcome(&mut app, id) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        other => panic!("{other:?}"),
    };
    let view = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 8));
    let id = submit(
        &mut app,
        AppAction::HostAction(bed_api::features::HostAction::OpenTerminalLink {
            path,
            position: Some((2, 1)),
        }),
        None,
    );
    assert!(
        matches!(outcome(&mut app, id), AppRequestOutcome::Opened { panel: reused, .. } if reused == panel)
    );
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
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
    service_publication_retains_the_authenticated_ssh_target();
    captured_destination_and_cancellation();
    restoration_keeps_reserved_identity_and_group_selection();
    save_and_close_receipts_wait_for_acknowledged_bytes();
    #[cfg(feature = "workspaces")]
    {
        workspace_snapshot_waits_for_an_earlier_write_without_committing_drafts();
        workspace_replacement_waits_for_an_earlier_write();
    }
    quit_waits_for_an_earlier_write();
    preview_selections_and_double_clicks_supersede_pending_remote_opens();
    location_navigation_replaces_a_remote_preview_and_finishes_the_jump();
    terminal_file_links_finish_the_jump_after_a_remote_read();
    println!(
        "remote editor: SSH target retention, captured destinations, cancellation, shared restoration, delayed save and transition receipts, preview supersession and location navigation passed"
    );
}
