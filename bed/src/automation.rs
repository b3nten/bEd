//! bEd automation submits its own captured application transactions.
use crate::application::requests::{
    AppAction, AppRequestId, AppRequestOutcome, capture_app_request_from, poll_app_request_result,
    submit_app_request,
};
use bedtime_api::workbench::PanelTarget;
use bevy::prelude::*;
use std::io;

pub(crate) fn configure(app: &mut App) {
    register_operations(app.world_mut());
    app.configure_sets(
        PreUpdate,
        bedtime_plugin_automation::AutomationSystems::Ingress
            .before(bedtime_core::CoreSystems::Services),
    )
    .configure_sets(
        PostUpdate,
        bedtime_plugin_automation::AutomationSystems::Complete
            .after(bedtime_core::CoreSystems::Requests),
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

fn register_operations(world: &mut World) {
    use bedtime_plugin_automation::{ExtensionDefinition, register_extension};
    use serde_json::json;
    for (name, description, submit, arguments) in [
        (
            "bed.open_file",
            "Open a file using bEd placement and viewer policy",
            open_file
                as fn(
                    &mut World,
                    serde_json::Value,
                ) -> Result<
                    bedtime_plugin_automation::ExtensionStart,
                    bedtime_plugin_automation::AutomationError,
                >,
            json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"viewer":{"type":["string","null"]},"additional":{"type":"boolean"},"move_existing":{"type":"boolean"},"source":{"type":["string","null"]}}}),
        ),
        (
            "bed.layout.select",
            "Select a numbered or Startup layout",
            select_layout,
            json!({"type":"object","required":["layout"],"properties":{"layout":{"enum":[1,2,3,4,5,6,7,8,9,"startup"]}}}),
        ),
        (
            "bed.layout.reset",
            "Reset the current layout",
            reset_layout,
            json!({"type":"object"}),
        ),
        (
            "bed.workspace.open",
            "Open or restore a local workspace",
            open_workspace,
            json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}),
        ),
        (
            "bed.workspace.save",
            "Save the current workspace",
            save_workspace,
            json!({"type":"object","properties":{"name":{"type":["string","null"]}}}),
        ),
        (
            "bed.state",
            "Observe bEd layout and workspace state",
            state,
            json!({"type":"object"}),
        ),
    ] {
        register_extension(
            world,
            ExtensionDefinition {
                name,
                description,
                arguments,
                result: json!({"type":"object"}),
                submit,
            },
        )
        .expect("bEd automation extensions register once");
    }
}
fn tracked(
    world: &mut World,
    action: AppAction,
    source: Option<Entity>,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    let id = submit(world, action, source);
    Ok(bedtime_plugin_automation::ExtensionStart::pending(
        move |world| {
            poll(world, id).map(|result| {
            use bedtime_plugin_automation::{wire, panel_handle, document_handle, AutomationError, ErrorKind};
            use serde_json::json;
            let outcome = result.map_err(|error| AutomationError::new(ErrorKind::Failed, error.to_string()))?;
            Ok(match outcome {
                AppRequestOutcome::Opened { panel, document } => {
                    let handle = panel_handle(world, panel)?;
                    json!({"outcome":"opened","panel":wire::panel_value(handle),"document":document.map(|id| wire::document_value(document_handle(world,id)))})
                }
                AppRequestOutcome::Saved { documents } => {
                    let saved = documents.into_iter().map(|saved| json!({"document":wire::document_value(document_handle(world,saved.document)),"revision":saved.revision})).collect::<Vec<_>>();
                    json!({"outcome":"saved","documents":saved})
                }
                AppRequestOutcome::Closed { panels } => json!({"outcome":"closed","count":panels.len()}),
                AppRequestOutcome::Applied => json!({"outcome":"applied"}),
                AppRequestOutcome::Ignored => json!({"outcome":"ignored"}),
            })
        })
        },
    ))
}

fn open_file(
    world: &mut World,
    args: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    use bedtime_plugin_automation::{resolve_panel, wire};
    let path = wire::string(&args, "path")?.into();
    let viewer = args
        .get("viewer")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| wire::invalid("viewer must be a string"))
        })
        .transpose()?;
    let flag = |key| {
        args.get(key)
            .map(|v| v.as_bool().ok_or_else(|| wire::invalid("expected boolean")))
            .transpose()
            .map(|v| v.unwrap_or(false))
    };
    let source = args
        .get("source")
        .filter(|v| !v.is_null())
        .map(|v| wire::panel(v).and_then(|p| resolve_panel(world, p)))
        .transpose()?;
    tracked(
        world,
        AppAction::OpenFileWithViewer {
            path,
            viewer,
            additional: flag("additional")?,
            move_existing: flag("move_existing")?,
        },
        source,
    )
}
fn select_layout(
    world: &mut World,
    args: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    let slot = crate::config::LayoutSlot::from_value(&args["layout"]).ok_or_else(|| {
        bedtime_plugin_automation::wire::invalid("layout must be 1..9 or startup")
    })?;
    tracked(
        world,
        AppAction::Command(crate::application::requests::WindowCommand::UseLayout(slot)),
        None,
    )
}
fn reset_layout(
    world: &mut World,
    _: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    tracked(
        world,
        AppAction::Command(crate::application::requests::WindowCommand::ResetLayout),
        None,
    )
}
fn open_workspace(
    world: &mut World,
    args: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    let path = bedtime_plugin_automation::wire::string(&args, "path")?.into();
    tracked(
        world,
        AppAction::Workspace(bedtime_plugin_workspaces::WorkspaceAction::OpenLocalWorkspace(path)),
        None,
    )
}
fn save_workspace(
    world: &mut World,
    args: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    let name = args
        .get("name")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| bedtime_plugin_automation::wire::invalid("name must be a string"))
        })
        .transpose()?;
    tracked(
        world,
        AppAction::Workspace(bedtime_plugin_workspaces::WorkspaceAction::SaveCurrent { name }),
        None,
    )
}
fn state(
    world: &mut World,
    _: serde_json::Value,
) -> Result<bedtime_plugin_automation::ExtensionStart, bedtime_plugin_automation::AutomationError> {
    use serde_json::json;
    Ok(bedtime_plugin_automation::ExtensionStart::Complete(
        json!({"layout":world.resource::<crate::application::LayoutSlots>().active().key(),"workspace":world.resource::<bedtime_plugin_workspaces::WorkspaceAttachment>().saved().map(|workspace| workspace.to_value()),"error":world.resource::<crate::application::ApplicationState>().error}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::TempDir;
    use crate::application::{
        AppOptions, AppRuntimePlugin, RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore,
    };
    use bedtime_plugin_automation::{
        AutomationPlugin, AutomationRequests, AutomationResults, DocumentInfo, Operation,
        OperationValue, Readiness, Snapshot,
    };
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
            let settings = crate::config::AppConfig::with_paths(
                dir.path("config"),
                env!("CARGO_MANIFEST_DIR").into(),
            )
            .unwrap();
            let paths = bedtime_api::launch::ApplicationPaths::new(
                "bed",
                Some(settings.config_dir.clone()),
                None,
            )
            .unwrap();
            let store = WorkspaceStore::load(&paths).unwrap();
            let mut app = App::new();
            app.insert_resource(paths.clone())
                .insert_non_send(settings)
                .insert_non_send(bedtime_ui::icons::Icons::default())
                .insert_non_send(WorkspacePersistence::new(store))
                .insert_resource(RemoteHelperDirectory(std::path::PathBuf::new()))
                .add_plugins(crate::themes::ThemePlugin::new(
                    env!("CARGO_MANIFEST_DIR").into(),
                ))
                .add_plugins(AppRuntimePlugin {
                    options: AppOptions {
                        working_directory: Some(dir.path("")),
                        ..Default::default()
                    },
                })
                .add_plugins(bedtime_plugin_editor::EditorPlugin)
                .add_plugins(AutomationPlugin);
            configure(&mut app);
            bedtime_core::settings::initialize_settings(
                app.world_mut(),
                Some(paths.config_dir.join("settings.json")),
            )
            .unwrap();
            crate::config::register_app_settings(app.world_mut()).unwrap();
            bedtime_core::settings::finalize_settings(app.world_mut()).unwrap();
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
            .find(|panel| panel.panel_type == bedtime_plugin_editor::TEXT_PANEL_TYPE)
            .expect("Text panel");
        assert!(panel.title.contains("document.txt"));
        assert_eq!(panel.readiness, Readiness::Ready);
        fixture
            .run(AppAction::ApplyEdits {
                document: document.handle.id(),
                revision: document.revision,
                edits: vec![bedtime_documents::ByteEdit {
                    range: 0..8,
                    bytes: b"changed".to_vec(),
                }],
            })
            .unwrap();
        let read =
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
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
            .run(AppAction::HostAction(
                bedtime_api::features::HostAction::Undo {
                    document: document.handle.id(),
                },
            ))
            .unwrap();
        let read =
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
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
                edits: vec![bedtime_documents::ByteEdit {
                    range: 0..8,
                    bytes: b"new".to_vec(),
                }],
            })
            .unwrap();
        let error = fixture
            .run(AppAction::ApplyEdits {
                document: document.handle.id(),
                revision: document.revision,
                edits: vec![bedtime_documents::ByteEdit {
                    range: 0..3,
                    bytes: b"stale".to_vec(),
                }],
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Document changed; refresh before applying edits")
        );
        let read =
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
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
                bedtime_api::features::HostAction::OpenDirectory {
                    location: bedtime_api::files::FileLocation {
                        provider: bedtime_api::files::FileProviderId::Local,
                        path: directory.clone(),
                    },
                },
            ))
            .unwrap();
        let read =
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle).unwrap();
        assert_eq!(read.bytes, b"original");
        assert_eq!(
            fixture
                .app
                .world()
                .resource::<bedtime_api::files::FileContext>()
                .directory,
            fs::canonicalize(&directory).unwrap()
        );
        assert!(!directory.join(".bed").exists());
        assert_eq!(
            fixture
                .app
                .world()
                .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>(),
            &bedtime_plugin_workspaces::WorkspaceAttachment::Temporary
        );
    }

    #[test]
    fn attachment_preserves_handles_and_replacement_retires_them() {
        let mut fixture = Fixture::new();
        let document = fixture.open();
        fixture
            .run(AppAction::AttachWorkspace(fixture.dir.path("")))
            .unwrap();
        assert!(
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle).is_ok()
        );
        let replacement = fixture.dir.path("other");
        fs::create_dir(&replacement).unwrap();
        let replacement = bedtime_plugin_workspaces::WorkspaceSpec::local(
            fs::canonicalize(replacement).unwrap().to_str().unwrap(),
        );
        fixture
            .app
            .world_mut()
            .resource_mut::<bedtime_plugin_workspaces::WorkspaceRepositoryResource>()
            .0
            .commit(
                replacement.clone(),
                bedtime_plugin_workspaces::WorkspaceSnapshot {
                    history: None,
                    layouts: Value::Null,
                    plugins: serde_json::json!({"type":"object"}),
                },
            )
            .unwrap();
        fixture.run(AppAction::SetWorkspace(replacement)).unwrap();
        assert_eq!(
            bedtime_plugin_automation::read_document(fixture.app.world(), document.handle)
                .unwrap_err()
                .kind,
            bedtime_plugin_automation::ErrorKind::StaleHandle
        );
    }
}
