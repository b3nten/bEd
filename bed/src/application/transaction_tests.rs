//! Independent viewer drafts use the same permanent-resource save and close pipeline.
use super::*;
use crate::application::test_support::TempDir;
use bedtime_plugin_csv_table::{CsvStates, CsvTablePlugin};
use std::{fs, thread, time::Duration};

struct CsvFixture {
    app: App,
    path: PathBuf,
    tab: Entity,
    csv: Entity,
    document: DocumentId,
}

impl CsvFixture {
    fn new(directory: &TempDir) -> Self {
        let path = directory.write("project/table.csv", b"name,value\nalpha,one\n");
        let mut app = App::new();
        install_test_infrastructure(&mut app, &directory.path("config"));
        app.add_plugins((
            (bevy::MinimalPlugins, bevy::input::InputPlugin),
            AppRuntimePlugin {
                options: AppOptions {
                    working_directory: Some(directory.root().to_owned()),
                    ..Default::default()
                },
            },
            bedtime_plugin_editor::EditorPlugin,
            CsvTablePlugin,
        ));
        app.update();
        bedtime_core::settings::apply_settings_request(
            app.world_mut(),
            &bedtime_api::settings::SettingsRequest::new("bedtime.documents", "autosave", false),
        )
        .unwrap();
        let tab = open_file(
            app.world_mut(),
            &path,
            Some(&bedtime_api::ViewerId::from(
                bedtime_plugin_csv_table::VIEWER_ID,
            )),
            false,
            None,
        )
        .unwrap();
        let csv = panel_presentation(app.world(), tab).unwrap();
        let document = app
            .world()
            .get::<PanelBinding>(csv)
            .unwrap()
            .document
            .unwrap();
        let mut fixture = Self {
            app,
            path,
            tab,
            csv,
            document,
        };
        fixture.wait_ready();
        fixture
    }

    fn request(&mut self, action: AppAction) -> io::Result<AppRequestOutcome> {
        let request = capture_app_request_from(
            self.app.world(),
            action,
            PanelTarget::LastFocused,
            Some(self.tab),
        );
        let id = submit_app_request(self.app.world_mut(), request);
        wait_app_request(self.app.world_mut(), id)
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.update();
            if self
                .app
                .world()
                .non_send::<CsvStates>()
                .get(self.csv)
                .is_some_and(|panel| panel.is_ready())
            {
                return;
            }
            assert!(Instant::now() < deadline, "CSV preparation did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn draft(&mut self, text: &str) {
        self.app
            .world_mut()
            .non_send_mut::<CsvStates>()
            .set_cell_draft(self.csv, 1, 1, text.into())
            .unwrap();
    }

    fn invalidate_revision(&mut self) {
        // Change the shared document without advancing CSV preparation. The
        // transaction must publish this revision before checking the old draft.
        let mut session = self.app.world_mut().non_send_mut::<DocumentSession>();
        let revision = session.document_revision(self.document).unwrap();
        session
            .apply_edits(
                self.document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"outside,change\n".to_vec(),
                }],
            )
            .unwrap();
    }

    fn draft_text(&self) -> Option<&str> {
        self.app
            .world()
            .non_send::<CsvStates>()
            .draft_text(self.csv)
    }
}

#[test]
fn deleting_a_file_with_a_rejected_csv_draft_preserves_the_deleted_buffer_and_reports_the_error() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("draft survives deletion");
    fixture.invalidate_revision();
    fs::remove_file(&fixture.path).unwrap();
    fixture
        .app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .invalidate_removed_path(fixture.document)
        .unwrap();
    let _ = tick_app_services(fixture.app.world_mut());
    let snapshot = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(fixture.document)
        .unwrap();
    assert!(
        snapshot.path.is_empty(),
        "a deleted buffer must become Save As even when draft validation fails"
    );
    assert!(snapshot.dirty);
    assert_eq!(snapshot.bytes, b"outside,change\nname,value\nalpha,one\n");
    assert!(
        !fixture.path.exists(),
        "draft handling must not recreate a removed file"
    );
    assert_eq!(fixture.draft_text(), Some("draft survives deletion"));
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(
        fixture
            .app
            .world()
            .resource::<ApplicationState>()
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Deleted file buffer preserved"))
    );
}

#[test]
fn a_pending_workspace_attachment_validates_csv_drafts_before_changing_the_workspace() {
    let directory = TempDir::new();
    fs::create_dir_all(directory.path("other")).unwrap();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("keep this workspace draft");
    fixture.invalidate_revision();
    let original_workspace = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .session_id();
    let transition = AppAction::SetWorkspace(WorkspaceSpec::local(
        directory.path("other").to_string_lossy(),
    ));
    assert!(fixture.request(transition).is_err());
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .session_id(),
        original_workspace
    );
    assert_eq!(fixture.draft_text(), Some("keep this workspace draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
}

#[test]
fn temporary_directory_adoption_commits_csv_input_and_retains_the_live_presentation() {
    let directory = TempDir::new();
    fs::create_dir_all(directory.path("other")).unwrap();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("attached edit");
    fixture
        .request(AppAction::SetWorkspace(WorkspaceSpec::local(
            directory.path("other").to_string_lossy(),
        )))
        .unwrap();
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(fixture.app.world().entities().contains(fixture.csv));
    assert_eq!(
        panel_presentation(fixture.app.world(), fixture.tab),
        Some(fixture.csv)
    );
    assert_eq!(fixture.draft_text(), None);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .snapshot(fixture.document)
            .unwrap()
            .bytes,
        b"name,value\nalpha,attached edit\n"
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        directory.path("other").canonicalize().unwrap()
    );
}

#[test]
fn terminal_promotion_rejects_a_stale_retained_csv_draft_before_closing_any_panel() {
    let directory = TempDir::new();
    fs::create_dir(directory.path("other")).unwrap();
    let mut fixture = CsvFixture::new(&directory);
    let caller = super::terminal_tests::open_terminal(fixture.app.world_mut());
    let other = super::terminal_tests::open_terminal(fixture.app.world_mut());
    fixture.draft("keep this promotion draft");
    fixture.invalidate_revision();
    let session = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .session_id();
    let request = capture_app_request_from(
        fixture.app.world(),
        AppAction::Workspace(WorkspaceAction::Promote(directory.path("other"))),
        PanelTarget::LastFocused,
        Some(caller),
    );
    let id = submit_app_request(fixture.app.world_mut(), request);
    process_app_requests(fixture.app.world_mut()).unwrap();
    assert!(
        poll_app_request_result(fixture.app.world_mut(), id)
            .unwrap()
            .is_err()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .session_id(),
        session
    );
    assert_eq!(fixture.draft_text(), Some("keep this promotion draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert!(
        [caller, other, fixture.tab, fixture.csv]
            .into_iter()
            .all(|panel| fixture.app.world().entities().contains(panel))
    );
}

#[test]
fn registered_workspace_command_acknowledges_promotion_failure() {
    let directory = TempDir::new();
    fs::create_dir(directory.path("other")).unwrap();
    let mut fixture = CsvFixture::new(&directory);
    let caller = super::terminal_tests::open_terminal(fixture.app.world_mut());
    fixture.draft("retain this command draft");
    fixture.invalidate_revision();
    let session = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .session_id();
    let mut request = capture_app_request_from(
        fixture.app.world(),
        AppAction::PluginCommand(bedtime_plugin_workspaces::PROMOTE_COMMAND.into()),
        PanelTarget::LastFocused,
        Some(caller),
    );
    request.arguments = json!({"directory":directory.path("other")});
    let id = submit_app_request(fixture.app.world_mut(), request);
    process_app_requests(fixture.app.world_mut()).unwrap();
    let result = poll_app_request_result(fixture.app.world_mut(), id)
        .expect("the command resolves its actual promotion");
    assert!(result.is_err());
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .session_id(),
        session
    );
    assert!(
        fixture
            .app
            .world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert!(fixture.app.world().entities().contains(caller));
    assert!(!bedtime_core::operation_barrier_active(fixture.app.world()));
}
