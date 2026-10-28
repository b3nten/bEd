//! The Workspaces panel: every folder bed has a workspace for, to open in this
//! window or delete. Deleting removes bed's saved layout and settings for the
//! folder, never the folder. The default workspace isn't listed: it's where bed
//! is without a project, not a workspace you manage.
use super::{RecentWorkspaces, Start, Workspace, available, delete};
use crate::{
    menus::switch,
    panels::PanelPolicy,
    settings::{CONTROL_WIDTH, GlobalLocation, columns},
};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{
    CommandAppExt, ContextCommand, ContextCommandInvocation, ContextCommandResult, ErrorDialog,
    Handler, Panel, PanelFactory, PanelInstance, persistence::DirectoryStore,
    persistence::ReflectPersist, request_error_dialog,
};
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct OpenWorkspaces;
impl ContextCommand for OpenWorkspaces {
    const KEY: &'static str = "app.open_workspaces";
    const LABEL: &'static str = "Workspaces";
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Persist)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct WorkspacesPanel;

impl PanelPolicy for WorkspacesPanel {
    /// There's one list of workspaces.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct WorkspacesPanelPlugin;

impl Plugin for WorkspacesPanelPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<WorkspacesPanel>()
            .init_resource::<KnownWorkspaces>()
            .add_command_handler::<OpenWorkspaces>(Handler::new(open_workspaces));
    }
}

fn open_workspaces(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
    let factory = PanelFactory::of::<WorkspacesPanel>();
    crate::panels::open_panel::<WorkspacesPanel>(world, (), factory).map(|_| ())
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
struct Known {
    folder: PathBuf,
    open_elsewhere: bool,
    missing: bool,
}

/// The list, looked up again about once a second while the panel is drawn:
/// other windows open and close workspaces.
#[derive(Resource, Default)]
struct KnownWorkspaces {
    list: Vec<Known>,
    checked: Option<Instant>,
}

/// Recent folders first, then any other folder with a saved workspace (its
/// store's `path.txt`).
fn look_up(recent: &RecentWorkspaces, config: Option<&Path>) -> Vec<Known> {
    let mut folders = recent.0.clone();
    if let Some(config) = config
        && let Ok(stores) = std::fs::read_dir(config.join("workspaces"))
    {
        let mut others: Vec<PathBuf> = stores
            .flatten()
            .filter(|store| store.file_name() != "default")
            .filter_map(|store| std::fs::read(store.path().join("path.txt")).ok())
            .map(|bytes| PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
            .filter(|folder| !folders.contains(folder))
            .collect();
        others.sort();
        folders.extend(others);
    }
    folders
        .into_iter()
        .map(|folder| {
            let workspace = folder_workspace(&folder);
            Known {
                open_elsewhere: config
                    .is_some_and(|config| !available(&workspace.store_directory(config))),
                missing: !folder.is_dir(),
                folder,
            }
        })
        .collect()
}

fn folder_workspace(folder: &Path) -> Workspace {
    Workspace {
        root: folder.to_owned(),
        default: false,
    }
}

/// What a row's buttons asked for.
enum Action {
    Open(PathBuf),
    Delete(PathBuf),
}

impl Panel for WorkspacesPanel {
    const LABEL: &'static str = "Workspaces";

    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, _: Entity) {
        let config = crate::config_directory();
        let stale = world
            .resource::<KnownWorkspaces>()
            .checked
            .is_none_or(|checked| checked.elapsed() > Duration::from_secs(1));
        if stale {
            let list = look_up(world.resource::<RecentWorkspaces>(), config.as_deref());
            let mut known = world.resource_mut::<KnownWorkspaces>();
            known.list = list;
            known.checked = Some(Instant::now());
        }
        let list = world.resource::<KnownWorkspaces>().list.clone();
        let current = world.resource::<Workspace>().clone();
        let confirming_id = ui.id().with("confirm-delete");
        let mut confirming: Option<PathBuf> = ui.data(|data| data.get_temp(confirming_id));
        let mut action = None;
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            if list.is_empty() {
                ui.weak("No workspaces yet. Open a folder to make one.");
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for known in &list {
                        let here = !current.default && known.folder == current.root;
                        let name = known.folder.file_name().map_or_else(
                            || known.folder.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        columns(
                            ui,
                            &name,
                            |ui| {
                                ui.set_max_width(CONTROL_WIDTH);
                                let path = egui::RichText::new(abbreviate(&known.folder)).weak();
                                ui.add(egui::Label::new(path).truncate())
                                    .on_hover_text(known.folder.display().to_string());
                            },
                            |ui| {
                                if confirming.as_ref() == Some(&known.folder) {
                                    if ui.button("Delete saved layout and settings").clicked() {
                                        action = Some(Action::Delete(known.folder.clone()));
                                        confirming = None;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        confirming = None;
                                    }
                                    return;
                                }
                                if here {
                                    ui.weak("This window");
                                } else if known.open_elsewhere {
                                    ui.weak("Open in another window");
                                } else if known.missing {
                                    ui.weak("Folder missing");
                                } else if ui.button("Open").clicked() {
                                    action = Some(Action::Open(known.folder.clone()));
                                }
                                let deletable = !here && !known.open_elsewhere;
                                if ui
                                    .add_enabled(deletable, egui::Button::new("Delete"))
                                    .on_hover_text(
                                        "Forget this workspace: its saved layout and settings",
                                    )
                                    .clicked()
                                {
                                    confirming = Some(known.folder.clone());
                                }
                            },
                        );
                    }
                });
        });
        ui.data_mut(|data| match &confirming {
            Some(folder) => {
                data.insert_temp(confirming_id, folder.clone());
            }
            None => data.remove::<PathBuf>(confirming_id),
        });
        match action {
            Some(Action::Open(folder)) => switch(world, Start::Folder(folder)),
            Some(Action::Delete(folder)) => forget(world, &folder, config.as_deref()),
            None => {}
        }
    }
}

/// Delete `folder`'s workspace and take it off the recent list.
fn forget(world: &mut World, folder: &Path, config: Option<&Path>) {
    let result = (|| {
        let config = config.ok_or("There's no configuration directory")?;
        delete(&folder_workspace(folder), config)?;
        let global = world.resource::<GlobalLocation>().0.clone();
        if let Some(global) = global {
            world
                .resource_mut::<RecentWorkspaces>()
                .forget(&DirectoryStore::new(global), folder)?;
        }
        Ok::<_, String>(())
    })();
    world.resource_mut::<KnownWorkspaces>().checked = None;
    if let Err(error) = result {
        request_error_dialog(
            world,
            ErrorDialog {
                title: "Delete Workspace".into(),
                message: format!("Couldn't delete the workspace for {}.", folder.display()),
                details: Some(error),
            },
        );
    }
}

/// `~/…` for folders in the home folder.
fn abbreviate(folder: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| folder.strip_prefix(home).ok())
    {
        Some(relative) => format!("~/{}", relative.display()),
        None => folder.display().to_string(),
    }
}
