//! bEd automation submits its own captured application transactions.
use crate::application::{
    AppAction, AppRequestId, AppRequestOutcome, AppSystems, capture_app_request_from,
    poll_app_request_result, submit_app_request,
};
use bed_api::workbench::PanelTarget;
use bevy::prelude::*;
use std::io;

pub(crate) fn configure(app: &mut App) {
    app.configure_sets(
        PreUpdate,
        bed_plugin_automation::AutomationSystems::Ingress.before(AppSystems::Services),
    )
    .configure_sets(
        PostUpdate,
        bed_plugin_automation::AutomationSystems::Complete.after(AppSystems::Requests),
    );
}

pub(crate) fn submit(world: &mut World, action: AppAction, source: Option<Entity>) -> AppRequestId {
    let request = capture_app_request_from(world, action, PanelTarget::LargestArea, source);
    submit_app_request(world, request)
}

pub(crate) fn poll(world: &mut World, id: AppRequestId) -> Option<io::Result<AppRequestOutcome>> {
    poll_app_request_result(world, id)
        .map(|result| result.map_err(|error| io::Error::new(error.kind, error.message)))
}

#[cfg(all(test, feature = "editor"))]
mod tests {
    use super::*;
    use crate::application::test_support::TempDir;
    use crate::application::{
        AppOptions, AppRuntimePlugin, RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore,
    };
    use bed_plugin_automation::{
        AutomationPlugin, AutomationRequests, AutomationResults, DocumentInfo, Operation,
        OperationValue, Readiness, Snapshot,
    };
    #[cfg(feature = "workspaces")]
    use serde_json::Value;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    struct Fixture {
        app: App,
        dir: TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new();
            let settings = crate::config::Settings::with_paths(
                dir.path("config"),
                env!("CARGO_MANIFEST_DIR").into(),
            )
            .unwrap();
            let paths = bed_api::launch::ApplicationPaths::new(
                "bed",
                Some(settings.config_dir.clone()),
                None,
            )
            .unwrap();
            let store = WorkspaceStore::load(&paths).unwrap();
            let mut app = App::new();
            app.insert_resource(paths.clone())
                .insert_non_send(settings)
                .insert_non_send(bed_ui::icons::Icons::default())
                .insert_non_send(WorkspacePersistence::new(store))
                .insert_resource(RemoteHelperDirectory(std::path::PathBuf::new()))
                .add_plugins(AppRuntimePlugin {
                    options: AppOptions {
                        working_directory: Some(dir.path("")),
                        ..Default::default()
                    },
                })
                .add_plugins(bed_plugin_editor::EditorPlugin)
                .add_plugins(AutomationPlugin);
            configure(&mut app);
            bed_core::settings::initialize_settings(
                app.world_mut(),
                Some(paths.config_dir.join("settings.json")),
            )
            .unwrap();
            crate::config::register_app_settings(app.world_mut()).unwrap();
            bed_core::settings::finalize_settings(app.world_mut()).unwrap();
            crate::config::sync_view(app.world_mut()).unwrap();
            app.update();
            Self { app, dir }
        }
        fn run(&mut self, action: AppAction) -> io::Result<AppRequestOutcome> {
            let id = submit(self.app.world_mut(), action, None);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                self.app.update();
                if let Some(result) = poll(self.app.world_mut(), id) {
                    return result;
                }
                assert!(Instant::now() < deadline, "Application operation timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        fn inspect(&mut self) -> Snapshot {
            let id = self
                .app
                .world_mut()
                .resource_mut::<AutomationRequests>()
                .submit(Operation::Inspect);
            self.app.update();
            let OperationValue::Snapshot(snapshot) = self
                .app
                .world_mut()
                .resource_mut::<AutomationResults>()
                .take(id)
                .unwrap()
                .unwrap()
            else {
                panic!("Snapshot receipt")
            };
            snapshot
        }
        fn open(&mut self) -> DocumentInfo {
            let path = self.dir.path("document.txt");
            fs::write(&path, b"original").unwrap();
            self.run(AppAction::OpenFileWithViewer {
                path,
                viewer: None,
                additional: false,
                move_existing: false,
            })
            .unwrap();
            self.inspect().documents.remove(0)
        }
    }

    #[test]
    fn edits_undo_and_save_complete_through_the_same_application_queue() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        let snapshot = fixture.inspect();
        let panel = snapshot
            .panels
            .iter()
            .find(|panel| panel.panel_type == bed_plugin_editor::TEXT_PANEL_TYPE)
            .expect("Text panel");
        assert!(panel.title.contains("document.txt"));
        assert_eq!(panel.readiness, Readiness::Ready);
        fixture
            .run(AppAction::ApplyEdits {
                document: document.handle.id(),
                revision: document.revision,
                edits: vec![bed_documents::ByteEdit {
                    range: 0..8,
                    bytes: b"changed".to_vec(),
                }],
            })
            .unwrap();
        let read =
            bed_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
        assert_eq!(read.bytes, b"changed");
        assert!(read.info.dirty);
        let AppRequestOutcome::Saved { documents } = fixture
            .run(AppAction::SaveDocuments(vec![document.handle.id()]))
            .unwrap()
        else {
            panic!("Actual save receipt")
        };
        assert_eq!(documents.len(), 1);
        assert_eq!(fs::read(&document.path).unwrap(), b"changed");
        fixture
            .run(AppAction::HostAction(bed_api::features::HostAction::Undo {
                document: document.handle.id(),
            }))
            .unwrap();
        let read =
            bed_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
        assert_eq!(read.bytes, b"original");
        assert!(read.info.dirty);
    }

    #[test]
    fn stale_edits_fail_without_mutating_the_current_document() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        fixture
            .run(AppAction::ApplyEdits {
                document: document.handle.id(),
                revision: document.revision,
                edits: vec![bed_documents::ByteEdit {
                    range: 0..8,
                    bytes: b"new".to_vec(),
                }],
            })
            .unwrap();
        let error = fixture
            .run(AppAction::ApplyEdits {
                document: document.handle.id(),
                revision: document.revision,
                edits: vec![bed_documents::ByteEdit {
                    range: 0..3,
                    bytes: b"stale".to_vec(),
                }],
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let read =
            bed_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
        assert_eq!(read.bytes, b"new");
    }

    #[test]
    fn unknown_directory_preserves_document_handles_without_saving_a_workspace() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        let directory = fixture.dir.path("temporary");
        fs::create_dir(&directory).unwrap();
        fixture
            .run(AppAction::HostAction(
                bed_api::features::HostAction::OpenDirectory {
                    location: bed_api::files::FileLocation {
                        provider: bed_api::files::FileProviderId::Local,
                        path: directory.clone(),
                    },
                },
            ))
            .unwrap();
        let read =
            bed_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
        assert_eq!(read.bytes, b"original");
        assert_eq!(
            fixture
                .app
                .world()
                .resource::<bed_api::files::FileContext>()
                .directory,
            fs::canonicalize(&directory).unwrap()
        );
        assert!(!directory.join(".bed").exists());
        #[cfg(feature = "workspaces")]
        assert_eq!(
            fixture
                .app
                .world()
                .resource::<bed_plugin_workspaces::WorkspaceAttachment>(),
            &bed_plugin_workspaces::WorkspaceAttachment::Temporary
        );
    }

    #[cfg(feature = "workspaces")]
    #[test]
    fn attachment_preserves_handles_and_replacement_retires_them() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        fixture
            .run(AppAction::AttachWorkspace(fixture.dir.path("")))
            .unwrap();
        assert!(bed_plugin_automation::read_document(fixture.app.world(), document.handle).is_ok());
        let replacement = fixture.dir.path("other");
        fs::create_dir(&replacement).unwrap();
        let replacement = bed_plugin_workspaces::WorkspaceSpec::local(
            fs::canonicalize(replacement).unwrap().to_str().unwrap(),
        );
        fixture
            .app
            .world_mut()
            .resource_mut::<bed_plugin_workspaces::WorkspaceRepositoryResource>()
            .0
            .commit(
                replacement.clone(),
                bed_plugin_workspaces::WorkspaceSnapshot {
                    layouts: Value::Null,
                    plugins: serde_json::json!({}),
                },
            )
            .unwrap();
        fixture.run(AppAction::SetWorkspace(replacement)).unwrap();
        assert_eq!(
            bed_plugin_automation::read_document(fixture.app.world(), document.handle)
                .unwrap_err()
                .kind,
            bed_plugin_automation::ErrorKind::StaleHandle
        );
    }
}
