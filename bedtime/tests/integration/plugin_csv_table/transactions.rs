//! Independent viewer drafts use the same permanent-resource save and close pipeline.
use crate::headless_host;
use crate::temp_dir;
use bedtime_api::{
    documents::PanelBinding,
    features::HostAction,
    workbench::{PanelPresentation, PanelTarget, WorkbenchAction},
};
use bedtime_core::panel_presentation;
use bedtime_documents::{ByteEdit, DocumentId, DocumentSession};
use bedtime_plugin_csv_table::{CsvStates, CsvTablePlugin};
use bedtime_workbench::WorkbenchLayout;
use bevy::prelude::*;
use std::{fs, thread, time::Duration};
use std::{path::PathBuf, time::Instant};
use temp_dir::TempDir;

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
        headless_host::install(&mut app, &directory.path("config"));
        app.add_plugins((bedtime_plugin_editor::EditorPlugin, CsvTablePlugin));
        app.insert_resource(bedtime_core::DocumentClosePolicy(
            bedtime_documents::ClosePolicy::Save,
        ));
        headless_host::finish(&mut app, directory.root());
        bedtime_core::settings::apply_settings_request(
            app.world_mut(),
            &bedtime_api::settings::SettingsRequest::new("bedtime.documents", "autosave", false),
        )
        .unwrap();
        let tab = bedtime_core::open_file(
            app.world_mut(),
            &path,
            Some(&bedtime_api::ViewerId::from(
                bedtime_plugin_csv_table::VIEWER_ID,
            )),
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

    fn request(&mut self, action: HostAction) -> Result<bedtime_core::HostOutcome, String> {
        headless_host::host(self.app.world_mut(), Some(self.tab), action)
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

    fn text_view(&mut self) -> Result<(), String> {
        let tab = self
            .app
            .world()
            .get::<PanelPresentation>(self.tab)
            .unwrap()
            .id;
        let outcome = self.request(HostAction::OpenDocumentViewer {
            tab,
            document: self.document,
            viewer: bedtime_plugin_editor::TEXT_VIEWER.into(),
        })?;
        let bedtime_core::HostOutcome::Opened(panel) = outcome else {
            panic!("Viewer opens a tab")
        };
        self.tab = panel;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        headless_host::workbench(self.app.world_mut(), WorkbenchAction::ClosePanel(self.tab))
            .map(|_| ())
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
fn replacing_a_csv_preview_commits_and_retains_its_unsaved_cell_draft() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
        .request(HostAction::PreviewFileIn {
            path: directory
                .write("other.txt", b"other")
                .to_string_lossy()
                .into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        })
        .unwrap();
    assert!(fixture.app.world().entities().contains(fixture.tab));
    assert!(fixture.app.world().entities().contains(fixture.csv));
    assert_eq!(fixture.draft_text(), None);
    let snapshot = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(fixture.document)
        .unwrap();
    assert!(snapshot.dirty);
    assert!(String::from_utf8_lossy(&snapshot.bytes).contains("retain this draft"));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"name,value\nalpha,one\n");
}

#[test]
fn saving_a_world_csv_commits_and_acknowledges_the_pending_cell_before_writing_disk() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("two, quoted");
    headless_host::invoke(
        fixture.app.world_mut(),
        "bedtime.documents.save",
        Some(fixture.tab),
    )
    .unwrap();
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
            .non_send::<DocumentSession>()
            .snapshot(fixture.document)
            .unwrap()
            .dirty
    );
}

#[test]
fn opening_another_viewer_commits_csv_drafts_and_saving_either_tab_commits_shared_drafts() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.draft("first edit");
    fixture.text_view().unwrap();
    let text = panel_presentation(fixture.app.world(), fixture.tab).unwrap();
    assert_ne!(text, fixture.csv);
    let text_view = fixture
        .app
        .world()
        .get::<PanelBinding>(text)
        .unwrap()
        .view
        .unwrap();
    assert!(
        fixture
            .app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .with_view(text_view, |view| view.view.request_focus)
            .unwrap(),
        "the new text tab focuses its widget"
    );
    assert!(
        fixture.app.world().entities().contains(fixture.csv),
        "the independent CSV tab remains open"
    );
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .snapshot(fixture.document)
            .unwrap()
            .bytes,
        b"name,value\nalpha,first edit\n"
    );
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"name,value\nalpha,one\n",
        "opening a viewer commits drafts without saving the file"
    );
    fixture.wait_ready();
    fixture.draft("hidden edit");
    headless_host::invoke(
        fixture.app.world_mut(),
        "bedtime.documents.save",
        Some(fixture.tab),
    )
    .unwrap();
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
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
fn stale_csv_drafts_block_save_close_and_new_viewer_without_discarding_the_draft() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for operation in ["save", "close", "switch", "preview"] {
        let directory = TempDir::new();
        let mut fixture = CsvFixture::new(&directory);
        fixture.draft("keep my draft");
        fixture.invalidate_revision();
        let result = match operation {
            "save" => headless_host::invoke(
                fixture.app.world_mut(),
                "bedtime.documents.save",
                Some(fixture.tab),
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
                    .request(HostAction::PreviewFileIn {
                        path: directory
                            .write("other.txt", b"other")
                            .to_string_lossy()
                            .into_owned(),
                        viewer: None,
                        target: PanelTarget::LargestArea,
                    })
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
                .non_send::<DocumentSession>()
                .snapshot(fixture.document)
                .unwrap()
                .bytes,
            b"outside,change\nname,value\nalpha,one\n"
        );
    }
}

#[test]
fn a_rejected_csv_tab_draft_blocks_the_text_tab_save_and_retains_both_tabs() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let directory = TempDir::new();
    let mut fixture = CsvFixture::new(&directory);
    fixture.text_view().unwrap();
    let text = panel_presentation(fixture.app.world(), fixture.tab).unwrap();
    fixture.draft("hidden draft");
    fixture.invalidate_revision();
    assert!(
        headless_host::invoke(
            fixture.app.world_mut(),
            "bedtime.documents.save",
            Some(fixture.tab)
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
