//! bEd startup and numbered-layout policy over the shared runtime.
pub(super) mod layouts;
use crate::application::workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};
#[cfg(test)]
use crate::config::AppConfig;
pub use crate::config::LayoutSlot;
use bedtime_api::documents::PanelInput;
use bedtime_api::features::HostAction;
pub use bedtime_core::{DocumentNavigation, RemoteHelperDirectory};
use bedtime_core::{active_panel_entities, panel_entities, retire_panels};
use bedtime_documents::DocumentSession;
use bedtime_plugin_workspaces::{
    WorkspaceAction, WorkspaceAttachment, WorkspaceRepositoryResource,
};
#[cfg(test)]
use bedtime_workbench::PanelPresentation;
use bedtime_workbench::{PanelRecord, WorkbenchLayout};
use bevy::prelude::*;
pub use layouts::LayoutSlots;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutCommand {
    SetCurrentLayout,
    ResetLayout,
    UseLayout(LayoutSlot),
    SetLayoutDefault(LayoutSlot),
    RestoreBuiltinLayout(LayoutSlot),
    SaveDefaultLayout,
    ResetDefaultLayout,
}
pub use bedtime_plugin_layouts::LayoutRequestId;
#[derive(Resource, Default)]
struct LayoutNotifications(Vec<LayoutRequestId>);

pub fn submit_layout_command(world: &mut World, command: LayoutCommand) -> LayoutRequestId {
    if let LayoutCommand::UseLayout(slot) = command {
        return layouts::request_switch(world, slot);
    }
    if matches!(command, LayoutCommand::ResetLayout) {
        return layouts::request_reset(world);
    }
    let result = apply_layout_command(world, command).map_err(|error| error.to_string());
    bedtime_plugin_layouts::completed_request::<LayoutSlot>(world, result)
}
pub fn take_layout_result(world: &mut World, id: LayoutRequestId) -> Option<Result<(), String>> {
    bedtime_plugin_layouts::take_result::<LayoutSlot>(world, id)
}
pub fn apply_layout_command(world: &mut World, command: LayoutCommand) -> io::Result<()> {
    match command {
        LayoutCommand::UseLayout(_) | LayoutCommand::ResetLayout => {
            let id = submit_layout_command(world, command);
            if let Some(result) = take_layout_result(world, id) {
                result.map_err(io::Error::other)?;
            } else {
                world.resource_mut::<LayoutNotifications>().0.push(id);
            }
        }
        LayoutCommand::SetCurrentLayout => {
            layouts::save_default(world, world.resource::<LayoutSlots>().active())?
        }
        LayoutCommand::SetLayoutDefault(slot) => layouts::save_default(world, slot)?,
        LayoutCommand::RestoreBuiltinLayout(slot) => world
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_layout_default(slot, None)?,
        LayoutCommand::SaveDefaultLayout => layouts::save_default(world, LayoutSlot::Layout1)?,
        LayoutCommand::ResetDefaultLayout => world
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_layout_default(LayoutSlot::Layout1, None)?,
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct OpenFileOptions {
    pub path: PathBuf,
    pub viewer: Option<String>,
    pub additional: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    pub working_directory: Option<PathBuf>,
    pub workspace: Option<WorkspaceSpec>,
    pub files: Vec<OpenFileOptions>,
    /// Initial numbered arrangement for a fresh bEd window.
    pub initial_layout: Option<LayoutSlot>,
    /// Initial tool, opened before file arguments so those files retain focus.
    pub startup_panel: Option<String>,
}

#[derive(Resource, Clone, Debug)]
pub struct ApplicationState {
    pub error: Option<String>,
    pub windows: Vec<WorkspaceSpec>,
}

/// bEd's numbered global templates retain their current application-owned schema.
pub struct WorkspacePersistence {
    pub(crate) store: WorkspaceStore,
}
impl WorkspacePersistence {
    pub fn new(store: WorkspaceStore) -> Self {
        Self { store }
    }
    pub(crate) fn install(app: &mut App, store: WorkspaceStore) {
        app.add_plugins(bedtime_plugin_workspaces::WorkspacePersistencePlugin::new(
            store.repository(),
        ));
        app.insert_non_send(Self::new(store));
    }
}

#[derive(Default)]
pub struct AppRuntimePlugin {
    pub options: AppOptions,
}
#[derive(Resource)]
pub(crate) struct InitialAppOptions(pub(crate) Option<AppOptions>);
#[derive(Resource, Default)]
pub(crate) struct AppInitialized(pub(crate) bool);
#[derive(Resource, Default)]
struct AppObservation {
    workspace_error: Option<String>,
    notices: bevy::ecs::message::MessageCursor<bedtime_core::CoreNotice>,
}

impl Plugin for AppRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_core::DocumentHostPlugin>() {
            app.add_plugins(bedtime_core::DocumentHostPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_workspaces::WorkspacesIntegrationPlugin>() {
            app.add_plugins(bedtime_plugin_workspaces::WorkspacesIntegrationPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_workspaces::WorkspacePersistencePlugin>()
            && app.world().contains_non_send::<WorkspacePersistence>()
        {
            let repository = app
                .world()
                .non_send::<WorkspacePersistence>()
                .store
                .repository();
            app.add_plugins(bedtime_plugin_workspaces::WorkspacePersistencePlugin::new(
                repository,
            ));
        }
        app.add_plugins(bedtime_core::layouts::DocumentLayoutsPlugin::new(
            LayoutSlot::Layout1,
        ));
        app.init_resource::<LayoutNotifications>()
            .init_resource::<LayoutSlots>()
            .init_resource::<AppInitialized>()
            .init_resource::<AppObservation>()
            .init_resource::<WorkspaceAttachment>()
            .insert_resource(bedtime_core::DocumentClosePolicy(
                bedtime_documents::ClosePolicy::Save,
            ))
            .insert_resource(InitialAppOptions(Some(self.options.clone())))
            .add_systems(
                PreUpdate,
                publish_app_state
                    .after(bedtime_core::CoreSystems::Services)
                    .before(bedtime_core::CoreSystems::Frame),
            )
            .add_systems(
                PostUpdate,
                update_application_policy.after(bedtime_plugin_layouts::LayoutSystems::Requests),
            )
            .add_systems(Startup, initialize_app);
        let world = app.world_mut();
        let directory = self
            .options
            .working_directory
            .clone()
            .expect("bEd startup supplies its working directory");
        let (context, services) = if let (Some(target), Some(client)) = (
            world.non_send::<DocumentSession>().ssh_target().cloned(),
            world.non_send::<DocumentSession>().remote_client(),
        ) {
            let root = world
                .non_send::<DocumentSession>()
                .options()
                .project_root
                .clone()
                .unwrap_or(directory);
            (
                bedtime_files::FileContext::remote(target.host.clone(), root)
                    .expect("injected remote root is canonical"),
                bedtime_files::FileServices::remote_target(target, client),
            )
        } else {
            (
                bedtime_files::FileContext::local(directory).expect("startup directory is valid"),
                bedtime_files::FileServices::local(),
            )
        };
        bedtime_core::initialize_file_context(world, context.clone(), services)
            .expect("startup file context is valid");
        world.insert_resource(ApplicationState {
            error: None,
            windows: Vec::new(),
        });
        crate::application::command_registration::register(world);
        let persist = world.register_system(|world: &mut World| -> Result<(), String> {
            persist_workspace(world).map_err(|error| error.to_string())
        });
        world.insert_resource(bedtime_plugin_layouts::LayoutPersistence(persist));
        {
            let snapshot = world.register_system(|world: &mut World| -> Value {
                layouts::snapshot(world).expect("bEd layout snapshot is valid")
            });
            world.insert_resource(bedtime_plugin_workspaces::WorkspaceLayoutSnapshot(snapshot));
            let preflight = world.register_system(|In(value): In<Value>| -> Result<(), String> {
                validate_numbered_workspace_layout(&value)
            });
            world.insert_resource(bedtime_plugin_workspaces::WorkspaceLayoutPreflight(
                preflight,
            ));
            let restore = world.register_system(restore_numbered_workspace_layout);
            world.insert_resource(bedtime_plugin_workspaces::WorkspaceLayoutRestore(restore));
            let entered = world.register_system(|world: &mut World| -> Result<(), String> {
                if matches!(
                    world.resource::<WorkspaceAttachment>(),
                    WorkspaceAttachment::Saved(_)
                ) {
                    return Ok(());
                }
                layouts::enter_directory(world).map_err(|error| error.to_string())
            });
            world.insert_resource(bedtime_plugin_workspaces::WorkspaceDirectoryEntered(
                entered,
            ));
            let window = world.register_system(
                |In(spec): In<WorkspaceSpec>, world: &mut World| -> Result<(), String> {
                    world.resource_mut::<ApplicationState>().windows.push(spec);
                    Ok(())
                },
            );
            world.insert_resource(bedtime_plugin_workspaces::WorkspaceWindowHandler(window));
        }
    }
}

fn validate_numbered_workspace_layout(value: &Value) -> Result<(), String> {
    LayoutSlot::from_value(&value["active_layout_slot"]).ok_or("Invalid active layout slot")?;
    let layouts = value["layouts"]
        .as_object()
        .ok_or("Saved layouts must be an object")?;
    for (slot, layout) in layouts {
        LayoutSlot::from_key(slot).ok_or_else(|| format!("Invalid layout slot: {slot}"))?;
        bedtime_core::validate_document_layout(layout)
            .map_err(|error| format!("Layout {slot}: {error}"))?;
    }
    Ok(())
}

fn restore_numbered_workspace_layout(
    In(restore): In<bedtime_plugin_workspaces::WorkspaceRestore>,
    world: &mut World,
) -> Result<(), String> {
    let result = (|| -> io::Result<()> {
        if let Some(panel) = restore.preserved_panel {
            // Core has saved and retired the surrounding workbench. Place the
            // live caller into the selected workspace's numbered arrangement.
            layouts::clear(world);
            if restore
                .layouts
                .as_ref()
                .is_some_and(|value| value["layouts"].is_object())
            {
                match layouts::load_snapshot(world, restore.layouts.as_ref().unwrap())? {
                    Some(value) => restore_layout_with_terminal(world, &value, true, Some(panel))?,
                    None => compose_default_layout_with_terminal(world, Some(panel))?,
                }
            } else {
                world.insert_resource(LayoutSlots::new(LayoutSlot::Layout1));
                compose_default_layout_with_terminal(world, Some(panel))?;
            }
            bedtime_workbench::advance_layout_epoch(world);
        } else if restore.retained_documents
            && restore.layouts.as_ref().is_some_and(|value| {
                value["active_layout_slot"].is_number() || value["active_layout_slot"].is_string()
            })
        {
            layouts::attach(
                world,
                restore.preserved_panel,
                restore.layouts.as_ref().unwrap(),
            )?;
        } else if restore.retained_documents {
            layouts::enter_directory(world)?;
        } else {
            layouts::clear(world);
            if restore.layouts.as_ref().is_some_and(|value| {
                value["active_layout_slot"].is_number() || value["active_layout_slot"].is_string()
            }) {
                match layouts::load_snapshot(world, restore.layouts.as_ref().unwrap())? {
                    Some(value) => restore_layout(world, &value, false)?,
                    None => compose_default_layout(world)?,
                }
            } else {
                layouts::initialize(world, LayoutSlot::Layout1)?;
            }
        }
        Ok(())
    })();
    result.map_err(|error| error.to_string())
}

fn initialize_app(world: &mut World) {
    let Some(options) = world.resource_mut::<InitialAppOptions>().0.take() else {
        return;
    };
    let result = (|| -> io::Result<()> {
        crate::config::ensure_settings_view(world)?;
        bedtime_core::settings::configure_document_settings(world)?;
        if let Some(spec) = options.workspace {
            let injected_remote = matches!(&spec.target,WorkspaceTarget::Ssh {host}
                if world.non_send::<DocumentSession>().ssh_target().is_some_and(|target|&target.host==host)
                && world.non_send::<DocumentSession>().options().project_root.as_deref()==Some(Path::new(&spec.root)));
            if injected_remote {
                world.insert_resource(WorkspaceAttachment::Saved(spec));
                restore_workspace(world)?;
            } else {
                open_startup_workspace(world, spec)?;
            }
        } else if let Some(slot) = options.initial_layout {
            layouts::initialize(world, slot)?;
        }
        if let Some(kind) = options.startup_panel.as_deref()
            && panel_entities(world).is_empty()
            && world
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .panel(kind)
                .is_some()
        {
            bedtime_core::open_panel(
                world,
                kind,
                PanelInput::None,
                Value::Null,
                None,
                None,
                false,
            )
            .map_err(io::Error::other)?;
        }
        for file in options.files {
            bedtime_core::submit_host_request(
                world,
                None,
                HostAction::OpenFileInstance {
                    path: file.path.to_string_lossy().into_owned(),
                    viewer: file.viewer,
                    additional: file.additional,
                },
            );
        }
        bedtime_core::resolve_documents(world);
        update_application_policy(world);
        Ok(())
    })();
    if let Err(error) = result {
        world.resource_mut::<ApplicationState>().error = Some(error.to_string());
    }
    world.resource_mut::<AppInitialized>().0 = true;
}

pub fn update_application_policy(world: &mut World) {
    {
        let error = world
            .get_resource::<bedtime_plugin_workspaces::WorkspacesState>()
            .and_then(|state| state.error.clone());
        if error != world.resource::<AppObservation>().workspace_error {
            if let Some(error) = error.as_ref() {
                world.resource_mut::<ApplicationState>().error = Some(error.clone());
            }
            world.resource_mut::<AppObservation>().workspace_error = error;
        }
    }
    let notice = world.resource_scope(|world, mut observation: Mut<AppObservation>| {
        observation
            .notices
            .read(world.resource::<Messages<bedtime_core::CoreNotice>>())
            .last()
            .map(|notice| notice.message.clone())
    });
    if let Some(notice) = notice {
        world.resource_mut::<ApplicationState>().error = Some(notice);
    }
    let pending = std::mem::take(&mut world.resource_mut::<LayoutNotifications>().0);
    for id in pending {
        match take_layout_result(world, id) {
            Some(Err(error)) => world.resource_mut::<ApplicationState>().error = Some(error),
            Some(Ok(())) => {}
            None => world.resource_mut::<LayoutNotifications>().0.push(id),
        }
    }
}

pub fn publish_app_state(world: &mut World) {
    crate::application::publish_status_identity(world);
}
fn open_startup_workspace(world: &mut World, spec: WorkspaceSpec) -> io::Result<()> {
    let action = if matches!(spec.target, WorkspaceTarget::Local)
        && world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&spec)?
            .is_none()
    {
        bedtime_core::submit_host_request(
            world,
            None,
            HostAction::OpenDirectory {
                location: bedtime_files::FileLocation {
                    provider: bedtime_files::FileProviderId::Local,
                    path: PathBuf::from(spec.root),
                },
            },
        );
        bedtime_core::resolve_documents(world);
        return Ok(());
    } else {
        WorkspaceAction::OpenWorkspace(spec)
    };
    let area = world
        .resource::<WorkbenchLayout>()
        .tiling
        .layout
        .largest_area();
    let id = bedtime_plugin_workspaces::submit_workspace_request(world, action, None, area);
    bedtime_core::resolve_documents(world);
    bedtime_plugin_workspaces::resolve_workspace_requests(world);
    bedtime_core::resolve_documents(world);
    bedtime_plugin_workspaces::resolve_workspace_requests(world);
    if let Some(Err(error)) = bedtime_plugin_workspaces::take_workspace_result(world, id) {
        return Err(io::Error::other(error));
    }
    Ok(())
}
fn snapshot_layout(
    world: &mut World,
    panels: Vec<Entity>,
    layout: &WorkbenchLayout,
    navigation: &DocumentNavigation,
) -> io::Result<Value> {
    bedtime_core::snapshot_document_layout(world, panels, layout, navigation)
        .map_err(io::Error::other)
}
fn restore_layout(world: &mut World, value: &Value, attach: bool) -> io::Result<()> {
    restore_layout_with_terminal(world, value, attach, None)
}
fn restore_layout_with_terminal(
    world: &mut World,
    value: &Value,
    attach: bool,
    terminal: Option<Entity>,
) -> io::Result<()> {
    bedtime_core::restore_document_layout(world, value, attach, terminal).map_err(io::Error::other)
}
fn compose_default_layout(world: &mut World) -> io::Result<()> {
    compose_default_layout_with_terminal(world, None)
}
fn compose_default_layout_with_terminal(
    world: &mut World,
    terminal: Option<Entity>,
) -> io::Result<()> {
    let slot = world.resource::<LayoutSlots>().active();
    restore_layout_with_terminal(world, &layouts::template(world, slot), true, terminal)
}
pub fn persist_workspace(world: &mut World) -> io::Result<()> {
    bedtime_plugin_workspaces::persist_workspace(world)?;
    world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .reload()?;
    Ok(())
}
pub fn save_current_workspace(
    world: &mut World,
    name: Option<String>,
) -> io::Result<WorkspaceSpec> {
    let snapshot = layouts::snapshot(world)?;
    let spec = bedtime_plugin_workspaces::save_current_workspace(world, name, snapshot)?;
    world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .reload()?;
    Ok(spec)
}
pub fn restore_workspace(world: &mut World) -> io::Result<()> {
    bedtime_plugin_workspaces::restore_current_workspace(world)?;
    Ok(())
}
#[cfg(test)]
pub(crate) fn test_application_paths(config: &Path) -> bedtime_api::launch::ApplicationPaths {
    bedtime_api::launch::ApplicationPaths::new("bed", Some(config.to_owned()), None).unwrap()
}

#[cfg(test)]
pub(crate) fn install_test_infrastructure(app: &mut App, config: &Path) {
    let settings = AppConfig::with_paths(
        config.to_owned(),
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
    )
    .unwrap();
    let paths = test_application_paths(config);
    app.insert_resource(paths.clone())
        .insert_non_send(bedtime_ui::icons::Icons::default())
        .insert_non_send(settings)
        .insert_resource(RemoteHelperDirectory(PathBuf::new()));
    WorkspacePersistence::install(app, WorkspaceStore::load(&paths).unwrap());
}
