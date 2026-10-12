//! Read-only local database features and their presentations share the host world.
use crate::{
    OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, SqlitePanel, VIEWER_ID, register_metadata,
};
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelBinding, HostRequest, PanelInput,
    Registry as EditorRegistry,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf};

#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct SqliteFile(pub PathBuf);

#[derive(Default)]
pub struct SqliteStates(HashMap<Entity, SqlitePanel>);
impl SqliteStates {
    pub fn get(&self, entity: Entity) -> Option<&SqlitePanel> {
        self.0.get(&entity)
    }
    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut SqlitePanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SqliteSystems {
    Poll,
}

pub struct SqlitePlugin;
impl Plugin for SqlitePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_resource::<bed_editor_api::CommandPresentations>()
            .init_non_send::<SqliteStates>()
            .add_systems(
                PreUpdate,
                poll_sqlite
                    .in_set(SqliteSystems::Poll)
                    .in_set(EditorFeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("SQLite declarations must register exactly once");
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "SQLite Viewer", create_sqlite_panel);
            for action in crate::shortcut_actions() {
                registrar.shortcut_action(action);
            }
            registrar.command(OPEN_COMMAND, "Open SQLite Database…", sqlite_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in SQLite Viewer", sqlite_command);
            registrar.command(
                crate::status::SHOW_COMMAND,
                "Show SQLite query",
                status_command,
            );
            registrar.command(
                crate::status::CANCEL_QUERY_COMMAND,
                "Cancel SQLite query",
                status_command,
            );
            registrar.menu(OPEN_COMMAND);
        })
        .expect("SQLite systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<SqliteStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_sqlite_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Browse", "Select a table to inspect its rows. Copy cell or Copy row exports data; Inspect cell opens its full value."),
        ("Query", "Use the Query tab to run SQL. Cancel stops an active query."),
    ]).shortcut_actions(&["bed.edit.copy"]));
    let input = world
        .get::<EditorPanelBinding>(creation.entity)
        .map(|binding| binding.input.clone())
        .unwrap_or_else(|| {
            creation.state["path"]
                .as_str()
                .map(|path| PanelInput::LocalFile(PathBuf::from(path)))
                .unwrap_or_default()
        });
    match input {
        PanelInput::None => {}
        PanelInput::Document(_) => return Err("SQLite Viewer requires a local file".into()),
        PanelInput::LocalFile(path) => {
            if world.resource::<EditorFrame>().remote {
                return Err("SQLite Viewer supports local databases only".into());
            }
            if world
                .get::<EditorPanelBinding>(creation.entity)
                .is_some_and(|binding| binding.document.is_some() || binding.view.is_some())
            {
                return Err("A SQLite panel cannot attach an editor document".into());
            }
            world
                .entity_mut(creation.entity)
                .insert((SqliteFile(path.clone()), crate::status::items()));
            world
                .non_send_mut::<SqliteStates>()
                .0
                .insert(creation.entity, SqlitePanel::new(path, &creation.state));
        }
    }
    Ok(Box::new(SqliteFrontend {
        entity: creation.entity,
    }))
}

fn poll_sqlite(
    mut states: NonSendMut<SqliteStates>,
    live: Query<&SqliteFile>,
    mut intents: ResMut<EditorIntents>,
    frame: Res<EditorFrame>,
    mut commands: ResMut<bed_editor_api::CommandPresentations>,
    mut entities: Commands,
) {
    for command in [OPEN_COMMAND, OPEN_FILE_COMMAND] {
        commands.0.insert(
            command.into(),
            bed_editor_api::CommandPresentation {
                enabled: !frame.remote,
                visible: !frame.remote,
                ..Default::default()
            },
        );
    }
    // Dropping a worker cancels its active query and wakes it to close. Panel
    // removal therefore releases database work without waiting on the UI thread.
    states.0.retain(|entity, _| live.get(*entity).is_ok());
    for (entity, panel) in &mut states.0 {
        panel.poll();
        entities.entity(*entity).insert(match panel.error() {
            Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.into()),
            None if panel.is_ready() => bed_editor_api::EditorPanelReadiness::Ready,
            None => bed_editor_api::EditorPanelReadiness::Pending,
        });
    }
    if states.0.values().any(SqlitePanel::preparing) {
        intents.push(None, HostRequest::Invalidate);
    }
}

fn sqlite_command(
    In(invocation): In<CommandInvocation>,
    frame: Res<EditorFrame>,
    bindings: Query<&EditorPanelBinding>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    if frame.remote {
        return Err("SQLite Viewer supports local databases only".into());
    }
    let request = match invocation.command.as_str() {
        OPEN_COMMAND => Some(HostRequest::OpenFileDialog {
            viewer: Some(VIEWER_ID.into()),
        }),
        OPEN_FILE_COMMAND => invocation.arguments["path"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                let binding = bindings.get(invocation.source?).ok()?;
                binding
                    .input
                    .local_file()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .map(|path| HostRequest::OpenFile {
                path,
                viewer: Some(VIEWER_ID.into()),
            }),
        _ => return Err(format!("Unknown SQLite command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn status_command(
    In(invocation): In<CommandInvocation>,
    live: Query<&SqliteFile>,
    mut states: NonSendMut<SqliteStates>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    let Some(entity) = invocation.source else {
        return Ok(Vec::new());
    };
    if live.get(entity).is_err() {
        return Ok(Vec::new());
    }
    let Some(panel) = states.0.get_mut(&entity) else {
        return Ok(Vec::new());
    };
    match invocation.command.as_str() {
        crate::status::SHOW_COMMAND => {
            panel.show_query();
            intents.push_at(Some(entity), invocation.area, HostRequest::Invalidate);
            Ok(vec![Action::FocusPanel(entity)])
        }
        crate::status::CANCEL_QUERY_COMMAND => {
            let serial = invocation.arguments["serial"]
                .as_u64()
                .ok_or("SQLite cancellation requires its captured query serial")?;
            if panel.cancel_query(serial) {
                intents.push_at(Some(entity), invocation.area, HostRequest::Invalidate);
            }
            Ok(Vec::new())
        }
        _ => Err(format!(
            "Unknown SQLite status command: {}",
            invocation.command
        )),
    }
}

struct SqliteFrontend {
    entity: Entity,
}
impl Panel for SqliteFrontend {
    fn title(&self, world: &World) -> String {
        world
            .non_send::<SqliteStates>()
            .get(self.entity)
            .map_or_else(
                || "SQLite Viewer".into(),
                |panel| panel.panel_title(&world.resource::<EditorFrame>().context()),
            )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let mut state = SystemState::<(
            Res<EditorFrame>,
            NonSendMut<SqliteStates>,
            ResMut<EditorIntents>,
        )>::new(world);
        let (frame, mut states, mut intents) = state
            .get_mut(world)
            .expect("SQLite plugin initialized its presentation resources");
        let mut requests = Vec::new();
        if let Some(panel) = states.0.get_mut(&self.entity) {
            panel.shortcuts = _shortcuts.clone();
            panel.draw_prepared(ui, &frame.context(), &mut requests);
        } else {
            ui.text_wrapped("Open a SQLite database to browse its tables.");
            if frame.remote {
                ui.text_wrapped("SQLite Viewer supports local databases only.");
            } else if ui.button("Open Database…") {
                requests.push(HostRequest::OpenFileDialog {
                    viewer: Some(VIEWER_ID.into()),
                });
            }
        }
        for request in requests {
            intents.push(Some(self.entity), request);
        }
    }

    fn save(&self, world: &World) -> Value {
        world
            .non_send::<SqliteStates>()
            .get(self.entity)
            .map_or(Value::Null, SqlitePanel::saved_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };

    struct Database(PathBuf);
    impl Database {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-sqlite-bevy-{}-{}.db",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            rusqlite::Connection::open(&path)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE records(value); INSERT INTO records VALUES ('original');",
                )
                .unwrap();
            Self(path)
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn create(app: &mut App, input: PanelInput, state: Value) -> (Entity, PanelResult) {
        let entity = app
            .world_mut()
            .spawn((
                bed_workbench_api::PanelPresentation::new(13),
                EditorPanelBinding {
                    input,
                    workspace: 2,
                    ..Default::default()
                },
            ))
            .id();
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let panel = app
            .world_mut()
            .run_system_with(factory, PanelCreation { entity, state })
            .unwrap();
        (entity, panel)
    }

    fn wait(app: &mut App, entity: Entity) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            let panel = app.world().non_send::<SqliteStates>().get(entity).unwrap();
            if panel.is_ready() {
                assert_eq!(panel.error(), None);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "SQLite preparation failed: {:?}",
                panel.error()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn local_file_factory_prepares_without_ui_and_restored_query_stays_inert() {
        let database = Database::new();
        let mut app = App::new();
        app.add_plugins(SqlitePlugin);
        let (entity, panel) = create(
            &mut app,
            PanelInput::LocalFile(database.0.clone()),
            json!({"sql": "DELETE FROM records", "tab": "Query"}),
        );
        let panel = panel.unwrap();
        wait(&mut app, entity);
        assert_eq!(panel.save(app.world())["sql"], "DELETE FROM records");
        assert_eq!(
            panel.title(app.world()),
            database.0.file_name().unwrap().to_string_lossy()
        );
        let connection = rusqlite::Connection::open(&database.0).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "original");
        let binding = app.world().get::<EditorPanelBinding>(entity).unwrap();
        assert!(binding.document.is_none() && binding.view.is_none());
        let status_context = bed_workbench_api::StatusContext {
            publisher: entity,
            item_id: "bed.sqlite.document",
            focused_panel: None,
            focused_frontend: Some(entity),
            source: Some(entity),
            area: 0,
        };
        assert!(
            crate::status::presentation(app.world(), status_context)
                .unwrap()
                .summary
                .contains("read-only query")
        );
        assert!(
            crate::status::presentation(
                app.world(),
                bed_workbench_api::StatusContext {
                    focused_frontend: None,
                    ..status_context
                }
            )
            .is_none()
        );
        assert!(
            crate::status::presentation(
                app.world(),
                bed_workbench_api::StatusContext {
                    item_id: "bed.sqlite.query",
                    ..status_context
                }
            )
            .is_none(),
            "restoring a SQL draft does not publish running work"
        );
        app.world_mut().despawn(entity);
        app.update();
        assert!(app.world().non_send::<SqliteStates>().get(entity).is_none());
        assert!(crate::status::presentation(app.world(), status_context).is_none());
    }

    #[test]
    fn remote_and_document_inputs_do_not_start_database_workers() {
        let mut app = App::new();
        app.add_plugins(SqlitePlugin);
        let (entity, panel) = create(
            &mut app,
            PanelInput::Document(bed_editing::identity::DocumentId(1)),
            Value::Null,
        );
        assert!(panel.err().unwrap().contains("requires a local file"));
        assert!(app.world().get::<SqliteFile>(entity).is_none());

        app.world_mut().resource_mut::<EditorFrame>().remote = true;
        let (remote, panel) = create(
            &mut app,
            PanelInput::LocalFile(PathBuf::from("/remote/not-opened.sqlite")),
            Value::Null,
        );
        assert!(panel.err().unwrap().contains("local databases only"));
        assert!(app.world().get::<SqliteFile>(remote).is_none());
        assert!(app.world().non_send::<SqliteStates>().0.is_empty());
        let (_, empty) = create(&mut app, PanelInput::None, Value::Null);
        assert_eq!(empty.unwrap().title(app.world()), "SQLite Viewer");
        assert!(app.world().non_send::<SqliteStates>().0.is_empty());
    }

    #[test]
    fn registered_commands_capture_local_source_and_area_and_reject_remote_workspaces() {
        let mut app = App::new();
        app.add_plugins(SqlitePlugin);
        let (origin, _) = create(&mut app, PanelInput::None, Value::Null);
        let command = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(OPEN_FILE_COMMAND)
            .unwrap()
            .handler;
        let invocation = CommandInvocation {
            command: OPEN_FILE_COMMAND.into(),
            source: Some(origin),
            area: 18,
            arguments: json!({"path": "/local/captured.sqlite"}),
        };
        app.world_mut()
            .run_system_with(command, invocation.clone())
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(origin));
        assert_eq!(requests[0].area, Some(18));
        assert!(
            matches!(&requests[0].request, HostRequest::OpenFile { path, viewer }
            if path == "/local/captured.sqlite" && viewer.as_deref() == Some(VIEWER_ID))
        );
        app.world_mut().resource_mut::<EditorFrame>().remote = true;
        assert!(
            app.world_mut()
                .run_system_with(command, invocation)
                .unwrap()
                .unwrap_err()
                .contains("local databases only")
        );
        assert!(
            app.world_mut()
                .resource_mut::<EditorIntents>()
                .take()
                .is_empty()
        );
    }
}
