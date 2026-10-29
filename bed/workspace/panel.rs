//! The Workspaces panel: every folder bed has a workspace for, to open in this
//! window or delete. Deleting removes bed's saved layout and settings for the
//! folder, never the folder. The default workspace isn't listed: it's where bed
//! is without a project, not a workspace you manage.
use super::{RecentWorkspaces, Start, Workspace, available, delete};
use crate::{menus::switch, panels::PanelPolicy};
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::WorkbenchWorldExt;
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, ErrorDialog, Handler, Panel,
    PanelFactory, PanelInstance, persistence::ReflectPersist,
};
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct OpenWorkspaces;
impl Action for OpenWorkspaces {
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
            .add_action_handler::<OpenWorkspaces>(Handler::new(open_workspaces));
    }
}

fn open_workspaces(world: &mut World, _: ActionInvocation) -> ActionResult {
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
enum Choice {
    Open(PathBuf),
    Delete(PathBuf),
}

impl Panel for WorkspacesPanel {
    const LABEL: &'static str = "Workspaces";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
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
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Workspaces").strong());
                ui.label(
                    egui::RichText::new(if list.is_empty() {
                        "None yet. Open a folder (File → Open Folder…) to make one."
                    } else {
                        "Click one to open it in this window."
                    })
                    .small()
                    .weak(),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for known in &list {
                            let row = Row {
                                known,
                                here: !current.default && known.folder == current.root,
                                confirming: confirming.as_ref() == Some(&known.folder),
                            };
                            match row.draw(ui, context) {
                                Some(RowAction::Open) => {
                                    action = Some(Choice::Open(known.folder.clone()));
                                }
                                Some(RowAction::AskToDelete) => {
                                    confirming = Some(known.folder.clone());
                                }
                                Some(RowAction::Delete) => {
                                    action = Some(Choice::Delete(known.folder.clone()));
                                    confirming = None;
                                }
                                Some(RowAction::Cancel) => confirming = None,
                                None => {}
                            }
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
            Some(Choice::Open(folder)) => switch(world, Start::Folder(folder)),
            Some(Choice::Delete(folder)) => forget(world, &folder, config.as_deref()),
            None => {}
        }
    }
}

/// One workspace in the list.
struct Row<'a> {
    known: &'a Known,
    /// This window's workspace.
    here: bool,
    /// Asking whether to delete it.
    confirming: bool,
}

enum RowAction {
    Open,
    AskToDelete,
    Delete,
    Cancel,
}

impl Row<'_> {
    /// A full-width line: folder icon, name, dimmed path; on the right, the
    /// status and a delete button. Clicking the line opens the workspace.
    fn draw(&self, ui: &mut egui::Ui, context: &UiContext<'_>) -> Option<RowAction> {
        let known = self.known;
        let name = known.folder.file_name().map_or_else(
            || known.folder.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let status = if self.here {
            Some("This window")
        } else if known.open_elsewhere {
            Some("Open in another window")
        } else if known.missing {
            Some("Folder missing")
        } else {
            None
        };
        let openable = status.is_none() && !self.confirming;
        let height = bedtime_ui::theme::control_height(context.metrics) + 4.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        // The line first, so the buttons drawn on it take their own clicks.
        let line = ui.interact(
            rect,
            ui.id().with(("workspace", &known.folder)),
            if openable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        line.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, openable, &name));
        // The labels drawn over the line take egui's hover, so ask about the
        // pointer instead.
        let hovered = ui.rect_contains_pointer(rect);
        let palette = &context.theme.palette;
        // Tints of the text colour show on any background, including the
        // window's when the panel has none.
        let tint = |color: bedtime_ui::theme::Rgba, alpha: f32| {
            let mut color = color;
            color[3] = alpha;
            color
        };
        let fill = if self.confirming {
            Some(tint(palette.error, 0.14))
        } else if openable && hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            Some(tint(palette.text, 0.08))
        } else if self.here {
            Some(tint(palette.text, 0.04))
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(
                rect,
                context.metrics.rounding,
                bedtime_ui::theme::color(fill),
            );
        }

        let mut result = line.clicked().then_some(RowAction::Open);
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        // Right to left: actions and status, then the name and path take
        // what's left.
        if self.confirming {
            if row.button("Cancel").clicked() {
                result = Some(RowAction::Cancel);
            }
            let delete =
                egui::RichText::new("Delete").color(bedtime_ui::theme::color(palette.error));
            if row.button(delete).clicked() {
                result = Some(RowAction::Delete);
            }
            row.weak("Delete its saved layout and settings?");
        } else {
            let deletable = !self.here && !known.open_elsewhere;
            row.add_enabled_ui(deletable, |ui| {
                let mut frame = context.frame(ui);
                let button = frame.icon_button_sized(BuiltinIcon::Trash, 14.0);
                let label = format!("Delete {name}");
                button.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, deletable, &label)
                });
                frame.tooltip(&button, "Delete this workspace's saved layout and settings");
                if button.clicked() {
                    result = Some(RowAction::AskToDelete);
                }
            });
            if let Some(status) = status {
                row.weak(status);
            }
        }
        row.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let side = 16.0;
            let (icon, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
            context
                .frame(ui)
                .icon_at(BuiltinIcon::Folder, [icon.min.x, icon.min.y], side);
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new(&name).strong()).selectable(false));
            // Where it is; the name is already shown.
            let parent = known.folder.parent().unwrap_or(&known.folder);
            let path = egui::RichText::new(abbreviate(parent)).small().weak();
            ui.add(egui::Label::new(path).truncate().selectable(false));
        });
        result
    }
}

/// Delete `folder`'s workspace and take it off the recent list.
fn forget(world: &mut World, folder: &Path, config: Option<&Path>) {
    let result = (|| {
        let config = config.ok_or("There's no configuration directory")?;
        delete(&folder_workspace(folder), config)?;
        world.resource_mut::<RecentWorkspaces>().forget(folder);
        Ok::<_, String>(())
    })();
    world.resource_mut::<KnownWorkspaces>().checked = None;
    if let Err(error) = result {
        world.workbench_mut().request_error_dialog(ErrorDialog {
            title: "Delete Workspace".into(),
            message: format!("Couldn't delete the workspace for {}.", folder.display()),
            details: Some(error),
        });
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
