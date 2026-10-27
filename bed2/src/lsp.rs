//! Application bridge for local language tools. Source identity/settings and
//! Files navigation remain application policy; the reusable plugin owns sync.
mod documentation;
mod interaction;
use bedtime_editor2::{Navigation, Reveal, TextBuffer, TextSelection, workbench::CodeLanguage};
use bedtime_lsp2::{Location, workbench::*};
use bedtime_ui2::egui;
use bedtime_workbench2::*;
use bevy::{ecs::message::MessageCursor, prelude::*};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
struct LanguageState {
    saves: MessageCursor<DocumentSaveFinished>,
    opening: Vec<Opening>,
    navigating: Vec<Navigating>,
    saves_waiting: VecDeque<(DocumentId, TextBuffer)>,
}

struct Opening {
    source: DefinitionResult,
    location: Location,
    opened: crate::opening::OpenedFile,
    started: Instant,
}

struct Navigating {
    source: DefinitionResult,
    target: Entity,
    path: std::path::PathBuf,
    task: ManagedTask<Result<Navigation, String>>,
    opened: crate::opening::OpenedFile,
}

#[derive(Component, Default)]
struct LanguageError(Option<(String, Instant)>);

#[derive(Component)]
struct Choices(DefinitionResult);

pub struct LanguageToolsPlugin;
impl Plugin for LanguageToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LspPlugin, crate::lsp_panel::LspPanelPlugin))
            .init_resource::<LanguageState>()
            .add_systems(
                Update,
                (
                    project
                        .before(LspSystems::Update)
                        .after(crate::files::FilesSystems::Update),
                    navigate.after(LspSystems::Update),
                    interaction::receive.after(LspSystems::Update),
                ),
            );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_lsp_interaction", interaction::inspect)
                .register_automation_command("app.inspect_lsp", inspect)
                .register_automation_command("app.lsp_definition", |world, value| {
                    let panel = parse_panel(&value)?;
                    request_definition(world, panel)?;
                    Ok(serde_json::json!({"queued":true}))
                })
                .register_automation_command("app.lsp_restart", |world, value| {
                    let panel = parse_panel(&value)?;
                    let doc = world
                        .get::<DocumentView>(panel)
                        .ok_or("Panel has no document")?
                        .document();
                    restart(world, doc)?;
                    Ok(serde_json::json!({"queued":true}))
                });
        }
    }
}

fn project(world: &mut World) {
    {
        let desired = (|| {
            if world
                .get_resource::<crate::files::ProjectFolder>()
                .is_none()
                || !crate::storage::ready(world)
            {
                return None;
            }
            let global = world.get_resource::<crate::settings::GlobalPreferences>()?;
            let overrides = world.get_resource::<crate::settings::ProjectPreferences>();
            if !overrides
                .and_then(|p| p.0.language_servers_enabled)
                .unwrap_or(global.0.language_servers_enabled)
            {
                return None;
            }
            let servers = overrides
                .and_then(|p| p.0.language_servers.as_ref())
                .unwrap_or(&global.0.language_servers);
            if servers.values().any(|s| s.validate().is_err()) {
                return None;
            }
            let root = crate::files::project_directory(world)?;
            if world
                .get_resource::<LspProject>()
                .is_some_and(|p| p.root == root && &p.servers == servers)
            {
                return Some(None);
            }
            Some(Some(LspProject {
                root,
                servers: servers.clone(),
            }))
        })();
        match desired {
            Some(Some(project)) => {
                world.insert_resource(project);
            }
            Some(None) => {}
            None => {
                world.remove_resource::<LspProject>();
            }
        }
    }
    let docs: Vec<_> = world
        .query::<(Entity, &CodeLanguage)>()
        .iter(world)
        .map(|(doc, language)| (doc, language.0.clone()))
        .collect();
    for (doc, language) in docs {
        let source = crate::file_editor::source_path(world, DocumentId(doc)).and_then(|path| {
            if language == "text" {
                return None;
            }
            let id = match (language.as_str(), path.extension().and_then(|s| s.to_str())) {
                ("typescript", Some("tsx")) => "typescriptreact",
                ("javascript", Some("jsx")) => "javascriptreact",
                _ => &language,
            };
            bedtime_lsp2::FileUri::from_path(&path)
                .ok()
                .map(|uri| LspDocument {
                    uri,
                    language: id.into(),
                })
        });
        if let Some(source) = source {
            if world.get::<LspDocument>(doc) != Some(&source) {
                world.entity_mut(doc).insert(source);
            }
        } else {
            world.entity_mut(doc).remove::<LspDocument>();
        }
    }
    let attached: Vec<_> = world
        .query_filtered::<Entity, With<LspDocument>>()
        .iter(world)
        .collect();
    for doc in attached {
        if world.get::<CodeLanguage>(doc).is_none()
            || crate::file_editor::source_path(world, DocumentId(doc)).is_none()
        {
            world.entity_mut(doc).remove::<LspDocument>();
        }
    }
    let completions: Vec<_> = world.resource_scope(|world, mut state: Mut<LanguageState>| {
        world
            .get_resource::<Messages<DocumentSaveFinished>>()
            .map(|messages| state.saves.read(messages).cloned().collect())
            .unwrap_or_default()
    });
    for completion in completions {
        if let Some(text) = crate::file_editor::take_saved_text(world, &completion) {
            world
                .resource_mut::<LanguageState>()
                .saves_waiting
                .push_back((completion.document, text));
        }
    }
    let mut saved = std::mem::take(&mut world.resource_mut::<LanguageState>().saves_waiting);
    while let Some((document, text)) = saved.pop_front() {
        if notify_saved(world, document, text.clone()).is_err() {
            world
                .resource_mut::<LanguageState>()
                .saves_waiting
                .push_back((document, text));
        }
    }
}

fn show_error(world: &mut World, panel: Entity, error: String) {
    if let Ok(mut entity) = world.get_entity_mut(panel) {
        entity.insert(LanguageError(Some((error, Instant::now()))));
    }
}

pub(crate) fn error_for_panel(world: &World, panel: Entity) -> Option<(&str, Instant)> {
    let (message, reported_at) = world.get::<LanguageError>(panel)?.0.as_ref()?;
    Some((message, *reported_at))
}

fn navigation_current(world: &World, source: &DefinitionResult, target: Entity) -> bool {
    if !definition_ticket_is_current(world, &source.origin) {
        return false;
    }
    let Some(document) = Entity::try_from_bits(source.origin.document.id) else {
        return false;
    };
    let Some(panel) = Entity::try_from_bits(source.origin.panel) else {
        return false;
    };
    let source_current = match inspect_document(world, DocumentId(document)) {
        Ok(info) => info.revision == source.origin.revision && !info.has_drafts,
        // Replacing a clean preview deliberately retires its old canonical file
        // document. The already-accepted open is now guarded by its load stamp.
        Err(DocumentError::Unavailable(_)) => panel == target,
        Err(_) => false,
    };
    source_current
        && world.get::<PanelInstance>(panel).is_some()
        && (panel == target
            || world
                .get::<DocumentView>(panel)
                .is_some_and(|view| view.document() == DocumentId(document)))
}

fn start_open(
    world: &mut World,
    source: DefinitionResult,
    location: Location,
) -> Result<(), String> {
    if !definition_is_current(world, &source) {
        return Ok(());
    }
    let panel = Entity::try_from_bits(source.origin.panel).ok_or("Definition source has closed")?;
    let workbench = Workbench::for_entity(world, panel)?;
    let layout = workbench.panel_layout(panel)?;
    let opened = crate::opening::open_file(
        world,
        layout,
        location.uri.path(),
        crate::opening::FileOpenMode::Preview,
    )?;
    world.resource_mut::<LanguageState>().opening.push(Opening {
        source,
        location,
        opened,
        started: Instant::now(),
    });
    Ok(())
}

fn navigate(world: &mut World) {
    let results = take_definitions(world);
    for result in results {
        let Some(panel) = Entity::try_from_bits(result.origin.panel) else {
            continue;
        };
        if !definition_is_current(world, &result) {
            continue;
        }
        if result.locations.is_empty() {
            show_error(world, panel, "No definition found".into());
        } else if result.locations.len() == 1 {
            let location = result.locations[0].clone();
            if let Err(error) = start_open(world, result, location) {
                show_error(world, panel, error);
            }
        } else {
            world.entity_mut(panel).insert(Choices(result));
        }
    }
    let pending = std::mem::take(&mut world.resource_mut::<LanguageState>().opening);
    for open in pending {
        if !navigation_current(world, &open.source, open.opened.panel) {
            continue;
        }
        let panel = Entity::try_from_bits(open.source.origin.panel).unwrap();
        let ready = match open.opened.result(world) {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                show_error(world, panel, error);
                continue;
            }
            None => false,
        };
        if !ready {
            if open.started.elapsed() < Duration::from_secs(30) {
                world.resource_mut::<LanguageState>().opening.push(open);
            } else {
                show_error(world, panel, "Opening definition timed out".into());
            }
            continue;
        }
        let result = (|| {
            let target = open.opened.panel;
            let document = world
                .get::<DocumentView>(target)
                .ok_or("Definition target is not a text document")?
                .document();
            let handle = cast_document::<TextBuffer>(world, document).map_err(|e| e.to_string())?;
            let text = read_document(world, handle)
                .map_err(|e| e.to_string())?
                .clone();
            let revision = inspect_document(world, document)
                .map_err(|e| e.to_string())?
                .revision;
            let location = open.location.clone();
            let task = Workbench::for_entity(world, target)?.work().spawn_task(
                Some(target),
                "Definition position",
                move |cancel| {
                    if cancel.is_cancelled() {
                        return Err("Navigation cancelled".into());
                    }
                    let range = location.encoding.range(&text, location.range)?;
                    let selection = Some(
                        TextSelection::new(&text, range.start, range.end)
                            .map_err(|e| e.to_string())?,
                    );
                    Ok(Navigation {
                        revision,
                        range,
                        selection,
                        reveal: Reveal::Center,
                        focus: true,
                    })
                },
            )?;
            Ok::<_, String>(Navigating {
                source: open.source,
                target,
                path: open.location.uri.path(),
                task,
                opened: open.opened,
            })
        })();
        match result {
            Ok(n) => world.resource_mut::<LanguageState>().navigating.push(n),
            Err(error) => show_error(world, panel, error),
        }
    }
    let pending = std::mem::take(&mut world.resource_mut::<LanguageState>().navigating);
    for pending in pending {
        if !navigation_current(world, &pending.source, pending.target)
            || !matches!(pending.opened.result(world), Some(Ok(())))
        {
            pending.task.cancel();
            continue;
        }
        let panel = Entity::try_from_bits(pending.source.origin.panel).unwrap();
        match pending.task.try_recv() {
            Ok(Ok(navigation)) => {
                if crate::files::panel_path(world, pending.target) != Some(pending.path) {
                    show_error(
                        world,
                        panel,
                        "Definition target changed while opening".into(),
                    );
                    continue;
                }
                if let Err(error) =
                    bedtime_editor2::workbench::navigate_editor(world, pending.target, navigation)
                {
                    show_error(world, panel, error.to_string());
                }
            }
            Ok(Err(error)) => show_error(world, panel, error),
            Err(std::sync::mpsc::TryRecvError::Empty) => world
                .resource_mut::<LanguageState>()
                .navigating
                .push(pending),
            Err(_) => show_error(world, panel, "Definition position worker stopped".into()),
        }
    }
}

fn invoke(world: &mut World, panel: Entity) {
    world.entity_mut(panel).insert(LanguageError(None));
    if let Err(error) = request_definition(world, panel) {
        show_error(world, panel, error);
    }
}

pub fn editor_input(
    ui: &mut egui::Ui,
    world: &mut World,
    panel: Entity,
    output: &bedtime_editor2::EditorOutput,
) {
    if !crate::close::accepts_user_actions(world) {
        return;
    }
    interaction::draw(ui, world, panel, output);
    if output.response.has_focus()
        && !output.composing
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12))
    {
        invoke(world, panel);
    }
    if let Some(choices) = world.entity_mut(panel).take::<Choices>() {
        if definition_is_current(world, &choices.0) {
            let mut selected = None;
            let mut keep = true;
            egui::Window::new("Definitions")
                .id(egui::Id::new(("definitions", panel)))
                .open(&mut keep)
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    for (index, location) in choices.0.locations.iter().enumerate() {
                        if ui
                            .button(format!(
                                "{}:{}",
                                location.uri.path().display(),
                                location.range.start.line + 1
                            ))
                            .clicked()
                        {
                            selected = Some(index);
                        }
                    }
                });
            if let Some(index) = selected {
                let location = choices.0.locations[index].clone();
                if let Err(error) = start_open(world, choices.0, location) {
                    show_error(world, panel, error);
                }
            } else if keep {
                world.entity_mut(panel).insert(choices);
            }
        }
    }
}

pub fn context_menu(ui: &mut egui::Ui, world: &mut World, panel: Entity) {
    let Some(activity) = language_activity(world, panel) else {
        return;
    };
    if activity.supports_definition {
        ui.separator();
        let composing = world
            .get::<bedtime_editor2::workbench::WorkbenchEditor>(panel)
            .is_some_and(|editor| editor.view.is_composing());
        if ui
            .add_enabled(!composing, egui::Button::new("Go to Definition     F12"))
            .clicked()
        {
            invoke(world, panel);
            ui.close();
        }
    }
}

#[cfg(feature = "automation")]
fn parse_panel(value: &serde_json::Value) -> Result<Entity, String> {
    Entity::try_from_bits(
        value["panel"]
            .as_str()
            .ok_or("panel must be an entity ID string")?
            .parse()
            .map_err(|_| "Invalid panel ID")?,
    )
    .ok_or("Invalid panel ID".into())
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> Result<serde_json::Value, String> {
    let docs: Vec<_> = world
        .query_filtered::<Entity, With<LspDocument>>()
        .iter(world)
        .collect();
    let docs:Vec<_>=docs.into_iter().map(|doc| {
        let source=world.get::<LspDocument>(doc).unwrap();let state=world.get::<DocumentLsp>(doc);
        serde_json::json!({"document":doc.to_bits().to_string(),"uri":source.uri.as_str(),"language":source.language,"status":format!("{:?}",server_status(world,DocumentId(doc))),"error":state.and_then(|s|s.error.as_ref()),"diagnostics":state.and_then(|s|s.diagnostics.as_ref()).map(|r|serde_json::json!({"revision":r.revision,"verified":r.verified,"items":r.items().iter().map(|d|serde_json::json!({"start":d.range.start,"end":d.range.end,"message":d.message.as_ref()})).collect::<Vec<_>>()}))})
    }).collect();
    Ok(serde_json::json!({"documents":docs}))
}
