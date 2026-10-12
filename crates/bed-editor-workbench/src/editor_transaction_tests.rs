//! Retained viewer drafts use the same permanent-resource save and close pipeline.
use super::*;
use crate::test_support::TempDir;
use bed_csv::{CsvPlugin, CsvStates};
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
            bed_runtime::BedRuntimePlugin,
            EditorWorkbenchPlugin {
                options: EditorWorkbenchOptions {
                    working_directory: Some(directory.root().to_owned()),
                    ..Default::default()
                },
            },
            bed_editor::EditorPlugin,
            CsvPlugin,
        ));
        app.world_mut().non_send_mut::<Settings>().settings["autosave"] = json!(false);
        app.update();
        let tab = open_file(
            app.world_mut(),
            &path,
            Some(bed_csv::VIEWER_ID),
            false,
            None,
        )
        .unwrap();
        let csv = panel_presentation(app.world(), tab).unwrap();
        let document = app
            .world()
            .get::<EditorPanelBinding>(csv)
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
    fn request(&mut self, action: EditorAction) -> io::Result<EditorRequestOutcome> {
        let request = capture_editor_request_from(
            self.app.world(),
            action,
            PanelTarget::LastFocused,
            Some(self.tab),
        );
        let id = submit_editor_request(self.app.world_mut(), request);
        process_editor_requests(self.app.world_mut())?;
        poll_editor_request_result(self.app.world_mut(), id)
            .expect("local request completed")
            .map_err(|error| io::Error::new(error.kind, error.message))
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
    fn text_view(&mut self) -> io::Result<()> {
        self.request(EditorAction::SwitchViewer {
            panel: self.tab,
            viewer: bed_editor::TEXT_VIEWER.into(),
        })
        .map(|_| ())
    }
    fn close(&mut self) -> io::Result<()> {
        self.request(EditorAction::ClosePanel(self.tab)).map(|_| ())
    }
    fn invalidate_revision(&mut self) {
        // Change the shared document without advancing CSV preparation. The
        // transaction must publish this revision before checking the old draft.
        let mut session = self.app.world_mut().non_send_mut::<EditorSession>();
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
fn replacing_a_csv_preview_commits_and_retains_its_unsaved_cell_draft() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    let id = fixture
        .app
        .world()
        .get::<PanelPresentation>(fixture.tab)
        .unwrap()
        .id;
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .areas[0]
        .preview = Some(id);
    fixture.draft("retain this draft");
    fixture
        .request(EditorAction::HostRequest(HostRequest::PreviewFileIn {
            path: directory
                .write("other.txt", b"other")
                .to_string_lossy()
                .into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        }))
        .unwrap();
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(fixture.app.world().entities().contains(fixture.csv));
    assert_eq!(fixture.draft_text(), None);
    let snapshot = fixture
        .app
        .world()
        .non_send::<EditorSession>()
        .snapshot(fixture.document)
        .unwrap();
    assert!(snapshot.dirty);
    assert!(String::from_utf8_lossy(&snapshot.bytes).contains("retain this draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
}

#[test]
fn saving_a_world_csv_commits_and_acknowledges_the_pending_cell_before_writing_disk() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("two, quoted");
    assert!(
        dispatch_editor_command(
            fixture.app.world_mut(),
            WindowCommand::Save,
            PanelTarget::LastFocused
        )
        .unwrap()
    );
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"name,value\nalpha,\"two, quoted\"\n"
    );
    assert_eq!(
        fixture.draft_text(),
        None,
        "the real acknowledgement retires the submitted draft"
    );
    assert!(
        !fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .snapshot(fixture.document)
            .unwrap()
            .dirty
    );
}

#[test]
fn viewer_switch_commits_the_csv_draft_and_hidden_csv_drafts_participate_in_text_view_saves() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("first edit");
    fixture.text_view().unwrap();
    let text = panel_presentation(fixture.app.world(), fixture.tab).unwrap();
    assert_ne!(text, fixture.csv);
    let text_view = fixture
        .app
        .world()
        .get::<EditorPanelBinding>(text)
        .unwrap()
        .view
        .unwrap();
    assert!(
        fixture
            .app
            .world_mut()
            .non_send_mut::<EditorSession>()
            .with_view(text_view, |view| view.view.request_focus)
            .unwrap(),
        "changing the selected tab's presentation focuses its text widget"
    );
    assert!(
        fixture.app.world().entities().contains(fixture.csv),
        "a tab retains inactive presentations"
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .snapshot(fixture.document)
            .unwrap()
            .bytes,
        b"name,value\nalpha,first edit\n"
    );
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"name,value\nalpha,one\n",
        "a viewer change updates memory without saving the file"
    );
    fixture.wait_ready();
    fixture.draft("hidden edit");
    assert!(
        dispatch_editor_command(
            fixture.app.world_mut(),
            WindowCommand::Save,
            PanelTarget::LastFocused
        )
        .unwrap()
    );
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"name,value\nalpha,hidden edit\n"
    );
    assert_eq!(fixture.draft_text(), None);
    assert_eq!(
        panel_presentation(fixture.app.world(), fixture.tab),
        Some(text)
    );
}

#[test]
fn closing_a_world_csv_commits_and_saves_the_last_presentation_before_retiring_its_entity() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("close edit");
    fixture.close().unwrap();
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"name,value\nalpha,close edit\n"
    );
    assert!(!fixture.app.world().entities().contains(fixture.tab));
    assert!(!fixture.app.world().entities().contains(fixture.csv));
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .non_send::<CsvStates>()
            .get(fixture.csv)
            .is_none()
    );
}

#[test]
fn stale_csv_drafts_block_save_close_and_viewer_switch_without_discarding_the_draft() {
    for operation in ["save", "close", "switch", "preview"] {
        let directory = TempDir::new();
        let mut fixture = CsvFixture::new(&directory);
        fixture.draft("keep my draft");
        fixture.invalidate_revision();
        let result = match operation {
            "save" => dispatch_editor_command(
                fixture.app.world_mut(),
                WindowCommand::Save,
                PanelTarget::LastFocused,
            )
            .map(|_| ()),
            "close" => fixture.close(),
            "switch" => fixture.text_view(),
            "preview" => {
                let id = fixture
                    .app
                    .world()
                    .get::<PanelPresentation>(fixture.tab)
                    .unwrap()
                    .id;
                fixture
                    .app
                    .world_mut()
                    .resource_mut::<WorkbenchLayout>()
                    .tiling
                    .areas[0]
                    .preview = Some(id);
                fixture
                    .request(EditorAction::HostRequest(HostRequest::PreviewFileIn {
                        path: directory
                            .write("other.txt", b"other")
                            .to_string_lossy()
                            .into_owned(),
                        viewer: None,
                        target: PanelTarget::LargestArea,
                    }))
                    .map(|_| ())
            }
            _ => unreachable!(),
        };
        assert!(
            result.is_err(),
            "{operation} must validate against the current canonical revision"
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
        assert_eq!(fixture.draft_text(), Some("keep my draft"));
        assert!(fixture.app.world().entities().contains(fixture.tab));
        assert_eq!(
            panel_presentation(fixture.app.world(), fixture.tab),
            Some(fixture.csv)
        );
        assert_eq!(
            fixture
                .app
                .world()
                .non_send::<EditorSession>()
                .snapshot(fixture.document)
                .unwrap()
                .bytes,
            b"outside,change\nname,value\nalpha,one\n"
        );
    }
}

#[test]
fn a_rejected_hidden_csv_draft_blocks_the_text_view_save_and_retains_the_tab() {
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.text_view().unwrap();
    let text = panel_presentation(fixture.app.world(), fixture.tab).unwrap();
    fixture.draft("hidden draft");
    fixture.invalidate_revision();
    assert!(
        dispatch_editor_command(
            fixture.app.world_mut(),
            WindowCommand::Save,
            PanelTarget::LastFocused
        )
        .is_err()
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert_eq!(fixture.draft_text(), Some("hidden draft"));
    assert!(fixture.app.world().entities().contains(fixture.csv));
    assert_eq!(
        panel_presentation(fixture.app.world(), fixture.tab),
        Some(text)
    );
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
        .non_send_mut::<EditorSession>()
        .invalidate_removed_path(fixture.document)
        .unwrap();
    let _ = tick_editor_services(fixture.app.world_mut());
    let snapshot = fixture
        .app
        .world()
        .non_send::<EditorSession>()
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
            .resource::<EditorWorkspace>()
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
        .non_send::<EditorSession>()
        .workspace_id();
    let transition = EditorAction::SetWorkspace(WorkspaceSpec::local(
        directory.path("other").to_string_lossy(),
    ));
    assert!(fixture.request(transition).is_err());
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .workspace_id(),
        original_workspace
    );
    assert_eq!(fixture.draft_text(), Some("keep this workspace draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(
        fixture
            .app
            .world()
            .resource::<EditorWorkspace>()
            .spec
            .is_none()
    );
}

#[test]
fn pending_workspace_attachment_commits_csv_input_and_retains_the_actual_live_presentation() {
    let directory = TempDir::new();
    fs::create_dir_all(directory.path("other")).unwrap();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("attached edit");
    fixture
        .request(EditorAction::SetWorkspace(WorkspaceSpec::local(
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
            .non_send::<EditorSession>()
            .snapshot(fixture.document)
            .unwrap()
            .bytes,
        b"name,value\nalpha,attached edit\n"
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<EditorWorkspace>()
            .spec
            .as_ref()
            .unwrap()
            .root,
        directory
            .path("other")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
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
        .non_send::<EditorSession>()
        .workspace_id();
    let request = capture_editor_request_from(
        fixture.app.world(),
        EditorAction::HostRequest(HostRequest::PromoteWorkspace {
            root: directory.path("other"),
        }),
        PanelTarget::LastFocused,
        Some(caller),
    );
    let id = submit_editor_request(fixture.app.world_mut(), request);
    process_editor_requests(fixture.app.world_mut()).unwrap();
    assert!(
        poll_editor_request_result(fixture.app.world_mut(), id)
            .unwrap()
            .is_err()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<EditorSession>()
            .workspace_id(),
        session
    );
    assert_eq!(fixture.draft_text(), Some("keep this promotion draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
    assert!(
        fixture
            .app
            .world()
            .resource::<EditorWorkspace>()
            .spec
            .is_none()
    );
    assert!(
        [caller, other, fixture.tab, fixture.csv]
            .into_iter()
            .all(|panel| fixture.app.world().entities().contains(panel))
    );
}
