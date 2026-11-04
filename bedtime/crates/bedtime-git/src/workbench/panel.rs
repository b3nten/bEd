mod tree;
use super::*;
use crate::{ConflictChoice, RepositoryOperation};
use bedtime_ui::{BuiltinIcon, UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;

/// The Git panel. Its draft commit message and view options are saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct GitPanel {
    pub draft: String,
    pub options: GitPanelOptions,
    #[reflect(ignore)]
    selected: Option<String>,
    #[reflect(ignore)]
    tree: tree::TreeView,
    #[reflect(ignore)]
    pub(super) rendered_rows: usize,
}

/// What the app does for the Git panel: open the repository's folder as the
/// workspace, and a terminal there (beside the panel). Without them, those
/// buttons don't show.
#[derive(Resource, Default, Clone, Copy)]
pub struct GitHost {
    pub open_folder: Option<fn(&mut World, PathBuf)>,
    pub open_terminal: Option<fn(&mut World, PathBuf, Entity)>,
}

/// Whether the app takes actions (it isn't quitting).
fn accepts_user_actions(world: &World) -> bool {
    world
        .get_resource::<ManagedWork>()
        .is_some_and(|work| !work.is_closing())
}

enum Action {
    Operation(Operation),
    Discard(String),
    Confirm,
    Cancel,
    Open(String),
    Diff(String, DiffSide, bool),
    Terminal,
    OpenRoot,
    Options(GitPanelOptions),
    ToggleDirectory(DirectoryKey),
}

impl Panel for GitPanel {
    const LABEL: &'static str = "Git";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut session) = world.remove_resource::<GitSession>() else {
            ui.weak("Loading repository…");
            return;
        };
        let mut state = world.entity_mut(panel).take::<Self>().unwrap();
        let mut action = None;
        let bounds = ui
            .available_rect_before_wrap()
            .shrink2(egui::vec2(12.0, 10.0));
        let height = ui.text_style_height(&egui::TextStyle::Body);
        let footer_height = (height * 8.5).min(bounds.height() * 0.52);
        let footer = egui::Rect::from_min_max(
            egui::pos2(bounds.min.x, bounds.max.y - footer_height),
            bounds.max,
        );
        let header_rect =
            egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.max.x, footer.min.y - 4.0));
        let mut header = ui.new_child(egui::UiBuilder::new().max_rect(header_rect));
        header.set_clip_rect(header_rect);
        let host = world.get_resource::<GitHost>().copied().unwrap_or_default();
        draw_header(&mut header, context, &mut session, host, &mut action);
        let list_rect = egui::Rect::from_min_max(
            egui::pos2(bounds.min.x, header.cursor().min.y + 4.0),
            header_rect.max,
        );
        state.rendered_rows = 0;
        if list_rect.height() > 0.0 {
            let mut list = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
            list.set_clip_rect(list_rect);
            if let Some(view) = session.view.clone() {
                if view.status.root.is_empty() {
                    state.tree = tree::TreeView::default();
                    list.weak("This directory is not in a Git repository.");
                } else if view.rows.is_empty() {
                    state.tree = tree::TreeView::default();
                    list.label("Working tree clean");
                    list.weak("No changes to commit.");
                } else {
                    let enabled = session.writable() && accepts_user_actions(world);
                    if view.options.view == ViewMode::Tree {
                        tree::draw(
                            &mut list,
                            context,
                            panel,
                            &view,
                            &session.collapsed,
                            &mut state,
                            enabled,
                            &mut action,
                        );
                    } else {
                        state.tree = tree::TreeView::default();
                        egui::ScrollArea::vertical()
                            .id_salt(("git-changes", panel))
                            .auto_shrink([false; 2])
                            .show_rows(&mut list, height * 1.6, view.rows.len(), |ui, range| {
                                state.rendered_rows = range.len();
                                for row in range {
                                    let key = view.rows[row].key(&view.status);
                                    ui.push_id(key, |ui| {
                                        draw_row(
                                            ui,
                                            context,
                                            &view,
                                            &view.rows[row],
                                            &mut state,
                                            session.writable() && accepts_user_actions(world),
                                            &mut action,
                                        );
                                    });
                                }
                            });
                    }
                }
            }
        }
        let mut composer = ui.new_child(egui::UiBuilder::new().max_rect(footer));
        composer.set_clip_rect(footer);
        composer.separator();
        let view = session.view.clone();
        let count = view.as_ref().map_or(0, |v| v.staged_count);
        composer.horizontal(|ui| {
            ui.label("Commit message");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!("{count} staged"));
            });
        });
        let suggestion = view.as_ref().and_then(|v| v.suggested_message.clone());
        let hint = suggestion
            .clone()
            .unwrap_or_else(|| "Describe your changes…".into());
        // An empty draft commits with the suggested message, when there is one.
        let message = if session.draft.trim().is_empty() {
            suggestion.unwrap_or_default()
        } else {
            session.draft.clone()
        };
        let button_height = height * 1.6;
        let spacing = composer.spacing().item_spacing.y;
        let progress = session.progress();
        // Leave room below the message for the commit row (and progress row).
        let mut reserved = spacing + button_height.max(composer.spacing().interact_size.y);
        if progress.is_some() {
            reserved += spacing + composer.spacing().interact_size.y;
        }
        let input_height = (composer.available_height() - reserved).max(height);
        composer.add_sized(
            [composer.available_width(), input_height],
            // `desired_rows(1)`: the default of 4 rows sets a minimum height that can
            // exceed `input_height` and push the commit row past the footer clip.
            egui::TextEdit::multiline(&mut session.draft)
                .desired_rows(1)
                .hint_text(hint)
                .id_salt(("git-commit", panel)),
        );
        let can_commit = session.writable()
            && !message.trim().is_empty()
            && view.as_ref().is_some_and(|v| {
                !v.has_conflicts
                    && match session.commit_mode {
                        CommitMode::Amend => {
                            v.status.head.is_some()
                                && v.status.operation == RepositoryOperation::None
                        }
                        CommitMode::Commit | CommitMode::Signoff => count > 0,
                    }
            });
        let accepts_actions = accepts_user_actions(world);
        composer.horizontal(|ui| {
            let menu_width = button_height;
            let width = (ui.available_width() - menu_width - ui.spacing().item_spacing.x).max(1.0);
            let label = match session.commit_mode {
                CommitMode::Commit => "Commit Staged",
                CommitMode::Amend => "Amend Commit",
                CommitMode::Signoff => "Commit with Signoff",
            };
            let response = ui.add_enabled(
                can_commit && accepts_actions,
                egui::Button::new(label)
                    .left_text(())
                    .right_text(())
                    .truncate()
                    .min_size(egui::vec2(width, button_height)),
            );
            if response.clicked() {
                action = Some(Action::Operation(Operation::Commit {
                    message: message.clone(),
                    mode: session.commit_mode,
                }));
            }
            ui.add_enabled_ui(accepts_actions && session.pending.is_none(), |ui| {
                let (response, _) = egui::containers::menu::MenuButton::from_button(
                    egui::Button::new(()).min_size(egui::vec2(menu_width, button_height)),
                )
                .ui(ui, |ui| {
                    for (mode, label, tooltip) in [
                        (
                            CommitMode::Commit,
                            "Commit",
                            "Commit staged changes as a new commit",
                        ),
                        (
                            CommitMode::Amend,
                            "Amend",
                            "Replace the last commit using this message and the current index",
                        ),
                        (
                            CommitMode::Signoff,
                            "Signoff",
                            "Add a Signed-off-by trailer using your Git identity",
                        ),
                    ] {
                        if ui
                            .selectable_value(&mut session.commit_mode, mode, label)
                            .on_hover_text(tooltip)
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                });
                bedtime_ui::paint_chevron(
                    ui.painter(),
                    response.rect.center(),
                    1.0,
                    ui.visuals().text_color(),
                );
                response.on_hover_text("Commit action");
            });
        });
        if let Some(progress) = progress {
            composer.horizontal(|ui| {
                ui.weak(progress);
                if ui.small_button("Cancel").clicked() {
                    action = Some(Action::Cancel);
                }
            });
        }
        world.entity_mut(panel).insert(state);
        if let Some(action) = action {
            let result = if accepts_user_actions(world) {
                match action {
                    Action::Operation(
                        operation @ Operation::Resolve {
                            choice: ConflictChoice::Delete,
                            ..
                        },
                    ) => {
                        session.confirmation = Some(operation);
                        Ok(())
                    }
                    Action::Operation(operation) => begin(world, &mut session, operation),
                    Action::Discard(path) => request_discard(world, &mut session, path),
                    Action::Confirm => {
                        if let Some(operation) = session.confirmation.take() {
                            begin(world, &mut session, operation)
                        } else {
                            Ok(())
                        }
                    }
                    Action::Options(options) => {
                        session.options = options;
                        reproject(world, &mut session)
                    }
                    Action::ToggleDirectory(key) => {
                        if !session.collapsed.remove(&key) {
                            session.collapsed.insert(key);
                        }
                        reproject(world, &mut session)
                    }
                    Action::Cancel => {
                        cancel(&mut session);
                        Ok(())
                    }
                    Action::Diff(path, side, pin) => diff::open(
                        world,
                        panel,
                        diff::Descriptor {
                            root: PathBuf::from(&session.view.as_ref().unwrap().status.root),
                            path,
                            side,
                        },
                        pin,
                    )
                    .map(|_| ()),
                    Action::Open(path) => {
                        let path =
                            PathBuf::from(&session.view.as_ref().unwrap().status.root).join(path);
                        world
                            .workbench_mut()
                            .open_path(&path, PathOptions::default())
                            .map(|_| ())
                    }
                    Action::Terminal => {
                        if let Some(open) = world
                            .get_resource::<GitHost>()
                            .and_then(|h| h.open_terminal)
                        {
                            let area = world.get::<InArea>(panel).unwrap().0;
                            open(world, session.root.clone(), area);
                        }
                        Ok(())
                    }
                    Action::OpenRoot => {
                        if let Some(open) =
                            world.get_resource::<GitHost>().and_then(|h| h.open_folder)
                        {
                            let root = PathBuf::from(&session.view.as_ref().unwrap().status.root);
                            open(world, root);
                        }
                        Ok(())
                    }
                }
            } else {
                Err("Cancel quit before using Git controls.".into())
            };
            if let Err(error) = result {
                session.error = Some(error);
            }
        }
        world.insert_resource(session);
    }
}

fn draw_header(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    session: &mut GitSession,
    host: GitHost,
    action: &mut Option<Action>,
) {
    let enabled = session.writable();
    let view = session.view.clone();
    let branch = view.as_ref().map_or("Git", |v| {
        if v.status.branch.is_empty() {
            "Git"
        } else {
            &v.status.branch
        }
    });
    ui.horizontal(|ui| {
        let button_width = 22.0;
        let gap = ui.spacing().item_spacing.x.min(4.0);
        ui.spacing_mut().item_spacing.x = gap;
        let branch_width = (ui.available_width() - (button_width + gap) * 5.0).max(24.0);
        ui.add_enabled_ui(enabled, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(branch_width, button_width),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let (response, _) = egui::containers::menu::MenuButton::from_button(
                        egui::Button::new(branch)
                            .right_text(())
                            .truncate()
                            .min_size(egui::vec2(branch_width, button_width)),
                    )
                    .config(
                        egui::containers::menu::MenuConfig::new()
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                    )
                    .ui(ui, |ui| {
                        ui.set_min_width(220.0);
                        ui.label("Switch branch");
                        if let Some(view) = &view {
                            egui::ScrollArea::vertical()
                                .id_salt("git-branches")
                                .max_height(160.0)
                                .show(ui, |ui| {
                                    for name in &view.status.branches {
                                        if ui
                                            .selectable_label(*name == view.status.branch, name)
                                            .clicked()
                                            && *name != view.status.branch
                                        {
                                            *action =
                                                Some(Action::Operation(Operation::SwitchBranch {
                                                    name: name.clone(),
                                                }));
                                            ui.close();
                                        }
                                    }
                                });
                        }
                        ui.separator();
                        ui.label("Create branch");
                        let response = ui.text_edit_singleline(&mut session.branch_input);
                        let _ = response;
                        if ui
                            .add_enabled(
                                !session.branch_input.trim().is_empty(),
                                egui::Button::new("Create and switch"),
                            )
                            .clicked()
                        {
                            *action = Some(Action::Operation(Operation::CreateBranch {
                                name: session.branch_input.trim().to_owned(),
                            }));
                            ui.close();
                        }
                    });
                    context.frame(ui).icon_at(
                        BuiltinIcon::ChevronDown,
                        [response.rect.right() - 16.0, response.rect.center().y - 5.0],
                        10.0,
                    );
                    response.on_hover_text(format!("{branch}\nSwitch or create a branch"));
                },
            );
        });
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(3.0, 3.0);
            let response = context
                .frame(ui)
                .icon_button_sized(BuiltinIcon::Settings, 16.0);
            context.frame(ui).tooltip(&response, "Git panel settings");
            egui::Popup::menu(&response)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let mut options = session.options;
                    ui.set_min_width(190.0);
                    ui.weak("View as");
                    ui.radio_value(&mut options.view, ViewMode::List, "List");
                    ui.radio_value(&mut options.view, ViewMode::Tree, "Tree");
                    ui.separator();
                    ui.weak("Group by");
                    ui.radio_value(&mut options.group_by, GroupBy::None, "None");
                    ui.radio_value(
                        &mut options.group_by,
                        GroupBy::TrackedUntracked,
                        "Tracked and untracked",
                    );
                    ui.radio_value(
                        &mut options.group_by,
                        GroupBy::StagedUnstaged,
                        "Staged and unstaged",
                    );
                    if options != session.options {
                        *action = Some(Action::Options(options));
                    }
                });
        });
        for (icon, tip, operation) in [
            (
                BuiltinIcon::CloudDownload,
                "Fetch changes from the remote",
                Some(Operation::Fetch),
            ),
            (
                BuiltinIcon::ArrowDownToLine,
                "Pull remote changes",
                Some(Operation::Pull),
            ),
            (
                BuiltinIcon::ArrowUpFromLine,
                "Push or publish this branch",
                Some(Operation::Push),
            ),
            (BuiltinIcon::RefreshCw, "Refresh repository status", None),
        ] {
            ui.add_enabled_ui(operation.is_none() || enabled, |ui| {
                ui.spacing_mut().button_padding = egui::vec2(3.0, 3.0);
                let response = context.frame(ui).icon_button_sized(icon, 16.0);
                context.frame(ui).tooltip(&response, tip);
                if response.clicked() {
                    if let Some(operation) = operation {
                        *action = Some(Action::Operation(operation));
                    } else {
                        session.invalidate();
                    }
                }
            });
        }
    });
    if let Some(view) = &view {
        if !view.status.writable && !view.status.root.is_empty() {
            ui.weak("Open the repository root to change Git state.");
            if host.open_folder.is_some() && ui.small_button("Open repository root").clicked() {
                *action = Some(Action::OpenRoot);
            }
        }
        if view.status.upstream.is_some() && (view.status.ahead > 0 || view.status.behind > 0) {
            ui.weak(format!(
                "{} ahead · {} behind",
                view.status.ahead, view.status.behind
            ));
        }
        if view.status.operation != RepositoryOperation::None {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{:?} in progress", view.status.operation));
                if view.status.operation != RepositoryOperation::Merge
                    && ui
                        .add_enabled(enabled, egui::Button::new("Continue"))
                        .clicked()
                {
                    *action = Some(Action::Operation(Operation::Continue));
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Abort operation…"))
                    .clicked()
                {
                    session.confirmation = Some(Operation::Abort);
                }
            });
        }
    }
    if let Some(error) = session.error.clone() {
        egui::ScrollArea::vertical()
            .id_salt("git-error")
            .max_height(55.0)
            .show(ui, |ui| {
                ui.colored_label(bedtime_ui::theme::color(context.theme.palette.error), error);
            });
        ui.horizontal(|ui| {
            if ui.small_button("Dismiss").clicked() {
                session.error = None;
            }
            if host.open_terminal.is_some() && ui.small_button("Open Git terminal").clicked() {
                *action = Some(Action::Terminal);
            }
        });
    }
    if let Some(operation) = session.confirmation.clone() {
        let message = match &operation {
            Operation::Discard { path, .. } => {
                format!("Discard working changes in {path}? This cannot be undone in the editor.")
            }
            Operation::Resolve { path, .. } => format!("Delete {path} and stage its deletion?"),
            _ => "Abort the current Git operation and restore its previous tracked state?".into(),
        };
        ui.label(message);
        ui.horizontal(|ui| {
            if ui
                .button(match &operation {
                    Operation::Discard { .. } => "Discard changes",
                    Operation::Resolve { .. } => "Delete file",
                    _ => "Abort operation",
                })
                .clicked()
            {
                *action = Some(Action::Confirm);
            }
            if ui.button("Keep changes").clicked() {
                session.confirmation = None;
            }
        });
    }
    if !session.output.is_empty() {
        ui.collapsing("Git output", |ui| {
            egui::ScrollArea::vertical()
                .id_salt("git-output")
                .max_height(60.0)
                .show(ui, |ui| {
                    ui.label(&session.output);
                });
        });
    }
    ui.separator();
}

/// A tree row's place (up to the staging checkbox) and its item's response:
/// only the folder or file's name takes the pointer.
#[allow(clippy::too_many_arguments)]
fn tree_item(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &RepositoryView,
    row: &Row,
    selected: Option<&str>,
    expanded: Option<bool>,
    checkbox_width: f32,
    height: f32,
) -> (egui::Rect, egui::Response) {
    let (rect, row_response) = ui.allocate_exact_size(
        egui::vec2(
            (ui.available_width() - checkbox_width - ui.spacing().item_spacing.x).max(1.0),
            height,
        ),
        egui::Sense::hover(),
    );
    let item = tree::entry_row(context, view, row, selected, expanded).map_or(rect, |entry| {
        bedtime_ui::tree::item_rect(ui, context, &entry, rect, bedtime_ui::tree::INDENT)
    });
    let response = ui.interact(item, row_response.id.with("item"), egui::Sense::click());
    (rect, response)
}

fn draw_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &RepositoryView,
    row: &Row,
    panel: &mut GitPanel,
    enabled: bool,
    action: &mut Option<Action>,
) {
    let height = if view.options.view == ViewMode::Tree {
        bedtime_ui::tree::row_height(context)
    } else {
        ui.text_style_height(&egui::TextStyle::Body) * 1.6
    };
    // Keep checkbox hit rectangles clear of the floating scrollbar.
    let width = (ui.available_width() - 16.0).max(1.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_max_width(width);
            let checkbox_width = ui.spacing().interact_size.y;
            match row {
                Row::Header(group, indices, staged) => {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(
                            (ui.available_width() - checkbox_width - ui.spacing().item_spacing.x)
                                .max(1.0),
                            height,
                        ),
                        egui::Sense::hover(),
                    );
                    let label = egui::WidgetText::from(
                        egui::RichText::new(format!("{}  {}", group.label(), indices.len()))
                            .strong(),
                    );
                    let galley = label.into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        rect.width(),
                        egui::TextStyle::Body,
                    );
                    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
                    ui.painter().galley(pos, galley, ui.visuals().text_color());
                    if *group != Group::Conflicts {
                        let (response, operation) =
                            changes_checkbox(ui, view, indices, *staged, enabled);
                        let _ = response;
                        if let Some(operation) = operation {
                            *action = Some(Action::Operation(operation));
                        }
                    }
                }
                Row::Directory {
                    key,
                    depth: _depth,
                    changes,
                    staged,
                    expanded,
                    conflicted,
                } => {
                    let (rect, response) = tree_item(
                        ui,
                        context,
                        view,
                        row,
                        panel.selected.as_deref(),
                        Some(*expanded),
                        checkbox_width,
                        height,
                    );
                    tree::paint_entry(
                        ui,
                        context,
                        view,
                        row,
                        rect,
                        Some(&response),
                        panel.selected.as_deref(),
                        Some(*expanded),
                        1.0,
                    );
                    if response.clicked() {
                        panel.selected = Some(key.path.clone());
                    }
                    if (response.clicked() && !response.double_clicked())
                        || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        *action = Some(Action::ToggleDirectory(key.clone()));
                    }
                    bedtime_ui::tree::tooltip(ui, context, &response, &key.path);
                    let enabled = enabled && !conflicted;
                    let (response, operation) =
                        changes_checkbox(ui, view, changes, *staged, enabled);
                    let _ = response;
                    if let Some(operation) = operation {
                        *action = Some(Action::Operation(operation));
                    }
                }
                Row::File {
                    index,
                    group,
                    depth,
                } => {
                    let entry = &view.status.entries[*index];
                    let stage = match group {
                        Some(Group::Staged) => Staging::Staged,
                        Some(Group::Unstaged) => Staging::Unstaged,
                        _ => staging(entry),
                    };
                    let response = if view.options.view == ViewMode::Tree {
                        let (rect, response) = tree_item(
                            ui,
                            context,
                            view,
                            row,
                            panel.selected.as_deref(),
                            None,
                            checkbox_width,
                            height,
                        );
                        tree::paint_entry(
                            ui,
                            context,
                            view,
                            row,
                            rect,
                            Some(&response),
                            panel.selected.as_deref(),
                            None,
                            1.0,
                        );
                        response
                    } else {
                        ui.add_space((*depth as f32 * 16.0).min((width - 90.0).max(0.0)));
                        let (code, ink) = status_ink(context, entry, *group);
                        ui.colored_label(bedtime_ui::theme::color(ink), code.to_string());
                        let label = entry.path.rsplit('/').next().unwrap();
                        let font = egui::TextStyle::Body.resolve(ui.style());
                        let mut text = egui::text::LayoutJob::default();
                        text.append(
                            label,
                            0.0,
                            egui::TextFormat {
                                font_id: font.clone(),
                                color: ui.visuals().text_color(),
                                ..Default::default()
                            },
                        );
                        if view.options.view == ViewMode::List
                            && let Some((directory, _)) = entry.path.rsplit_once('/')
                        {
                            text.append(
                                &format!("  {directory}"),
                                0.0,
                                egui::TextFormat {
                                    font_id: font,
                                    color: ui.visuals().weak_text_color(),
                                    ..Default::default()
                                },
                            );
                        }
                        ui.add_sized(
                            [
                                (ui.available_width()
                                    - checkbox_width
                                    - ui.spacing().item_spacing.x)
                                    .max(1.0),
                                height,
                            ],
                            egui::Button::selectable(
                                panel.selected.as_deref() == Some(&entry.path),
                                text,
                            )
                            .right_text(())
                            .truncate(),
                        )
                    };
                    bedtime_ui::tree::tooltip(
                        ui,
                        context,
                        &response,
                        format!("{}\n{:?}", entry.path, stage),
                    );
                    let side = match group {
                        Some(Group::Staged) => DiffSide::Staged,
                        Some(Group::Unstaged) => DiffSide::Unstaged,
                        _ => {
                            if stage == Staging::Staged {
                                DiffSide::Staged
                            } else {
                                DiffSide::Unstaged
                            }
                        }
                    };
                    if response.clicked() {
                        panel.selected = Some(entry.path.clone());
                        *action = Some(Action::Diff(entry.path.clone(), side, false));
                    }
                    if response.double_clicked()
                        || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        *action = Some(Action::Diff(entry.path.clone(), side, true));
                    }
                    response.context_menu(|ui| {
                        for (label, side) in [
                            ("Open unstaged diff", DiffSide::Unstaged),
                            ("Open staged diff", DiffSide::Staged),
                        ] {
                            if ui.button(label).clicked() {
                                *action = Some(Action::Diff(entry.path.clone(), side, true));
                                ui.close();
                            }
                        }
                        if ui.button("Open file").clicked() {
                            *action = Some(Action::Open(entry.path.clone()));
                            ui.close();
                        }
                        ui.separator();
                        if entry.conflicted {
                            for (label, choice) in [
                                ("Use current file", ConflictChoice::Current),
                                ("Use incoming file", ConflictChoice::Incoming),
                                ("Delete file…", ConflictChoice::Delete),
                            ] {
                                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                    *action = Some(Action::Operation(Operation::Resolve {
                                        path: entry.path.clone(),
                                        choice,
                                    }));
                                    ui.close();
                                }
                            }
                            if ui
                                .add_enabled(
                                    enabled,
                                    egui::Button::new("Save and stage resolution"),
                                )
                                .clicked()
                            {
                                *action = Some(Action::Operation(Operation::Stage {
                                    paths: vec![entry.path.clone()],
                                }));
                                ui.close();
                            }
                        } else {
                            if ui
                                .add_enabled(
                                    enabled && stage != Staging::Staged,
                                    egui::Button::new(if stage == Staging::Partial {
                                        "Stage remaining changes"
                                    } else {
                                        "Stage file"
                                    }),
                                )
                                .clicked()
                            {
                                *action = Some(Action::Operation(Operation::Stage {
                                    paths: vec![entry.path.clone()],
                                }));
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    enabled && stage != Staging::Unstaged,
                                    egui::Button::new("Unstage file"),
                                )
                                .clicked()
                            {
                                *action = Some(Action::Operation(Operation::Unstage {
                                    paths: vec![entry.path.clone()],
                                }));
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    enabled
                                        && !matches!(entry.worktree_status, ' ' | '.')
                                        && entry.index_status != '?',
                                    egui::Button::new("Discard file changes…"),
                                )
                                .clicked()
                            {
                                *action = Some(Action::Discard(entry.path.clone()));
                                ui.close();
                            }
                        }
                    });
                    let mut checked = stage == Staging::Staged;
                    let response = ui
                        .add_enabled(
                            enabled && !entry.conflicted,
                            egui::Checkbox::without_text(&mut checked)
                                .indeterminate(stage == Staging::Partial),
                        )
                        .on_hover_text(match stage {
                            Staging::Partial => "Partially staged. Stage remaining changes",
                            Staging::Staged => "Unstage file",
                            Staging::Unstaged => "Stage file",
                        });
                    if response.changed() {
                        let paths = vec![entry.path.clone()];
                        *action = Some(Action::Operation(if stage == Staging::Staged {
                            Operation::Unstage { paths }
                        } else {
                            Operation::Stage { paths }
                        }));
                    }
                }
            }
        },
    );
}

fn status_ink(
    context: &UiContext<'_>,
    entry: &StatusEntry,
    group: Option<Group>,
) -> (char, bedtime_ui::theme::Rgba) {
    let code = if entry.conflicted {
        '!'
    } else if group == Some(Group::Staged) {
        entry.index_status
    } else if !matches!(entry.worktree_status, ' ' | '.') {
        entry.worktree_status
    } else {
        entry.index_status
    };
    let ink = match code {
        '!' | 'U' | 'D' => context.theme.palette.error,
        'A' | '?' => context.theme.palette.success,
        _ => context.theme.palette.warning,
    };
    (code, ink)
}

fn changes_checkbox(
    ui: &mut egui::Ui,
    view: &RepositoryView,
    changes: &ChangeSet,
    stage: Staging,
    enabled: bool,
) -> (egui::Response, Option<Operation>) {
    let mut checked = stage == Staging::Staged;
    let response = ui.add_enabled(
        enabled,
        egui::Checkbox::without_text(&mut checked).indeterminate(stage == Staging::Partial),
    );
    let operation = response.changed().then(|| {
        let paths = changes
            .iter()
            .map(|index| view.status.entries[index].path.clone())
            .collect();
        if stage == Staging::Staged {
            Operation::Unstage { paths }
        } else {
            Operation::Stage { paths }
        }
    });
    (response, operation)
}
