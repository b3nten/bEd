//! Query widgets and replacement workers are retained in typed world state.
use super::*;
use bedtime_api::documents::DocumentFrame;
use bedtime_api::{
    documents::{DocumentEvent, DocumentReply},
    features::{FeatureSystems, HostEffects},
    workbench::{PanelActionInvocation, PanelActionResult, PanelActions},
};
use bedtime_api::{
    status::{
        StatusContext, StatusDrawContext, StatusDrawMode, StatusItem, StatusItems, StatusText,
    },
    workbench::{
        CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction,
        register,
    },
};
use bedtime_documents::{DocumentSession, SessionEvent};
use bedtime_files::{FileContext, FileServices};
use bevy::{ecs::system::SystemState, prelude::*};

#[derive(Default)]
pub struct SearchPanels(pub HashMap<Entity, SearchPanel>);

impl SearchPanels {
    pub fn is_searching(&self) -> bool {
        self.0.values().any(|panel| panel.search.needs_tick())
    }

    pub fn get(&self, entity: Entity) -> Option<&SearchPanel> {
        self.0.get(&entity)
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut SearchPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(Resource)]
struct Actions(bedtime_api::workbench::PanelActionSystem);

#[derive(Component, Clone, Default)]
struct SearchStatus {
    busy: bool,
    text: StatusText,
}

pub struct FileSearchPlugin;

impl Plugin for FileSearchPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut().init_non_send::<SearchState>();
        app.world_mut().init_non_send::<DocumentSession>();
        app.world_mut().init_non_send::<SearchPanels>();
        app.init_resource::<FileContext>()
            .init_resource::<FileServices>()
            .init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .add_message::<DocumentReply>()
            .add_message::<DocumentEvent>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.command_ref(SHOW_COMMAND);
                registrar.command_ref(NEW_COMMAND);
                registrar.command_ref(CANCEL_COMMAND);
                registrar.toolbar(NEW_COMMAND);
                registrar.panel_options(PANEL_ID, false, PanelPlacement::Center);
            })
            .expect("Search metadata is registered once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Search", create_panel);
            for action in crate::shortcut_actions() {
                registrar.shortcut_action(action);
            }
            registrar.command(SHOW_COMMAND, "Find in Project", show);
            registrar.command(NEW_COMMAND, "New Search Panel", open);
            registrar.command(CANCEL_COMMAND, "Cancel Search", cancel);
            registrar.global_shortcut(SHOW_COMMAND, &["Primary+Shift+F"]);
            registrar.command_icon(NEW_COMMAND, "search");
            registrar.command_icon(CANCEL_COMMAND, "search");
            registrar.panel_icon(PANEL_ID, "search");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("FileSearchPlugin is installed once");
        let actions = app.world_mut().register_system(panel_action);
        app.insert_resource(Actions(actions))
            .add_systems(
                PreUpdate,
                (replies, document_events, prepare)
                    .chain()
                    .in_set(FeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}

fn show(In(invocation): In<CommandInvocation>, mut intents: ResMut<HostEffects>) -> CommandResult {
    intents.push_at(
        invocation.source,
        invocation.area,
        HostAction::ShowPanel {
            panel_type: PANEL_ID.into(),
            document: None,
            state: Value::Null,
            action: Some(PanelAction::Find),
        },
    );
    Ok(Vec::new())
}

fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn cancel(
    In(invocation): In<CommandInvocation>,
    state: NonSend<SearchState>,
    mut panels: NonSendMut<SearchPanels>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let Some(entity) = invocation.source else {
        return Ok(Vec::new());
    };
    if state
        .replacement
        .as_ref()
        .is_some_and(|job| job.owns(entity))
    {
        return Ok(Vec::new());
    }
    let Some(panel) = panels.0.get_mut(&entity) else {
        return Ok(Vec::new());
    };
    if panel.search.needs_tick() {
        panel.search.cancel();
        intents.push(Some(entity), HostAction::Invalidate);
    }
    Ok(Vec::new())
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bedtime_api::workbench::PanelDocumentation::new(&[
        ("Search", "Enter a query to find text across the project. Use the search options to control matching and scope."),
        ("Results", "Select a match to navigate to it. Replace next or Replace all applies the replacement text; Cancel stops an active search."),
    ]).shortcut_actions(&["bedtime.edit.find"]));
    let services = world.resource::<FileServices>().clone();
    let mut panel = SearchPanel::new(services, &creation.state);
    panel.search.root = world.resource::<FileContext>().directory_string();
    world
        .non_send_mut::<SearchPanels>()
        .0
        .insert(creation.entity, panel);
    let actions = world.resource::<Actions>().0;
    world.entity_mut(creation.entity).insert((
        PanelActions(actions),
        StatusItems(vec![
            StatusItem {
                id: "bedtime.search.activity",
                label: "Search activity",
                priority: 150,
                icon: "search",
                prefer_icon: false,
                measure: status_measure,
                draw: status_draw,
                tooltip: status_tooltip,
            },
            StatusItem {
                id: "bedtime.search.results",
                label: "Search results",
                priority: 50,
                icon: "search",
                prefer_icon: false,
                measure: status_measure,
                draw: status_draw,
                tooltip: status_tooltip,
            },
        ]),
    ));
    publish_status(world, creation.entity);
    Ok(Box::new(Frontend(creation.entity)))
}

fn panel_action(
    In(invocation): In<PanelActionInvocation>,
    mut panels: NonSendMut<SearchPanels>,
) -> PanelActionResult {
    let panel = panels
        .0
        .get_mut(&invocation.entity)
        .ok_or("Search panel is closed")?;
    if invocation.action == PanelAction::Find {
        panel.open();
        Ok(true)
    } else {
        Ok(false)
    }
}

fn shutdown(
    mut exit: MessageReader<bevy::app::AppExit>,
    mut state: NonSendMut<SearchState>,
    mut panels: NonSendMut<SearchPanels>,
) {
    if exit.read().next().is_some() {
        state.replacement = None;
        for panel in panels.0.values_mut() {
            panel.search.cancel();
        }
    }
}

fn prepare(
    mut state: NonSendMut<SearchState>,
    mut panels: NonSendMut<SearchPanels>,
    alive: Query<Entity>,
    (context, services): (Res<FileContext>, Res<FileServices>),
    mut documents: NonSendMut<DocumentSession>,
    mut intents: ResMut<HostEffects>,
    mut commands: Commands,
) {
    panels.0.retain(|entity, _| alive.contains(*entity));
    let services = services.for_context(&context);
    let identity = (
        context.provider.clone(),
        context.directory.clone(),
        context.generation,
        documents.session_id(),
    );
    let changed = !state.services.same_provider(&services)
        || state
            .context_identity
            .as_ref()
            .is_some_and(|previous| previous != &identity);
    state.context_identity = Some(identity);
    if changed {
        state.replacement = None;
        state.services = services.clone();
        for panel in panels.0.values_mut() {
            panel.search.set_file_services(services.clone());
            panel.search.root = context.directory_string();
            panel.search.invalidate();
        }
    }
    let root = context.directory_string();
    let mut requests = Vec::new();
    for (entity, panel) in &mut panels.0 {
        if state
            .replacement
            .as_ref()
            .is_some_and(|job| job.owns(*entity))
        {
            continue;
        }
        if panel.search.tick(&root, &documents) {
            requests.push(HostAction::Invalidate);
        }
    }
    if let Some(job) = &mut state.replacement {
        let finished = match panels.0.get_mut(&job.owner()) {
            Some(panel) => job.tick(&mut panel.search, &mut documents, &mut requests),
            None => true,
        };
        if finished {
            state.replacement = None;
        }
        requests.push(HostAction::Invalidate);
    }
    for (entity, panel) in &panels.0 {
        commands
            .entity(*entity)
            .insert(search_status(panel, &state, &root, *entity));
    }
    for request in requests {
        intents.push(None, request);
    }
}

fn search_status(
    panel: &SearchPanel,
    state: &SearchState,
    root: &str,
    entity: Entity,
) -> SearchStatus {
    let mut text = panel.search.status_text(root);
    let replacing = state.replacement.as_ref().filter(|job| job.owns(entity));
    let busy = replacing.is_some() || panel.search.needs_tick();
    if let Some(job) = replacing {
        let (saved, remaining, replaced, failures) = job.progress();
        text.summary = format!("Replacing… {saved} files saved · {remaining} remaining");
        text.details.push_str(&format!(
            "\n{replaced} occurrences replaced\n{failures} failures"
        ));
        text.error |= failures != 0;
    } else if panel.search.needs_tick() {
        text = text.command("Cancel", CANCEL_COMMAND);
    }
    SearchStatus { busy, text }
}

fn publish_status(world: &mut World, entity: Entity) {
    let Some(panel) = world.non_send::<SearchPanels>().0.get(&entity) else {
        return;
    };
    let status = search_status(
        panel,
        world.non_send::<SearchState>(),
        &world.resource::<FileContext>().directory_string(),
        entity,
    );
    world.entity_mut(entity).insert(status);
}

fn visible_status(world: &World, context: StatusContext) -> Option<&SearchStatus> {
    let status = world.get::<SearchStatus>(context.publisher)?;
    match context.item_id {
        "bedtime.search.activity" if status.busy => Some(status),
        "bedtime.search.results" if !status.busy && context.is_focused() => Some(status),
        _ => None,
    }
}

fn cancel_width(ui: &Ui) -> f32 {
    ui.calc_text_size("Cancel")[0] + ui.clone_style().frame_padding()[0] * 2.0
}

fn status_measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    visible_status(world, context).map(|status| {
        status.text.measure(ui)
            + if status.text.commands.is_empty() {
                0.0
            } else {
                cancel_width(ui) + ui.clone_style().item_spacing()[0]
            }
    })
}

fn status_draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    let Some(status) = visible_status(world, context.context) else {
        return;
    };
    if context.mode == StatusDrawMode::Inline && !status.text.commands.is_empty() {
        let mut text = status.text.clone();
        text.commands.clear();
        text.draw(
            ui,
            StatusDrawContext {
                width: (context.width - cancel_width(ui) - ui.clone_style().item_spacing()[0])
                    .max(1.0),
                ..context
            },
            actions,
        );
        ui.same_line();
        if ui.small_button("Cancel") {
            actions.push(context.context.invoke(CANCEL_COMMAND, Value::Null));
        }
    } else {
        status.text.draw(ui, context, actions);
    }
}

fn status_tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(status) = visible_status(world, context) {
        status.text.tooltip(ui);
    }
}

fn replies(mut replies: MessageReader<DocumentReply>, mut state: NonSendMut<SearchState>) {
    for reply in replies.read() {
        let Some(job) = &mut state.replacement else {
            continue;
        };
        match reply {
            DocumentReply::Edit {
                recipient,
                token,
                result,
            } if recipient == MODULE_ID => job.edit_result(*token, result.clone()),
            DocumentReply::Save {
                recipient,
                token,
                result,
            } if recipient == MODULE_ID => job.save_result(*token, result.clone()),
            _ => {}
        }
    }
}

fn document_events(
    mut events: MessageReader<DocumentEvent>,
    state: NonSend<SearchState>,
    mut panels: NonSendMut<SearchPanels>,
) {
    if events.read().any(|event| {
        matches!(
            event.0,
            SessionEvent::Edited { .. }
                | SessionEvent::Reloaded { .. }
                | SessionEvent::Removed { .. }
                | SessionEvent::PathChanged { .. }
                | SessionEvent::Closed { .. }
        )
    }) {
        for (entity, panel) in &mut panels.0 {
            if !state
                .replacement
                .as_ref()
                .is_some_and(|job| job.owns(*entity))
            {
                panel.search.invalidate();
            }
        }
    }
}

struct Frontend(Entity);

impl Panel for Frontend {
    fn title(&self, _: &World) -> String {
        "Search".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let mut state = SystemState::<(
            NonSendMut<SearchPanels>,
            NonSendMut<SearchState>,
            Res<FileContext>,
            Option<NonSend<bedtime_ui::icons::Icons>>,
        )>::new(world);
        let (mut panels, mut search_state, context, icons) = state
            .get_mut(world)
            .expect("Search frontend resources are installed");
        panels
            .0
            .get_mut(&self.0)
            .expect("Search frontend has widget state")
            .search
            .shortcuts = _shortcuts.clone();
        if let Err(error) = panels
            .0
            .get_mut(&self.0)
            .expect("Search frontend has widget state")
            .draw_body(
                self.0,
                &mut search_state,
                ui,
                &context.directory_string(),
                icons.as_deref(),
                &mut requests,
            )
        {
            requests.push(HostAction::Notify {
                message: error.to_string(),
            });
        }
        for request in requests {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.0), request);
        }
        publish_status(world, self.0);
    }

    fn save(&self, world: &World) -> Value {
        world
            .non_send::<SearchPanels>()
            .0
            .get(&self.0)
            .expect("Search frontend has widget state")
            .state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::{WorkbenchPlugin, WorkbenchRequests, commit_close, panel_state};

    #[test]
    fn directory_replacement_cancels_old_replacement_work_before_requesting_edits() {
        use std::time::{Duration, Instant};
        let first = crate::test_support::TempDir::new();
        let second = crate::test_support::TempDir::new();
        let path = first.write("words.txt", b"alpha");
        let mut app = App::new();
        app.insert_resource(FileContext::local(first.root()).unwrap())
            .add_plugins((WorkbenchPlugin, FileSearchPlugin));
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
                panel_type: PANEL_ID.into(),
                state: json!({"query":"alpha","replacement":"beta"}),
                area: None,
            });
        app.update();
        let entity = *app
            .world()
            .non_send::<SearchPanels>()
            .0
            .keys()
            .next()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if !app.world().non_send::<SearchPanels>().0[&entity]
                .search
                .needs_tick()
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let job = ReplacementJob::new(
            entity,
            &app.world().non_send::<SearchPanels>().0[&entity].search,
            true,
        )
        .unwrap();
        app.world_mut().non_send_mut::<SearchState>().replacement = Some(job);
        app.world_mut().resource_mut::<HostEffects>().take();
        app.world_mut()
            .resource_mut::<FileContext>()
            .replace(FileContext::local(second.root()).unwrap());
        app.update();
        assert!(!app.world().non_send::<SearchState>().is_replacing());
        assert_eq!(std::fs::read(path).unwrap(), b"alpha");
        assert!(
            app.world_mut()
                .resource_mut::<HostEffects>()
                .take()
                .iter()
                .all(|effect| matches!(effect.action, HostAction::Invalidate))
        );
    }

    #[test]
    fn ambient_search_cancel_captures_its_owner_and_completed_counts_need_focus() {
        use std::time::{Duration, Instant};
        let dir = crate::test_support::TempDir::new();
        dir.write("first.txt", b"alpha beta beta");
        dir.write("second.txt", b"beta");
        let root = dir.root().to_string_lossy().into_owned();
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, FileSearchPlugin));
        app.insert_resource(FileContext::local(&root).unwrap());
        for query in ["alpha", "beta"] {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(WorkbenchAction::OpenPanel {
                    panel_type: PANEL_ID.into(),
                    state: json!({"query":query}),
                    area: None,
                });
            app.update();
        }
        app.update();
        let entities = app
            .world()
            .non_send::<SearchPanels>()
            .0
            .iter()
            .map(|(entity, panel)| (panel.search.query.clone(), *entity))
            .collect::<HashMap<_, _>>();
        let first = entities["alpha"];
        let second = entities["beta"];
        let context = |publisher, id, focused| StatusContext {
            publisher,
            item_id: id,
            focused_panel: focused,
            focused_frontend: focused,
            source: Some(publisher),
            area: 1,
        };
        let busy = context(first, "bedtime.search.activity", Some(second));
        assert!(visible_status(app.world(), busy).is_some());
        assert_eq!(
            visible_status(app.world(), busy).unwrap().text.commands[0].command,
            CANCEL_COMMAND
        );
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(busy.invoke(CANCEL_COMMAND, Value::Null));
        app.update();
        assert!(
            app.world().non_send::<SearchPanels>().0[&first]
                .search
                .canceled
        );
        assert!(
            !app.world().non_send::<SearchPanels>().0[&second]
                .search
                .canceled
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.world().non_send::<SearchPanels>().0[&second]
            .search
            .needs_tick()
        {
            assert!(Instant::now() < deadline, "search did not finish");
            app.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        app.update();
        assert!(
            visible_status(
                app.world(),
                context(second, "bedtime.search.activity", None)
            )
            .is_none()
        );
        assert!(
            visible_status(
                app.world(),
                context(second, "bedtime.search.results", Some(first))
            )
            .is_none()
        );
        let result = visible_status(
            app.world(),
            context(second, "bedtime.search.results", Some(second)),
        )
        .unwrap();
        assert_eq!(result.text.summary, "3 matches in 2 matching files");
        assert!(result.text.commands.is_empty());
        assert!(
            visible_status(
                app.world(),
                context(first, "bedtime.search.results", Some(first))
            )
            .unwrap()
            .text
            .summary
            .starts_with("Canceled")
        );
        commit_close(app.world_mut(), first);
        app.update();
        assert!(visible_status(app.world(), busy).is_none());
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(busy.invoke(CANCEL_COMMAND, Value::Null));
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
    }

    #[test]
    fn registered_search_panels_keep_independent_queries_and_release_only_closed_workers() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, FileSearchPlugin));
        app.world_mut().resource_mut::<FileContext>().directory = "/workspace".into();
        for query in ["first", "second"] {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(WorkbenchAction::OpenPanel {
                    panel_type: PANEL_ID.into(),
                    state: json!({"query":query,"regex":true,"replacement":"value"}),
                    area: None,
                });
            app.update();
        }
        let panels: Vec<_> = app
            .world()
            .non_send::<SearchPanels>()
            .0
            .keys()
            .copied()
            .collect();
        assert_eq!(app.world().non_send::<SearchPanels>().0.len(), 2);
        let original = panel_state(app.world_mut(), panels[1]).unwrap();
        assert!(commit_close(app.world_mut(), panels[0]));
        app.update();
        assert_eq!(app.world().non_send::<SearchPanels>().0.len(), 1);
        assert_eq!(panel_state(app.world_mut(), panels[1]).unwrap(), original);
        assert_eq!(
            app.world().non_send::<SearchPanels>().0[&panels[1]]
                .search
                .root,
            "/workspace"
        );
        let action = app.world().get::<PanelActions>(panels[1]).unwrap().0;
        assert!(
            app.world_mut()
                .run_system_with(
                    action,
                    PanelActionInvocation {
                        entity: panels[1],
                        action: PanelAction::Find
                    }
                )
                .unwrap()
                .unwrap()
        );
    }

    #[test]
    fn replacement_system_waits_for_its_own_edit_and_save_messages() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let directory = crate::test_support::TempDir::new();
        let path = directory.write("words.txt", b"alpha alpha");
        let mut documents = DocumentSession::new();
        let document = documents.open_file(&path).unwrap();
        let mut search = ContentSearch::new_lazy();
        search.query = "alpha".into();
        search.replacement = "beta".into();
        search.open();
        let root = directory.root().to_str().unwrap();
        search.tick(root, &documents);
        thread::sleep(Duration::from_millis(210));
        let deadline = Instant::now() + Duration::from_secs(5);
        while search.needs_tick() {
            search.tick(root, &documents);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!search.results.is_empty());
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.add_plugins(FileSearchPlugin);
        let entity = app.world_mut().spawn_empty().id();
        let job = ReplacementJob::new(entity, &search, true).unwrap();
        app.world_mut()
            .non_send_mut::<SearchPanels>()
            .0
            .insert(entity, SearchPanel { search });
        app.world_mut().non_send_mut::<SearchState>().replacement = Some(job);
        app.insert_resource(FileContext::local(root).unwrap());
        app.update();
        let status = app.world().get::<SearchStatus>(entity).unwrap();
        assert!(status.busy);
        assert!(status.text.summary.contains("1 remaining"));
        assert!(
            status.text.commands.is_empty(),
            "replacement acknowledgements cannot be canceled as a search"
        );
        let cancel_handler = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(CANCEL_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                cancel_handler,
                CommandInvocation {
                    command: CANCEL_COMMAND.into(),
                    source: Some(entity),
                    area: 1,
                    arguments: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        assert!(app.world().non_send::<SearchState>().replacement.is_some());
        assert!(
            !app.world().non_send::<SearchPanels>().0[&entity]
                .search
                .canceled
        );
        let requests = app.world_mut().resource_mut::<HostEffects>().take();
        let (token, result) = requests
            .into_iter()
            .find_map(|intent| {
                if let HostAction::ApplyModuleEditsWithResult {
                    token,
                    document,
                    revision,
                    edits,
                    ..
                } = intent.action
                {
                    let result = app
                        .world_mut()
                        .non_send_mut::<DocumentSession>()
                        .apply_edits(document, revision, &edits)
                        .and_then(|_| {
                            app.world()
                                .non_send::<DocumentSession>()
                                .document_revision(document)
                        })
                        .map_err(|error| error.to_string());
                    Some((token, result))
                } else {
                    None
                }
            })
            .expect("replacement requests its first edit");
        app.world_mut().write_message(DocumentReply::Edit {
            recipient: "other.feature".into(),
            token,
            result: result.clone(),
        });
        app.update();
        assert!(
            !app.world_mut()
                .resource_mut::<HostEffects>()
                .take()
                .iter()
                .any(|intent| matches!(intent.action, HostAction::SaveDocumentsWithResult { .. })),
            "another feature cannot acknowledge this edit"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"alpha alpha");
        app.world_mut().write_message(DocumentReply::Edit {
            recipient: MODULE_ID.into(),
            token,
            result,
        });
        app.update();
        let requests = app.world_mut().resource_mut::<HostEffects>().take();
        let save = requests
            .into_iter()
            .find_map(|intent| {
                if let HostAction::SaveDocumentsWithResult {
                    token, documents, ..
                } = intent.action
                {
                    Some((token, documents))
                } else {
                    None
                }
            })
            .expect("acknowledged edit requests its save barrier");
        assert_eq!(save.1, [document]);
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .save(document)
            .unwrap();
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        app.world_mut().write_message(DocumentReply::Save {
            recipient: MODULE_ID.into(),
            token: save.0,
            result: Ok(vec![bedtime_api::documents::SavedDocument {
                document,
                revision,
            }]),
        });
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().non_send::<SearchState>().replacement.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), b"beta beta");
        assert!(
            app.world().non_send::<SearchPanels>().0[&entity]
                .search
                .message
                .as_deref()
                .unwrap()
                .contains("saved 1 files")
        );
    }
}
