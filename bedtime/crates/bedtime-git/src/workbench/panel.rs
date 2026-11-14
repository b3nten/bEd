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
        let enabled = session.accepts() && accepts_user_actions(world);
        if list_rect.height() > 0.0
            && let Some(view) = session.view.clone()
        {
            // Two lists: side by side when there's the width, else stacked.
            let lanes = view.options.staging == StagingStyle::Lanes
                && !(view.rows.is_empty() && view.staged_rows.is_empty());
            let beside = lanes && list_rect.width() >= LANES_WIDTH;
            let (main, staged) = if !lanes {
                (list_rect, None)
            } else if beside {
                let (left, right) = list_rect.split_left_right_at_fraction(0.5);
                ui.painter().vline(
                    left.right(),
                    list_rect.y_range(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                (
                    left.shrink2(egui::vec2(6.0, 0.0)),
                    Some(right.shrink2(egui::vec2(6.0, 0.0))),
                )
            } else {
                let (top, bottom) = list_rect.split_top_bottom_at_fraction(0.55);
                ui.painter().hline(
                    list_rect.x_range(),
                    top.bottom(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                (
                    top.shrink2(egui::vec2(0.0, 4.0)),
                    Some(bottom.shrink2(egui::vec2(0.0, 4.0))),
                )
            };
            let mut list = ui.new_child(egui::UiBuilder::new().max_rect(main));
            list.set_clip_rect(main);
            draw_changes(
                &mut list,
                context,
                panel,
                &view,
                &session.collapsed,
                &mut state,
                enabled,
                &mut action,
                RowLayout::changes(view.options, beside),
            );
            if let Some(staged) = staged {
                let mut lane = ui.new_child(egui::UiBuilder::new().max_rect(staged));
                lane.set_clip_rect(staged);
                draw_staged(
                    &mut lane,
                    context,
                    panel,
                    &view,
                    &mut state,
                    enabled,
                    &mut action,
                    RowLayout::staged(beside),
                );
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
        let amend = session.commit_mode == CommitMode::Amend;
        let hint = if amend {
            "Keep the last commit's message, or write a new one…".into()
        } else {
            suggestion
                .clone()
                .unwrap_or_else(|| "Describe your changes…".into())
        };
        // An empty draft commits with the suggested message, when there is
        // one; amending with it keeps the last commit's message.
        let message = if !session.draft.trim().is_empty() {
            session.draft.clone()
        } else if amend {
            String::new()
        } else {
            suggestion.unwrap_or_default()
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
        let can_commit = session.accepts()
            && (amend || !message.trim().is_empty())
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
                    Action::Operation(operation) => {
                        submit(world, &mut session, Request::Operation(operation))
                    }
                    Action::Discard(path) => submit(world, &mut session, Request::Discard(path)),
                    Action::Confirm => {
                        if let Some(operation) = session.confirmation.take() {
                            submit(world, &mut session, Request::Operation(operation))
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
                        // Somewhere other than the Git panel.
                        let options = PathOptions {
                            place: Some(bedtime_workbench::Placement::From(panel)),
                            ..default()
                        };
                        world.workbench_mut().open_path(&path, options).map(|_| ())
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
                session.fail(error);
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
    let enabled = session.accepts();
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
                    ui.separator();
                    ui.weak("Stage with");
                    for (style, label) in [
                        (StagingStyle::Chip, "Status chips"),
                        (StagingStyle::Lanes, "Commit lanes"),
                    ] {
                        ui.radio_value(&mut options.staging, style, label);
                    }
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
            .max_height(110.0)
            .show(ui, |ui| {
                ui.colored_label(bedtime_ui::theme::color(context.theme.palette.error), error);
            });
        ui.horizontal_wrapped(|ui| {
            for (label, operation) in session.remedies.clone() {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    *action = Some(Action::Operation(operation));
                }
            }
            if ui.small_button("Dismiss").clicked() {
                session.dismiss();
            }
            if host.open_terminal.is_some() && ui.small_button("Open Git terminal").clicked() {
                *action = Some(Action::Terminal);
            }
        });
    }
    if let Some(operation) = session.confirmation.clone() {
        let untracked = matches!(&operation, Operation::Discard { path, .. }
            if view.as_ref().is_some_and(|v| v.status.entries.iter().any(|e| e.path == *path && e.index_status == '?')));
        let message = match &operation {
            Operation::Discard { path, .. } if untracked => {
                format!("Delete the untracked file {path}? This cannot be undone.")
            }
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
                    Operation::Discard { .. } if untracked => "Delete file",
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

/// The panel's width from which two lanes sit side by side.
const LANES_WIDTH: f32 = 420.0;

/// The control that stages (or unstages) a row's changes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    /// The status letter in a chip: outlined, tinted (partly staged) or filled.
    Chip,
    /// A button that moves the changes to the other list.
    Move(Glyph),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Right,
    Left,
    Down,
    Up,
}

/// How a list's rows are laid out.
#[derive(Clone, Copy)]
struct RowLayout {
    /// Drawn as the Files tree's rows, else as a flat list's.
    tree: bool,
    /// A flat list with the tree's row height: the staged files.
    compact: bool,
    /// The control comes before the row, else at its right edge.
    left: bool,
    control: Control,
}

impl RowLayout {
    /// The list of changes (all of them, or what isn't staged).
    fn changes(options: GitPanelOptions, beside: bool) -> Self {
        let (left, control) = match options.staging {
            StagingStyle::Chip => (true, Control::Chip),
            // Towards the staged lane: beside it, or under it.
            StagingStyle::Lanes if beside => (false, Control::Move(Glyph::Right)),
            StagingStyle::Lanes => (true, Control::Move(Glyph::Down)),
        };
        Self {
            tree: options.view == ViewMode::Tree,
            compact: false,
            left,
            control,
        }
    }

    /// The lane of staged files: back towards the changes.
    fn staged(beside: bool) -> Self {
        Self {
            tree: false,
            compact: true,
            left: true,
            control: Control::Move(if beside { Glyph::Left } else { Glyph::Up }),
        }
    }

    fn row_height(self, ui: &egui::Ui, context: &UiContext<'_>) -> f32 {
        if self.tree || self.compact {
            bedtime_ui::tree::row_height(context)
        } else {
            ui.text_style_height(&egui::TextStyle::Body) * 1.6
        }
    }

    /// The width a control takes, with the gap beside it.
    fn control_width(ui: &egui::Ui) -> f32 {
        ui.spacing().interact_size.y + ui.spacing().item_spacing.x
    }

    /// What the row keeps free before it, for the control.
    fn leading(self, ui: &egui::Ui) -> f32 {
        if self.left {
            Self::control_width(ui)
        } else {
            0.0
        }
    }

    /// What the row keeps free at its right edge, for the control.
    fn trailing(self, ui: &egui::Ui) -> f32 {
        if self.left {
            0.0
        } else {
            Self::control_width(ui)
        }
    }
}

/// The changes: a tree or a list, or what there is to say instead.
#[allow(clippy::too_many_arguments)]
fn draw_changes(
    list: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    view: &Arc<RepositoryView>,
    collapsed: &BTreeSet<DirectoryKey>,
    state: &mut GitPanel,
    enabled: bool,
    action: &mut Option<Action>,
    layout: RowLayout,
) {
    if view.status.root.is_empty() {
        state.tree = tree::TreeView::default();
        list.weak("This directory is not in a Git repository.");
    } else if view.rows.is_empty() {
        state.tree = tree::TreeView::default();
        if view.staged_rows.is_empty() {
            list.label("Working tree clean");
            list.weak("No changes to commit.");
        } else {
            list.weak("Every change is staged.");
        }
    } else if layout.tree {
        tree::draw(
            list, context, panel, view, collapsed, state, enabled, action, layout,
        );
    } else {
        state.tree = tree::TreeView::default();
        let height = list.text_style_height(&egui::TextStyle::Body) * 1.6;
        egui::ScrollArea::vertical()
            .id_salt(("git-changes", panel))
            .auto_shrink([false; 2])
            .show_rows(list, height, view.rows.len(), |ui, range| {
                state.rendered_rows = range.len();
                for row in range {
                    let key = view.rows[row].key(&view.status);
                    ui.push_id(key, |ui| {
                        draw_row(
                            ui,
                            context,
                            view,
                            &view.rows[row],
                            state,
                            enabled,
                            action,
                            layout,
                        );
                    });
                }
            });
    }
}

/// The lane of staged files.
#[allow(clippy::too_many_arguments)]
fn draw_staged(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    view: &Arc<RepositoryView>,
    state: &mut GitPanel,
    enabled: bool,
    action: &mut Option<Action>,
    layout: RowLayout,
) {
    if view.staged_rows.is_empty() {
        ui.weak("Nothing staged");
        return;
    }
    let height = layout.row_height(ui, context);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("git-staged", panel))
        .auto_shrink([false; 2])
        .show_rows(ui, height, view.staged_rows.len(), |ui, range| {
            for row in range {
                let row = &view.staged_rows[row];
                ui.push_id(("staged", row.key(&view.status)), |ui| {
                    draw_row(ui, context, view, row, state, enabled, action, layout);
                });
            }
        });
}

/// A tree row's place (leaving `trailing` free at its right edge) and its
/// item's response: only the folder or file's name takes the pointer.
#[allow(clippy::too_many_arguments)]
fn tree_item(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &RepositoryView,
    row: &Row,
    selected: Option<&str>,
    expanded: Option<bool>,
    trailing: f32,
    height: f32,
) -> (egui::Rect, egui::Response) {
    let (rect, row_response) = ui.allocate_exact_size(
        egui::vec2((ui.available_width() - trailing).max(1.0), height),
        egui::Sense::hover(),
    );
    let item = tree::entry_row(context, view, row, selected, expanded).map_or(rect, |entry| {
        bedtime_ui::tree::item_rect(ui, context, &entry, rect, bedtime_ui::tree::INDENT)
    });
    let response = ui.interact(item, row_response.id.with("item"), egui::Sense::click());
    (rect, response)
}

fn paint_glyph(painter: &egui::Painter, center: egui::Pos2, glyph: Glyph, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.5, color);
    let line = |from: egui::Vec2, to: egui::Vec2| {
        painter.line_segment([center + from, center + to], stroke);
    };
    let reach = 4.0;
    let along = match glyph {
        Glyph::Right => egui::vec2(1.0, 0.0),
        Glyph::Left => egui::vec2(-1.0, 0.0),
        Glyph::Down => egui::vec2(0.0, 1.0),
        Glyph::Up => egui::vec2(0.0, -1.0),
    };
    let across = egui::vec2(-along.y, along.x);
    let tip = along * reach;
    line(-tip, tip);
    line(tip, tip - along * 3.5 + across * 3.5);
    line(tip, tip - along * 3.5 - across * 3.5);
}

/// The staging control of a row; whether it was used. `code` is a file's
/// status letter and its colour (a folder or a group has none).
fn stage_control(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    control: Control,
    stage: Staging,
    enabled: bool,
    code: Option<(char, bedtime_ui::theme::Rgba)>,
    hint: &str,
) -> bool {
    let size = ui.spacing().interact_size.y;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), sense);
    // A checkbox to assistive technology (and scripts), whatever it looks like.
    let checked = stage == Staging::Staged;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, hint)
    });
    let fade = if enabled { 1.0 } else { 0.45 };
    let hovered = enabled && response.hovered();
    let text = bedtime_ui::theme::color(context.theme.palette.text);
    let painter = ui.painter();
    match control {
        Control::Chip => {
            let ink = code
                .map_or(text, |(_, ink)| bedtime_ui::theme::color(ink))
                .gamma_multiply(fade);
            let chip = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(size - 5.0));
            let fill = match stage {
                Staging::Staged => ink,
                Staging::Partial => ink.gamma_multiply(0.35),
                Staging::Unstaged if hovered => ink.gamma_multiply(0.18),
                Staging::Unstaged => egui::Color32::TRANSPARENT,
            };
            painter.rect_filled(chip, 4.0, fill);
            painter.rect_stroke(
                chip,
                4.0,
                egui::Stroke::new(if hovered { 1.5 } else { 1.0 }, ink),
                egui::StrokeKind::Inside,
            );
            // On a filled chip, the mark is cut out of it.
            let mark = if stage == Staging::Staged {
                ui.visuals().panel_fill
            } else {
                ink
            };
            if let Some((code, _)) = code {
                let mut font = egui::TextStyle::Body.resolve(ui.style());
                font.size = (font.size * 0.82).round();
                painter.text(chip.center(), egui::Align2::CENTER_CENTER, code, font, mark);
            } else {
                // A folder or a group: ticked, or a dash when partly staged.
                let stroke = egui::Stroke::new(1.5, mark);
                let at = |x: f32, y: f32| chip.center() + egui::vec2(x, y);
                match stage {
                    Staging::Staged => {
                        painter.line_segment([at(-3.5, 0.0), at(-1.0, 2.5)], stroke);
                        painter.line_segment([at(-1.0, 2.5), at(3.5, -2.5)], stroke);
                    }
                    Staging::Partial => {
                        painter.line_segment([at(-3.0, 0.0), at(3.0, 0.0)], stroke);
                    }
                    Staging::Unstaged => {}
                }
            }
        }
        Control::Move(glyph) => {
            if hovered {
                painter.rect_filled(rect.shrink(1.0), 4.0, ui.visuals().widgets.hovered.bg_fill);
            }
            let ink = if hovered {
                text
            } else {
                ui.visuals().weak_text_color()
            };
            paint_glyph(painter, rect.center(), glyph, ink.gamma_multiply(fade));
        }
    }
    response.on_hover_text(hint).clicked()
}

/// Staging or unstaging a set of changes, by how staged it is.
fn changes_operation(view: &RepositoryView, changes: &ChangeSet, stage: Staging) -> Operation {
    let paths = changes
        .iter()
        .map(|index| view.status.entries[index].path.clone())
        .collect();
    if stage == Staging::Staged {
        Operation::Unstage { paths }
    } else {
        Operation::Stage { paths }
    }
}

fn set_hint(stage: Staging) -> &'static str {
    match stage {
        Staging::Partial => "Partially staged. Stage the remaining changes",
        Staging::Staged => "Unstage these changes",
        Staging::Unstaged => "Stage these changes",
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &RepositoryView,
    row: &Row,
    panel: &mut GitPanel,
    enabled: bool,
    action: &mut Option<Action>,
    layout: RowLayout,
) {
    let height = layout.row_height(ui, context);
    // Keep the controls' hit rectangles clear of the floating scrollbar.
    let width = (ui.available_width() - 16.0).max(1.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_max_width(width);
            let trailing = layout.trailing(ui);
            let text = context.theme.palette.text;
            // A staged row of chips is tinted, to read down the list.
            let tint = |ui: &egui::Ui, stage: Staging, ink: bedtime_ui::theme::Rgba| {
                if layout.control == Control::Chip && stage != Staging::Unstaged {
                    ui.painter().rect_filled(
                        ui.max_rect(),
                        4.0,
                        bedtime_ui::theme::color(ink).gamma_multiply(0.09),
                    );
                }
            };
            match row {
                Row::Header(group, indices, staged) => {
                    let controlled = *group != Group::Conflicts;
                    let mut used = false;
                    if layout.left {
                        if controlled {
                            used = stage_control(
                                ui,
                                context,
                                layout.control,
                                *staged,
                                enabled,
                                None,
                                set_hint(*staged),
                            );
                        } else {
                            ui.add_space(layout.leading(ui));
                        }
                    }
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2((ui.available_width() - trailing).max(1.0), height),
                        egui::Sense::hover(),
                    );
                    let name = match (view.options.staging, group) {
                        (StagingStyle::Lanes, Group::Unstaged) => "Changes",
                        _ => group.label(),
                    };
                    let label = egui::WidgetText::from(
                        egui::RichText::new(format!("{name}  {}", indices.len())).strong(),
                    );
                    let galley = label.into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        rect.width(),
                        egui::TextStyle::Body,
                    );
                    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
                    ui.painter().galley(pos, galley, ui.visuals().text_color());
                    if !layout.left && controlled {
                        used = stage_control(
                            ui,
                            context,
                            layout.control,
                            *staged,
                            enabled,
                            None,
                            set_hint(*staged),
                        );
                    }
                    if used {
                        *action =
                            Some(Action::Operation(changes_operation(view, indices, *staged)));
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
                    let enabled = enabled && !conflicted;
                    tint(ui, *staged, text);
                    let mut used = layout.left
                        && stage_control(
                            ui,
                            context,
                            layout.control,
                            *staged,
                            enabled,
                            None,
                            set_hint(*staged),
                        );
                    let (rect, response) = tree_item(
                        ui,
                        context,
                        view,
                        row,
                        panel.selected.as_deref(),
                        Some(*expanded),
                        trailing,
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
                    used |= !layout.left
                        && stage_control(
                            ui,
                            context,
                            layout.control,
                            *staged,
                            enabled,
                            None,
                            set_hint(*staged),
                        );
                    if used {
                        *action =
                            Some(Action::Operation(changes_operation(view, changes, *staged)));
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
                    let (code, ink) = status_ink(context, entry, *group);
                    let hint = match stage {
                        Staging::Partial => "Partially staged. Stage remaining changes",
                        Staging::Staged => "Unstage file",
                        Staging::Unstaged => "Stage file",
                    };
                    let controllable = enabled && !entry.conflicted;
                    tint(ui, stage, ink);
                    let mut used = layout.left
                        && stage_control(
                            ui,
                            context,
                            layout.control,
                            stage,
                            controllable,
                            Some((code, ink)),
                            hint,
                        );
                    let response = if layout.tree {
                        let (rect, response) = tree_item(
                            ui,
                            context,
                            view,
                            row,
                            panel.selected.as_deref(),
                            None,
                            trailing,
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
                        // A chip already shows the status letter.
                        if layout.control != Control::Chip {
                            ui.colored_label(bedtime_ui::theme::color(ink), code.to_string());
                        }
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
                        if let Some((directory, _)) = entry.path.rsplit_once('/') {
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
                            [(ui.available_width() - trailing).max(1.0), height],
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
                            // An untracked folder is another repository.
                            let untracked = entry.index_status == '?';
                            if ui
                                .add_enabled(
                                    enabled
                                        && !matches!(entry.worktree_status, ' ' | '.')
                                        && !entry.path.ends_with('/'),
                                    egui::Button::new(if untracked {
                                        "Delete untracked file…"
                                    } else {
                                        "Discard file changes…"
                                    }),
                                )
                                .clicked()
                            {
                                *action = Some(Action::Discard(entry.path.clone()));
                                ui.close();
                            }
                        }
                    });
                    used |= !layout.left
                        && stage_control(
                            ui,
                            context,
                            layout.control,
                            stage,
                            controllable,
                            Some((code, ink)),
                            hint,
                        );
                    if used {
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
