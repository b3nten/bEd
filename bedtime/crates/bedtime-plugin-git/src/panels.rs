use crate::controller::{DiffKey, GitController, StagingState};
use bedtime_api::{features::HostAction, workbench::PanelTarget};
use bedtime_documents::DocumentId;
use bedtime_editor_widgets::DiffActionKind;
use bedtime_git::{ConflictChoice, DiffSide, Operation, RepositoryOperation, StatusEntry};
use dear_imgui_rs::{ItemFlags, MouseButton, StyleColor, StyleVar, TreeNodeFlags, Ui};
use serde_json::json;
use std::io;

#[derive(Default)]
pub(crate) struct GitPanel {
    confirm: Option<Operation>,
    groups: Option<StatusGroups>,
}

struct StatusGroups {
    status: std::sync::Arc<bedtime_git::Status>,
    conflicts: Vec<usize>,
    tracked: Vec<usize>,
    untracked: Vec<usize>,
}

impl StatusGroups {
    fn new(status: std::sync::Arc<bedtime_git::Status>) -> Self {
        let mut groups = Self {
            status,
            conflicts: Vec::new(),
            tracked: Vec::new(),
            untracked: Vec::new(),
        };
        for (index, entry) in groups.status.entries.iter().enumerate() {
            if entry.conflicted {
                groups.conflicts.push(index);
            } else if new_file(entry) {
                groups.untracked.push(index);
            } else {
                groups.tracked.push(index);
            }
        }
        groups.untracked.sort_by(|&a, &b| {
            groups.status.entries[a]
                .path
                .cmp(&groups.status.entries[b].path)
        });
        groups
    }
}

impl GitPanel {
    pub(crate) fn draw_body(
        &mut self,
        ui: &Ui,
        state: &mut GitController,
        documents: &mut bedtime_documents::DocumentSession,
        project_root: &str,
        icons: Option<&bedtime_ui::icons::Icons>,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let _controls =
            bedtime_ui::components::controls_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
        let fs = ui.text_line_height();
        // Keep cancellation reachable without moving the repository controls.
        let body_origin = ui.cursor_pos();
        let footer_height = fs * 1.8;
        let footer_y = body_origin[1] + ui.content_region_avail()[1] - footer_height;
        ui.set_cursor_pos([body_origin[0], footer_y.max(body_origin[1])]);
        let footer_right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
        let cancel_width = button_width(ui, "Cancel");
        if state.mutation_pending() {
            ui.set_cursor_pos_x(footer_right - cancel_width);
            if ui.small_button("Cancel") {
                state.cancel_operation();
            }
        }
        ui.set_cursor_pos(body_origin);
        if project_root.is_empty() {
            bedtime_ui::presentation::directory_label(ui, state.repo_root());
        }
        let status = state.status.clone();
        let branch = status.as_ref().map_or("Git", |status| {
            if status.root.is_empty() {
                "Git"
            } else {
                &status.branch
            }
        });
        {
            let style = ui.clone_style();
            let gap = style.item_spacing()[0].min(fs * 0.25);
            let _spacing = ui.push_style_var(StyleVar::ItemSpacing([gap, style.item_spacing()[1]]));
            let width = ui.content_region_avail()[0];
            let action_width = ui
                .frame_height()
                .min((width - fs * 2.0 - gap * 4.0) / 4.0)
                .max(1.0);
            let branch_width = (width - (action_width + gap) * 4.0).max(1.0);
            let label = fit_label(
                ui,
                branch,
                branch_width - style.frame_padding()[0] * 2.0 - ui.calc_text_size(" ▾")[0],
            );
            {
                let _disabled =
                    ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
                if ui.button_with_size(
                    format!("{label} ▾###git-branch-picker"),
                    [branch_width, 0.0],
                ) {
                    ui.open_popup("##git-branches");
                }
            }
            if ui.is_item_hovered() {
                bedtime_ui::components::tooltip_text(
                    ui,
                    &bedtime_ui::UiFrameContext::for_ui(ui),
                    format!("{branch}\nSwitch or create a branch"),
                );
            }
            for (key, id, fallback, tooltip, operation) in [
                (
                    "cloud-download",
                    "git-fetch",
                    "↓",
                    "Fetch changes from the remote",
                    Some(Operation::Fetch),
                ),
                (
                    "arrow-down-to-line",
                    "git-pull",
                    "↓",
                    "Pull remote changes into this branch",
                    Some(Operation::Pull),
                ),
                (
                    "arrow-up-from-line",
                    "git-push",
                    "↑",
                    if status
                        .as_ref()
                        .is_some_and(|status| status.upstream.is_some())
                    {
                        "Push this branch to its upstream"
                    } else {
                        "Publish this branch to the remote"
                    },
                    Some(Operation::Push),
                ),
                (
                    "refresh-cw",
                    "git-refresh",
                    "↻",
                    "Refresh repository status",
                    None,
                ),
            ] {
                ui.same_line();
                let _disabled = ui.begin_disabled_with_cond(
                    operation.is_some() && (!state.writable() || state.mutation_pending()),
                );
                if bedtime_ui::components::icon_button(
                    ui,
                    &bedtime_ui::UiFrameContext::for_ui(ui),
                    icons,
                    key,
                    id,
                    fallback,
                    [action_width, 0.0],
                ) {
                    if let Some(operation) = operation {
                        state.mutate(operation, documents, requests);
                    } else {
                        state.refresh = true;
                    }
                }
                if ui.is_item_hovered() {
                    bedtime_ui::components::tooltip_text(
                        ui,
                        &bedtime_ui::UiFrameContext::for_ui(ui),
                        tooltip,
                    );
                }
            }
        }
        {
            let _popup_style = bedtime_ui::components::context_menu_style(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
            );
            ui.with_bound_context(|| unsafe {
                dear_imgui_rs::sys::igSetNextWindowSize(
                    [fs * 24.0, 0.0].into(),
                    dear_imgui_rs::sys::ImGuiCond_Appearing,
                );
            });
            if let Some(_popup) = ui.begin_popup("##git-branches") {
                let _disabled =
                    ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
                ui.text_disabled("Switch branch");
                if let Some(status) = &status {
                    ui.child_window("##git-branch-list")
                        .size([
                            0.0,
                            (status.branches.len().clamp(1, 8) as f32)
                                * ui.text_line_height_with_spacing(),
                        ])
                        .build(ui, || {
                            for name in &status.branches {
                                if ui
                                    .selectable_config(name)
                                    .selected(name == &status.branch)
                                    .build()
                                    && name != &status.branch
                                {
                                    state.mutate(
                                        Operation::SwitchBranch { name: name.clone() },
                                        documents,
                                        requests,
                                    );
                                    ui.close_current_popup();
                                }
                            }
                        });
                }
                ui.separator();
                ui.text_disabled("Create branch");
                ui.set_next_item_width(-1.0);
                ui.input_text("##git-new-branch", &mut state.branch_input)
                    .build();
                let _empty = ui.begin_disabled_with_cond(state.branch_input.trim().is_empty());
                if ui.button_with_size("Create and switch", [ui.content_region_avail()[0], 0.0]) {
                    let name = state.branch_input.trim().to_owned();
                    state.mutate(Operation::CreateBranch { name }, documents, requests);
                    ui.close_current_popup();
                }
            }
        }
        show_error(ui, state, requests);
        let Some(status) = status else {
            self.groups = None;
            return Ok(());
        };
        if status.root.is_empty() {
            ui.dummy([0.0, fs * 0.5]);
            ui.text_wrapped("This directory is not in a Git repository.");
            return Ok(());
        }
        ui.dummy([0.0, fs * 0.2]);
        if !status.writable {
            ui.text_wrapped(format!(
                "Open the repository root to stage or commit all changes safely:\n{}",
                status.root
            ));
        }
        let enabled = state.writable() && !state.mutation_pending();
        match status.operation {
            RepositoryOperation::None => {}
            operation => {
                ui.separator();
                ui.text(format!("{} in progress", operation_name(operation)));
                let _disabled = ui.begin_disabled_with_cond(!enabled);
                if operation != RepositoryOperation::Merge && ui.button("Continue") {
                    state.mutate(Operation::Continue, documents, requests);
                }
                if operation != RepositoryOperation::Merge {
                    same_line_if_room(ui, button_width(ui, "Abort operation…"));
                }
                if ui.button("Abort operation…") {
                    self.confirm = Some(Operation::Abort);
                }
            }
        }
        if self
            .groups
            .as_ref()
            .is_none_or(|groups| !std::sync::Arc::ptr_eq(&groups.status, &status))
        {
            self.groups = Some(StatusGroups::new(std::sync::Arc::clone(&status)));
        }
        let groups = self.groups.as_ref().unwrap();
        if let Some(operation) = self.confirm.clone() {
            ui.separator();
            let (message, button) = confirmation_text(&operation);
            ui.text_wrapped(message);
            if ui.button(button) {
                state.mutate(operation, documents, requests);
                self.confirm = None;
            }
            same_line_if_room(ui, button_width(ui, "Keep changes"));
            if ui.button("Keep changes") {
                self.confirm = None;
            }
        }
        if !state.output.trim().is_empty()
            && ui.collapsing_header("Git output", TreeNodeFlags::empty())
        {
            ui.text_wrapped(&state.output);
        }
        ui.dummy([0.0, fs * 0.35]);
        ui.separator();
        ui.dummy([0.0, fs * 0.2]);
        // Reserve the composer and cancellation footer below the scrolling list.
        let spacing = ui.clone_style().item_spacing()[1];
        let composer_height = fs * 5.7 + ui.frame_height() + spacing * 6.0 + 1.0;
        let composer_y = footer_y - composer_height;
        let list_height = (composer_y - ui.cursor_pos()[1] - spacing).max(1.0);
        ui.child_window("##git-change-list")
            .size([0.0, list_height])
            .build(ui, || {
                if status.entries.is_empty() {
                    let _wrap = ui.push_text_wrap_pos(0.0);
                    ui.dummy([0.0, fs * 0.6]);
                    ui.text("Working tree clean");
                    ui.text_disabled("No changes to commit.");
                    return;
                }
                if !groups.conflicts.is_empty() {
                    draw_group(
                        ui,
                        "Conflicts",
                        &status.entries,
                        &groups.conflicts,
                        state,
                        documents,
                        requests,
                        &mut self.confirm,
                        icons,
                    );
                }
                draw_group(
                    ui,
                    "Tracked",
                    &status.entries,
                    &groups.tracked,
                    state,
                    documents,
                    requests,
                    &mut self.confirm,
                    icons,
                );
                draw_group(
                    ui,
                    "Untracked",
                    &status.entries,
                    &groups.untracked,
                    state,
                    documents,
                    requests,
                    &mut self.confirm,
                    icons,
                );
            });
        ui.set_cursor_pos_y(composer_y.max(ui.cursor_pos()[1]));
        let staged_entries: Vec<_> = status
            .entries
            .iter()
            .filter(|entry| state.staging_state(entry) != StagingState::Unstaged)
            .collect();
        let staged_count = staged_entries.len();
        let placeholder = if let [entry] = staged_entries.as_slice() {
            format!("update {}", entry.path.rsplit('/').next().unwrap())
        } else {
            "Describe your changes…".into()
        };
        // Keep the composer outside the scrolling list, even in busy repositories.
        ui.dummy([0.0, fs * 0.3]);
        ui.separator();
        ui.dummy([0.0, fs * 0.2]);
        ui.text("Commit message");
        let summary = format!("{staged_count} staged");
        let summary_width = ui.calc_text_size(&summary)[0];
        let right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
        if ui.item_rect_max()[0] + ui.clone_style().item_spacing()[0] + summary_width
            <= ui.cursor_screen_pos()[0] + ui.content_region_avail()[0]
        {
            ui.same_line_with_pos(right - summary_width);
            ui.text_disabled(summary);
        }
        let input_origin = ui.cursor_screen_pos();
        let empty_draft = state.draft.is_empty();
        ui.input_text_multiline(
            "##git-commit",
            &mut state.draft,
            [ui.content_region_avail()[0], fs * 4.2],
        )
        .build();
        if empty_draft && !ui.is_item_active() {
            let padding = ui.clone_style().frame_padding();
            let hint = fit_label(ui, &placeholder, ui.item_rect_size()[0] - padding[0] * 2.0);
            ui.get_window_draw_list().add_text(
                [input_origin[0] + padding[0], input_origin[1] + padding[1]],
                ui.style_color(StyleColor::TextDisabled),
                hint,
            );
        }
        {
            let _disabled = ui.begin_disabled_with_cond(!state.can_commit());
            let accent = ui.style_color(StyleColor::CheckMark);
            let background = ui.style_color(StyleColor::WindowBg);
            let _button = ui.push_style_color(
                StyleColor::Button,
                bedtime_ui::color::blend(accent, background, 0.22),
            );
            let _hover = ui.push_style_color(
                StyleColor::ButtonHovered,
                bedtime_ui::color::blend(accent, background, 0.32),
            );
            let _active = ui.push_style_color(
                StyleColor::ButtonActive,
                bedtime_ui::color::blend(accent, background, 0.42),
            );
            let _text = ui.push_style_color(
                StyleColor::Text,
                bedtime_ui::color::ensure_contrast(
                    ui.style_color(StyleColor::Text),
                    bedtime_ui::color::blend(accent, background, 0.42),
                    4.5,
                ),
            );
            if ui.button_with_size("Commit staged", [ui.content_region_avail()[0], 0.0]) {
                let message = state.draft.clone();
                state.mutate(
                    Operation::Commit { message, mode: Default::default() },
                    documents,
                    requests,
                );
            }
        }
        Ok(())
    }
}

fn staged(entry: &StatusEntry) -> bool {
    !matches!(entry.index_status, ' ' | '.' | '?')
}

fn unstaged(entry: &StatusEntry) -> bool {
    !matches!(entry.worktree_status, ' ' | '.') || entry.index_status == '?'
}

// New files stay in the same section when added to the index, until committed.
fn new_file(entry: &StatusEntry) -> bool {
    matches!(entry.index_status, '?' | 'A')
}

fn staging_checkbox(ui: &Ui, id: &str, staging: StagingState) -> Option<bool> {
    let mut checked = staging == StagingState::Staged;
    let _padding = ui.push_style_var(StyleVar::FramePadding([ui.text_line_height() * 0.15; 2]));
    // ImGui exposes the mixed checkbox appearance through an internal item flag.
    let _mixed = ui.push_item_flag(
        ItemFlags::from_bits_retain(dear_imgui_rs::sys::ImGuiItemFlags_MixedValue),
        staging == StagingState::Partial,
    );
    ui.checkbox(id, &mut checked).then_some(checked)
}

#[allow(clippy::too_many_arguments)]
fn draw_group(
    ui: &Ui,
    label: &str,
    entries: &[StatusEntry],
    indices: &[usize],
    state: &mut GitController,
    documents: &mut bedtime_documents::DocumentSession,
    requests: &mut Vec<HostAction>,
    confirm: &mut Option<Operation>,
    icons: Option<&bedtime_ui::icons::Icons>,
) {
    let _id = ui.push_id(label);
    let fs = ui.text_line_height();
    let checkbox_size = fs * 1.3;
    let right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
    ui.align_text_to_frame_padding();
    ui.text(label);
    ui.same_line();
    ui.text_disabled(indices.len().to_string());
    if label != "Conflicts" && !indices.is_empty() {
        ui.same_line_with_pos(right - checkbox_size);
        let staging = if indices
            .iter()
            .all(|entry| state.staging_state(&entries[*entry]) == StagingState::Staged)
        {
            StagingState::Staged
        } else if indices
            .iter()
            .any(|entry| state.staging_state(&entries[*entry]) != StagingState::Unstaged)
        {
            StagingState::Partial
        } else {
            StagingState::Unstaged
        };
        let _disabled = ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
        if let Some(stage) = staging_checkbox(ui, "##stage-group", staging) {
            let paths = indices
                .iter()
                .map(|&index| entries[index].path.clone())
                .collect();
            state.mutate(
                if stage {
                    Operation::Stage { paths }
                } else {
                    Operation::Unstage { paths }
                },
                documents,
                requests,
            );
        }
        if ui.is_item_hovered() {
            bedtime_ui::components::tooltip_text(
                ui,
                &bedtime_ui::UiFrameContext::for_ui(ui),
                if staging == StagingState::Staged {
                    "Unstage all files in this section"
                } else {
                    "Stage all files in this section"
                },
            );
        }
    }
    ui.dummy([0.0, fs * 0.15]);
    if indices.is_empty() {
        ui.text_disabled(format!("No {} changes.", label.to_lowercase()));
    }
    let row_height = fs * 1.6;
    let row_stride = row_height + ui.clone_style().item_spacing()[1];
    let mut clipper = dear_imgui_rs::ListClipper::new(indices.len())
        .items_height(row_stride)
        .begin(ui);
    // Keep an open context menu alive even if its row scrolls out of view.
    if let Some(index) = state.context_menu_path.as_ref().and_then(|path| {
        indices
            .iter()
            .position(|&index| &entries[index].path == path)
    }) {
        clipper.include_item_by_index(index);
    }
    while clipper.step() {
        for index in clipper.display_start()..clipper.display_end() {
            let entry = &entries[indices[index]];

            let _id = ui.push_id(&entry.path);
            let side = if staged(entry) && !unstaged(entry) {
                DiffSide::Staged
            } else {
                DiffSide::Unstaged
            };
            let staging = state.staging_state(entry);
            let code = if entry.conflicted {
                '!'
            } else if entry.index_status == '?' {
                '?'
            } else if unstaged(entry) {
                entry.worktree_status
            } else {
                entry.index_status
            };
            let (description, color) = match code {
                '!' | 'U' => ("Conflict", [0.95, 0.35, 0.30, 1.0]),
                'A' => ("Added", [0.40, 0.75, 0.50, 1.0]),
                '?' => ("Untracked", [0.40, 0.75, 0.50, 1.0]),
                'D' => ("Deleted", [0.95, 0.35, 0.30, 1.0]),
                'R' => ("Renamed", [0.90, 0.70, 0.35, 1.0]),
                'C' => ("Copied", [0.90, 0.70, 0.35, 1.0]),
                _ => ("Modified", [0.90, 0.70, 0.35, 1.0]),
            };
            let color = bedtime_ui::presentation::readable_color(ui, color);
            let row_origin = ui.cursor_pos();
            let gap = ui.clone_style().item_spacing()[0];
            let row_width = (ui.content_region_avail()[0] - checkbox_size - gap).max(1.0);
            let clicked = ui
                .selectable_config("##file")
                .size([row_width, row_height])
                .build();
            let min = ui.item_rect_min();
            let max = ui.item_rect_max();
            let hovered = ui.is_item_hovered();
            if clicked {
                if ui.is_mouse_double_clicked(MouseButton::Left) {
                    requests.push(HostAction::OpenFile {
                        path: state.absolute(&entry.path).to_string_lossy().into(),
                        viewer: None,
                    });
                } else {
                    state.open_diff(entry.path.clone(), side, requests);
                }
            }
            if hovered && ui.is_mouse_released(MouseButton::Right) {
                state.context_menu_path = Some(entry.path.clone());
            }
            {
                let _menu_style = bedtime_ui::components::context_item_menu_style(
                    ui,
                    &bedtime_ui::UiFrameContext::for_ui(ui),
                );
                if _menu_style.is_none()
                    && state.context_menu_path.as_deref() == Some(entry.path.as_str())
                {
                    state.context_menu_path = None;
                }
                if let Some(_popup) = ui.begin_popup_context_item() {
                    if ui.menu_item("Open file") {
                        requests.push(HostAction::OpenFileIn {
                            path: state.absolute(&entry.path).to_string_lossy().into(),
                            viewer: None,
                            target: PanelTarget::LastFocused,
                        });
                    }
                    if staged(entry) && ui.menu_item("View staged changes") {
                        state.open_diff(entry.path.clone(), DiffSide::Staged, requests);
                    }
                    if unstaged(entry) && ui.menu_item("View unstaged changes") {
                        state.open_diff(entry.path.clone(), DiffSide::Unstaged, requests);
                    }
                    ui.separator();
                    let _disabled =
                        ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
                    if !entry.conflicted
                        && staging != StagingState::Staged
                        && ui.menu_item(if staging == StagingState::Partial {
                            "Stage remaining changes"
                        } else {
                            "Stage file"
                        })
                    {
                        state.mutate(
                            Operation::Stage {
                                paths: vec![entry.path.clone()],
                            },
                            documents,
                            requests,
                        );
                    }
                    if !entry.conflicted
                        && staging != StagingState::Unstaged
                        && ui.menu_item("Unstage file")
                    {
                        state.mutate(
                            Operation::Unstage {
                                paths: vec![entry.path.clone()],
                            },
                            documents,
                            requests,
                        );
                    }
                    if unstaged(entry)
                        && !entry.conflicted
                        && entry.index_status != '?'
                        && ui.menu_item("Discard file changes…")
                    {
                        let key = (entry.path.clone(), DiffSide::Unstaged);
                        if let Some(diff) = state.diffs.get(&key) {
                            *confirm = Some(Operation::Discard {
                                path: entry.path.clone(),
                                snapshot: diff.snapshot.clone(),
                            });
                        } else {
                            state.open_diff(entry.path.clone(), DiffSide::Unstaged, requests);
                            state.error =
                                Some("Open the diff to review changes before discarding.".into());
                        }
                    }
                }
            }
            if hovered {
                let staging_label = match staging {
                    StagingState::Unstaged => "Not staged",
                    StagingState::Partial => "Partially staged",
                    StagingState::Staged => "Staged",
                };
                bedtime_ui::components::tooltip_text(
                    ui,
                    &bedtime_ui::UiFrameContext::for_ui(ui),
                    format!("{}\n{description} · {staging_label}", entry.path),
                );
            }

            // File name and directory share one selectable and one line.
            let filename = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            let directory = entry.path.rsplit_once('/').map(|(directory, _)| directory);
            let text_x = min[0] + fs * 1.65;
            let text_width = (max[0] - text_x - fs * 0.3).max(0.0);
            let filename = fit_label(ui, filename, text_width);
            let y = min[1] + (row_height - fs) * 0.5;
            {
                let _clip = ui.push_clip_rect(min, max, true);
                let draw = ui.get_window_draw_list();
                let origin = [min[0] + fs * 0.35, y];
                if let Some(icon) = icons.and_then(|icons| icons.get_for_file(&entry.path)) {
                    draw.add_image(
                        icon,
                        origin,
                        [origin[0] + fs, origin[1] + fs],
                        [0.0; 2],
                        [1.0; 2],
                        color,
                    );
                } else {
                    draw.add_text(origin, color, code.to_string());
                }
                draw.add_text([text_x, y], ui.style_color(StyleColor::Text), &filename);
                if let Some(directory) = directory {
                    let directory_x = text_x + ui.calc_text_size(&filename)[0] + fs * 0.6;
                    let directory_width = max[0] - directory_x - fs * 0.3;
                    if directory_width >= fs * 2.0 {
                        draw.add_text(
                            [directory_x, y],
                            ui.style_color(StyleColor::TextDisabled),
                            fit_label(ui, directory, directory_width),
                        );
                    }
                }
            }
            ui.same_line_with_pos(row_origin[0] + row_width + gap);
            ui.set_cursor_pos([
                ui.cursor_pos()[0],
                row_origin[1] + (row_height - checkbox_size) * 0.5,
            ]);
            {
                let _disabled = ui.begin_disabled_with_cond(
                    entry.conflicted || !state.writable() || state.mutation_pending(),
                );
                if let Some(stage) = staging_checkbox(ui, "##stage-file", staging) {
                    let paths = vec![entry.path.clone()];
                    state.mutate(
                        if stage {
                            Operation::Stage { paths }
                        } else {
                            Operation::Unstage { paths }
                        },
                        documents,
                        requests,
                    );
                }
                if ui.is_item_hovered() {
                    bedtime_ui::components::tooltip_text(
                        ui,
                        &bedtime_ui::UiFrameContext::for_ui(ui),
                        match staging {
                            StagingState::Partial => {
                                "Partially staged. Click to stage the remaining changes."
                            }
                            StagingState::Staged => "Remove this file from staging",
                            StagingState::Unstaged => {
                                if entry.conflicted {
                                    "Resolve this conflict in the diff before staging"
                                } else {
                                    "Include this file in the commit"
                                }
                            }
                        },
                    );
                }
            }
            ui.set_cursor_pos([
                row_origin[0],
                row_origin[1] + row_height + ui.clone_style().item_spacing()[1],
            ]);
        }
        ui.dummy([0.0, fs * 0.5]);
    }
}

/// No comparison has been selected yet. Unlike a loaded diff, this surface
/// registers no repository interest and does not queue background work.
pub(crate) fn apply_diff_action(
    state: &mut GitController,
    key: &DiffKey,
    action: bedtime_editor_widgets::DiffAction,
    document: Option<DocumentId>,
    documents: &bedtime_documents::DocumentSession,
    requests: &mut Vec<HostAction>,
) -> Option<Operation> {
    if action.kind != DiffActionKind::Discard {
        state.hunk_action(key, action, documents, requests);
        return None;
    }
    let dirty = document
        .and_then(|document| documents.snapshot(document).ok())
        .is_some_and(|document| document.dirty);
    if dirty {
        state.error = Some(
            "Save and refresh before discarding these changes, or use Undo for unsaved edits."
                .into(),
        );
    } else if state.comparisons.get(key).copied() != Some(action.comparison) {
        state.error = Some("This diff changed. Refresh before discarding these changes.".into());
    } else if let Some(diff) = state.diffs.get(key) {
        return Some(Operation::DiscardRange {
            path: key.0.clone(),
            snapshot: diff.snapshot.clone(),
            old_range: action.old_range,
            new_range: action.new_range,
        });
    } else {
        state.error = Some("The displayed changes differ from the saved file. Save and refresh before discarding them.".into());
    }
    None
}

fn operation_name(operation: RepositoryOperation) -> &'static str {
    match operation {
        RepositoryOperation::None => "No operation",
        RepositoryOperation::Merge => "Merge",
        RepositoryOperation::Rebase => "Rebase",
        RepositoryOperation::CherryPick => "Cherry-pick",
        RepositoryOperation::Revert => "Revert",
    }
}

pub(crate) fn confirmation_text(operation: &Operation) -> (&'static str, &'static str) {
    match operation {
        Operation::Abort => (
            "Abort the current Git operation and restore its previous tracked state?",
            "Abort operation",
        ),
        Operation::Resolve {
            choice: ConflictChoice::Delete,
            ..
        } => (
            "Delete this conflicted file and stage its deletion?",
            "Delete file",
        ),
        _ => (
            "Discard the selected working changes? This cannot be undone in the editor.",
            "Discard changes",
        ),
    }
}

fn button_width(ui: &Ui, label: &str) -> f32 {
    ui.calc_text_size(label)[0] + ui.clone_style().frame_padding()[0] * 2.0
}

fn same_line_if_room(ui: &Ui, width: f32) {
    let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
    if ui.item_rect_max()[0] + ui.clone_style().item_spacing()[0] + width <= right {
        ui.same_line();
    }
}

fn fit_label(ui: &Ui, label: &str, width: f32) -> String {
    if ui.calc_text_size(label)[0] <= width {
        return label.to_owned();
    }
    let available = (width - ui.calc_text_size("…")[0]).max(0.0);
    let mut end = 0;
    for (offset, character) in label.char_indices() {
        let next = offset + character.len_utf8();
        if ui.calc_text_size(&label[..next])[0] > available {
            break;
        }
        end = next;
    }
    format!("{}…", &label[..end])
}

pub(crate) fn show_error(ui: &Ui, state: &mut GitController, requests: &mut Vec<HostAction>) {
    if let Some(error) = state.error.clone() {
        let _wrap = ui.push_text_wrap_pos(0.0);
        ui.text_colored(
            bedtime_ui::presentation::readable_color(ui, [0.95, 0.35, 0.3, 1.0]),
            error,
        );
        if ui.small_button("Dismiss") {
            state.error = None;
        }
        same_line_if_room(ui, button_width(ui, "Open Git terminal"));
        if ui.small_button("Open Git terminal") {
            requests.push(HostAction::OpenPanel {
                panel_type: "bedtime.terminal.panel".into(),
                document: None,
                state: json!({"cwd":state.repo_root()}),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_documents::DocumentSession;
    use std::path::PathBuf;

    #[test]
    fn git_panel_keeps_controls_visible_while_long_change_lists_scroll() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, TextureId, WindowFlags};
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut controller = GitController::default();
        controller.root = "/preview".into();
        controller.status = Some(
            bedtime_git::Status {
                root: "/preview".into(),
                branch: "feature/long-branch-name".into(),
                writable: true,
                entries: (0..10_000)
                    .map(|index| StatusEntry {
                        path: format!("nested/long/Unicode_é_文件_{index}.rs"),
                        old_path: None,
                        index_status: 'M',
                        worktree_status: 'M',
                        conflicted: index == 0,
                    })
                    .collect(),
                ..Default::default()
            }
            .into(),
        );
        controller.draft = "Keep this draft".into();
        let mut state = controller;
        let mut panel = GitPanel::default();
        let mut session = DocumentSession::default();
        let mut icons = bedtime_ui::icons::Icons::default();
        for key in [
            "default",
            "rust",
            "git-branch",
            "plus",
            "minus",
            "cloud-download",
            "arrow-down-to-line",
            "arrow-up-from-line",
            "refresh-cw",
        ] {
            icons.set_texture(key, TextureId::new(1));
        }
        let mut requests = Vec::new();
        for width in [160.0, 240.0, 400.0, 1000.0] {
            let mut idle_position = None;
            for pushing in [false, true] {
                if pushing {
                    state.mutate(Operation::Push, &session, &mut requests);
                }
                // Settle native content and scrollbar measurements after each resize.
                for frame in 0..4 {
                    context.prepare_frame(FramePrepareOptions::new([1200.0, 700.0], 1.0 / 60.0));
                    let ui = context.frame();
                    ui.window("Git panel layout fixture")
                        .position([0.0; 2], Condition::Always)
                        .size([width, 650.0], Condition::Always)
                        .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_RESIZE)
                        .build(|| {
                            panel
                                .draw_body(
                                    ui,
                                    &mut state,
                                    &mut session,
                                    "/preview",
                                    Some(&icons),
                                    &mut requests,
                                )
                                .unwrap();
                        });
                    if frame == 1 {
                        context.binding().with_bound_context(|| unsafe {
                            let windows = &(*dear_imgui_rs::sys::igGetCurrentContext()).Windows;
                            let list =
                                std::slice::from_raw_parts(windows.Data, windows.Size as usize)
                                    .iter()
                                    .copied()
                                    .find(|window| {
                                        std::ffi::CStr::from_ptr((**window).Name)
                                            .to_string_lossy()
                                            .contains("##git-change-list")
                                    })
                                    .unwrap();
                            dear_imgui_rs::sys::igSetScrollY_WindowPtr(
                                list,
                                (*list).ScrollMax.y * 0.75,
                            );
                        });
                    }
                    let draw = context.render_legacy();
                    let vertices = draw.draw_data().total_vtx_count();
                    assert!(vertices > 0);
                    assert!(
                        vertices < 30_000,
                        "offscreen rows must not generate geometry: {vertices}"
                    );
                }
                context.binding().with_bound_context(|| unsafe {
                    let parent = dear_imgui_rs::sys::igFindWindowByName(
                        c"Git panel layout fixture".as_ptr(),
                    );
                    assert!(
                        (*parent).ScrollMax.x <= 1.0,
                        "controls overflow at {width}px"
                    );
                    assert!(
                        (*parent).ScrollMax.y <= 1.0,
                        "composer scrolls with the files at {width}px"
                    );
                    let windows = &(*dear_imgui_rs::sys::igGetCurrentContext()).Windows;
                    let list = std::slice::from_raw_parts(windows.Data, windows.Size as usize)
                        .iter()
                        .copied()
                        .find(|window| {
                            std::ffi::CStr::from_ptr((**window).Name)
                                .to_string_lossy()
                                .contains("##git-change-list")
                        })
                        .expect("the file list has its own scroll region");
                    assert!((*list).ScrollMax.y > 0.0, "long lists scroll at {width}px");
                    let composer = std::slice::from_raw_parts(windows.Data, windows.Size as usize)
                        .iter()
                        .copied()
                        .find(|window| {
                            std::ffi::CStr::from_ptr((**window).Name)
                                .to_string_lossy()
                                .contains("##git-commit")
                        })
                        .expect("the commit message input is visible");
                    assert!(
                        (*composer).Pos.y >= (*list).Pos.y + (*list).Size.y,
                        "the composer sits below the file list at {width}px"
                    );
                    assert!(
                        (*composer).Pos.y + (*composer).Size.y < (*parent).Pos.y + (*parent).Size.y,
                        "the composer stays inside the panel at {width}px"
                    );
                    let position = [(*list).Pos.x, (*list).Pos.y];
                    if let Some(idle) = idle_position {
                        assert_eq!(position, idle, "push progress does not shift the file list");
                    } else {
                        idle_position = Some(position);
                    }
                });
            }
            state.cancel_operation();
        }
        assert_eq!(state.draft, "Keep this draft");
        assert!(
            requests.is_empty(),
            "layout does not request Git operations"
        );

        // Exercise the native mixed checkbox, including what a click means.
        let draw_checkbox = |context: &mut Context, staging| {
            context.prepare_frame(FramePrepareOptions::new([1200.0, 700.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut result = None;
            let mut center = [0.0; 2];
            ui.window("Staging checkbox fixture")
                .position([450.0, 20.0], Condition::Always)
                .size([200.0, 80.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_RESIZE)
                .build(|| {
                    result = staging_checkbox(ui, "##stage", staging);
                    let min = ui.item_rect_min();
                    let max = ui.item_rect_max();
                    center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                });
            drop(context.render_legacy());
            (result, center)
        };
        for (staging, expected) in [
            (StagingState::Unstaged, true),
            (StagingState::Partial, true),
            (StagingState::Staged, false),
        ] {
            let (_, center) = draw_checkbox(&mut context, staging);
            context.io_mut().add_mouse_pos_event(center);
            draw_checkbox(&mut context, staging);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            draw_checkbox(&mut context, staging);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            assert_eq!(draw_checkbox(&mut context, staging).0, Some(expected));
        }

        // The branch button opens a picker without inserting controls into the panel.
        std::sync::Arc::make_mut(state.status.as_mut().unwrap()).branches =
            vec!["feature/long-branch-name".into(), "topic".into()];
        let mut draw_picker = |context: &mut Context, open| {
            context.prepare_frame(FramePrepareOptions::new([1200.0, 700.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Git panel layout fixture")
                .position([0.0; 2], Condition::Always)
                .size([400.0, 650.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_RESIZE)
                .build(|| {
                    if open {
                        ui.open_popup("##git-branches");
                    }
                    panel
                        .draw_body(
                            ui,
                            &mut state,
                            &mut session,
                            "/preview",
                            Some(&icons),
                            &mut requests,
                        )
                        .unwrap();
                });
            drop(context.render_legacy());
        };
        draw_picker(&mut context, true);
        draw_picker(&mut context, false);
        let target = context.binding().with_bound_context(|| unsafe {
            let windows = &(*dear_imgui_rs::sys::igGetCurrentContext()).Windows;
            let list = std::slice::from_raw_parts(windows.Data, windows.Size as usize)
                .iter()
                .copied()
                .find(|window| {
                    std::ffi::CStr::from_ptr((**window).Name)
                        .to_string_lossy()
                        .contains("##git-branch-list")
                })
                .expect("branch choices live in the popup");
            [
                (*list).Pos.x + (*list).Size.x * 0.5,
                (*list).Pos.y + (*list).Size.y * 0.75,
            ]
        });
        context.io_mut().add_mouse_pos_event(target);
        draw_picker(&mut context, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_picker(&mut context, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_picker(&mut context, false);
        assert!(
            state.mutation_pending(),
            "selecting another branch submits the switch"
        );
        state.cancel_operation();
    }

    #[test]
    fn new_files_and_partial_changes_keep_their_sections_and_staging_state() {
        for (index, worktree, is_new, staging) in [
            ('.', 'M', false, StagingState::Unstaged),
            ('M', '.', false, StagingState::Staged),
            ('M', 'M', false, StagingState::Partial),
            ('?', '?', true, StagingState::Unstaged),
            ('A', '.', true, StagingState::Staged),
            ('A', 'M', true, StagingState::Partial),
            ('D', '.', false, StagingState::Staged),
            ('.', 'D', false, StagingState::Unstaged),
        ] {
            let entry = StatusEntry {
                path: "a".into(),
                old_path: None,
                index_status: index,
                worktree_status: worktree,
                conflicted: false,
            };
            assert_eq!(new_file(&entry), is_new);
            assert_eq!(StagingState::from_entry(&entry), staging);
        }
    }
}
