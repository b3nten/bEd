use crate::{
    conflicts::has_conflict_markers,
    controller::{DiffKey, GitController, StagingState},
};
use bed_document_session::{DocumentId, ViewId};
use bed_editing::editor_commands::CursorReveal;
use bed_editor_ui::{DiffActionKind, DiffHunkRange, DiffPresentation, DiffSnapshot};
use bed_git::{ConflictChoice, Diff, DiffSide, Operation, RepositoryOperation, StatusEntry};
use bed_module_editor::{EditorConfig, EditorRuntime, TextPanel};
use bed_workbench_api::{
    HostContext, HostRequest, ModulePanel, ModuleServices, PanelAction, PanelTarget,
};
use dear_imgui_rs::{
    ItemFlags, MouseButton, StyleColor, StyleVar, TableColumnFlags, TableColumnWidth, TableFlags,
    TreeNodeFlags, Ui,
};
use serde_json::{Value, json};
use std::{
    any::Any,
    cell::RefCell,
    io,
    path::{Component, Path},
    rc::Rc,
    sync::Arc,
};

pub(crate) struct GitPanel {
    state: Rc<RefCell<GitController>>,
    confirm: Option<Operation>,
}
impl GitPanel {
    pub fn new(state: Rc<RefCell<GitController>>) -> Self {
        state.borrow_mut().visible += 1;
        Self {
            state,
            confirm: None,
        }
    }
}
impl Drop for GitPanel {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.visible = state.visible.saturating_sub(1);
    }
}
impl ModulePanel for GitPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Git".into()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Git requires workbench services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let icons = services.resources.take::<bed_ui::icons::Icons>();
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let fs = ui.text_line_height();
        // Reserve this line even while idle. Background reads and local staging
        // stay quiet; remote operation progress never moves the controls.
        let body_origin = ui.cursor_pos();
        let footer_height = fs * 1.8;
        let footer_y = body_origin[1] + ui.content_region_avail()[1] - footer_height;
        ui.set_cursor_pos([body_origin[0], footer_y.max(body_origin[1])]);
        let footer_right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
        let cancel_width = button_width(ui, "Cancel");
        let progress = state.progress_label().unwrap_or("");
        ui.text_disabled(fit_label(
            ui,
            progress,
            (ui.content_region_avail()[0] - cancel_width - fs * 0.6).max(0.0),
        ));
        if !progress.is_empty() && ui.is_item_hovered() {
            ui.tooltip_text(progress);
        }
        if state.mutation_pending() {
            ui.same_line_with_pos(footer_right - cancel_width);
            if ui.small_button("Cancel") {
                state.cancel_operation();
            }
        }
        ui.set_cursor_pos(body_origin);
        // A wide tile gives the change list its own column; a sidebar stacks it.
        let _layout = if ui.content_region_avail()[0] >= fs * 44.0 {
            ui.begin_table_with_sizing(
                "##git-layout",
                2,
                TableFlags::NO_SAVED_SETTINGS | TableFlags::BORDERS_INNER_V,
                [0.0; 2],
                0.0,
            )
        } else {
            None
        };
        if _layout.is_some() {
            ui.table_setup_column(
                "Repository",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(fs * 23.0)),
            );
            ui.table_setup_column(
                "Changes",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Stretch(1.0)),
            );
            ui.table_next_column();
        }
        if services.project_root.is_empty() {
            bed_ui::presentation::directory_label(ui, state.repo_root());
        }
        let status = state.status.clone();
        let branch = status.as_ref().map_or("Git", |status| {
            if status.root.is_empty() {
                "Git"
            } else {
                &status.branch
            }
        });
        let right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
        let refresh_width = button_width(ui, "Refresh");
        if let Some(icon) = icons.as_deref().and_then(|icons| icons.get("git-branch")) {
            ui.image_config(icon, [fs; 2])
                .tint_color(ui.style_color(StyleColor::Text))
                .build();
            ui.same_line();
        }
        let branch_width = (ui.content_region_avail()[0] - refresh_width - fs * 0.6).max(1.0);
        let label = fit_label(
            ui,
            &format!("{branch} ▾"),
            branch_width - ui.clone_style().frame_padding()[0] * 2.0,
        );
        {
            let _disabled =
                ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
            if ui.button_with_size(format!("{label}###git-branch-picker"), [branch_width, 0.0]) {
                ui.open_popup("##git-branches");
            }
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(format!("{branch}\nSwitch or create a branch"));
        }
        ui.same_line_with_pos(right - refresh_width);
        if ui.small_button("Refresh") {
            state.refresh = true;
        }
        {
            let _popup_style = bed_ui::util::popup_style::context_menu_style(ui);
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
                                        services.documents,
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
                    state.mutate(
                        Operation::CreateBranch { name },
                        services.documents,
                        requests,
                    );
                    ui.close_current_popup();
                }
            }
        }
        show_error(ui, &mut state, services, requests);
        let Some(status) = status else {
            return Ok(());
        };
        if status.root.is_empty() {
            ui.dummy([0.0, fs * 0.5]);
            ui.text_wrapped("This directory is not in a Git repository.");
            return Ok(());
        }
        let tracking = format!(
            "{}  ·  ↑{} ↓{}",
            status.upstream.as_deref().unwrap_or("No upstream"),
            status.ahead,
            status.behind
        );
        ui.text_disabled(fit_label(ui, &tracking, ui.content_region_avail()[0]));
        if ui.is_item_hovered() {
            ui.tooltip_text(format!(
                "{tracking}\n{} commits ahead, {} behind",
                status.ahead, status.behind
            ));
        }
        ui.dummy([0.0, fs * 0.2]);
        if !status.writable {
            ui.text_wrapped(format!(
                "Open the repository root to stage or commit all changes safely:\n{}",
                status.root
            ));
        }
        let enabled = state.writable() && !state.mutation_pending();
        {
            let _disabled = ui.begin_disabled_with_cond(!enabled);
            let sync_width = ui.content_region_avail()[0];
            let gap = ui.clone_style().item_spacing()[0];
            let columns = (sync_width - gap * 2.0) / 3.0;
            let inline = columns >= button_width(ui, "Publish");
            for (index, (label, operation)) in [
                ("Fetch", Operation::Fetch),
                ("Pull", Operation::Pull),
                (
                    if status.upstream.is_some() {
                        "Push"
                    } else {
                        "Publish"
                    },
                    Operation::Push,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                if index > 0 && inline {
                    ui.same_line();
                }
                if ui.button_with_size(label, [if inline { columns } else { sync_width }, 0.0]) {
                    state.mutate(operation, services.documents, requests);
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(match label {
                        "Fetch" => "Fetch changes from the remote",
                        "Pull" => "Pull remote changes into this branch",
                        "Push" => "Push this branch to its upstream",
                        _ => "Publish this branch to the remote",
                    });
                }
            }
        }
        ui.dummy([0.0, fs * 0.2]);
        match status.operation {
            RepositoryOperation::None => {}
            operation => {
                ui.separator();
                ui.text(format!("{} in progress", operation_name(operation)));
                let _disabled = ui.begin_disabled_with_cond(!enabled);
                if operation != RepositoryOperation::Merge && ui.button("Continue") {
                    state.mutate(Operation::Continue, services.documents, requests);
                }
                if operation != RepositoryOperation::Merge {
                    same_line_if_room(ui, button_width(ui, "Abort operation…"));
                }
                if ui.button("Abort operation…") {
                    self.confirm = Some(Operation::Abort);
                }
            }
        }
        let conflicts: Vec<_> = status
            .entries
            .iter()
            .filter(|e| e.conflicted)
            .cloned()
            .collect();
        let tracked: Vec<_> = status
            .entries
            .iter()
            .filter(|entry| !entry.conflicted && !new_file(entry))
            .cloned()
            .collect();
        let mut untracked: Vec<_> = status
            .entries
            .iter()
            .filter(|entry| !entry.conflicted && new_file(entry))
            .cloned()
            .collect();
        // Git lists indexed additions first; keep rows in path order when staging.
        untracked.sort_by(|a, b| a.path.cmp(&b.path));
        let staged_count = status
            .entries
            .iter()
            .filter(|entry| state.staging_state(entry) != StagingState::Unstaged)
            .count();
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
            let hint = fit_label(
                ui,
                "Describe your changes…",
                ui.item_rect_size()[0] - padding[0] * 2.0,
            );
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
                bed_editing::util::color::blend(accent, background, 0.22),
            );
            let _hover = ui.push_style_color(
                StyleColor::ButtonHovered,
                bed_editing::util::color::blend(accent, background, 0.32),
            );
            let _active = ui.push_style_color(
                StyleColor::ButtonActive,
                bed_editing::util::color::blend(accent, background, 0.42),
            );
            let _text = ui.push_style_color(
                StyleColor::Text,
                bed_editing::util::color::ensure_contrast(
                    ui.style_color(StyleColor::Text),
                    bed_editing::util::color::blend(accent, background, 0.42),
                    4.5,
                ),
            );
            if ui.button_with_size("Commit staged", [ui.content_region_avail()[0], 0.0]) {
                let message = state.draft.clone();
                state.mutate(Operation::Commit { message }, services.documents, requests);
            }
        }
        if let Some(operation) = self.confirm.clone() {
            ui.separator();
            let (message, button) = confirmation_text(&operation);
            ui.text_wrapped(message);
            if ui.button(button) {
                state.mutate(operation, services.documents, requests);
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
        if _layout.is_some() {
            ui.table_next_column();
        } else {
            ui.dummy([0.0, fs * 0.35]);
            ui.separator();
            ui.dummy([0.0, fs * 0.2]);
        }
        let list_height = (ui.content_region_avail()[1] - footer_height).max(fs * 6.0);
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
                if !conflicts.is_empty() {
                    draw_group(
                        ui,
                        "Conflicts",
                        &conflicts,
                        &mut state,
                        services,
                        requests,
                        &mut self.confirm,
                        icons.as_deref(),
                    );
                }
                draw_group(
                    ui,
                    "Tracked",
                    &tracked,
                    &mut state,
                    services,
                    requests,
                    &mut self.confirm,
                    icons.as_deref(),
                );
                draw_group(
                    ui,
                    "Untracked",
                    &untracked,
                    &mut state,
                    services,
                    requests,
                    &mut self.confirm,
                    icons.as_deref(),
                );
            });
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
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
        ItemFlags::from_bits_retain(dear_imgui_rs::sys::ImGuiItemFlags_MixedValue as i32),
        staging == StagingState::Partial,
    );
    ui.checkbox(id, &mut checked).then_some(checked)
}

fn draw_group(
    ui: &Ui,
    label: &str,
    entries: &[StatusEntry],
    state: &mut GitController,
    services: &mut ModuleServices<'_>,
    requests: &mut Vec<HostRequest>,
    confirm: &mut Option<Operation>,
    icons: Option<&bed_ui::icons::Icons>,
) {
    let _id = ui.push_id(label);
    let fs = ui.text_line_height();
    let checkbox_size = fs * 1.3;
    let right = ui.cursor_pos()[0] + ui.content_region_avail()[0];
    ui.align_text_to_frame_padding();
    ui.text(label);
    ui.same_line();
    ui.text_disabled(entries.len().to_string());
    if label != "Conflicts" && !entries.is_empty() {
        ui.same_line_with_pos(right - checkbox_size);
        let staging = if entries
            .iter()
            .all(|entry| state.staging_state(entry) == StagingState::Staged)
        {
            StagingState::Staged
        } else if entries
            .iter()
            .any(|entry| state.staging_state(entry) != StagingState::Unstaged)
        {
            StagingState::Partial
        } else {
            StagingState::Unstaged
        };
        let _disabled = ui.begin_disabled_with_cond(!state.writable() || state.mutation_pending());
        if let Some(stage) = staging_checkbox(ui, "##stage-group", staging) {
            let paths = entries.iter().map(|entry| entry.path.clone()).collect();
            state.mutate(
                if stage {
                    Operation::Stage { paths }
                } else {
                    Operation::Unstage { paths }
                },
                services.documents,
                requests,
            );
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(if staging == StagingState::Staged {
                "Unstage all files in this section"
            } else {
                "Stage all files in this section"
            });
        }
    }
    ui.dummy([0.0, fs * 0.15]);
    if entries.is_empty() {
        ui.text_disabled(format!("No {} changes.", label.to_lowercase()));
    }
    for entry in entries {
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
        let color = bed_ui::presentation::readable_color(ui, color);
        let row_origin = ui.cursor_pos();
        let gap = ui.clone_style().item_spacing()[0];
        let row_width = (ui.content_region_avail()[0] - checkbox_size - gap).max(1.0);
        let row_height = fs * 1.6;
        let clicked = ui
            .selectable_config("##file")
            .size([row_width, row_height])
            .build();
        let min = ui.item_rect_min();
        let max = ui.item_rect_max();
        let hovered = ui.is_item_hovered();
        if clicked {
            if ui.is_mouse_double_clicked(MouseButton::Left) {
                requests.push(HostRequest::OpenFile {
                    path: state.absolute(&entry.path).to_string_lossy().into(),
                    viewer: None,
                });
            } else {
                state.open_diff(entry.path.clone(), side, requests);
            }
        }
        {
            let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
            if let Some(_popup) = ui.begin_popup_context_item() {
                if ui.menu_item("Open file") {
                    requests.push(HostRequest::OpenFileIn {
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
                        services.documents,
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
                        services.documents,
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
            ui.tooltip_text(format!("{}\n{description} · {staging_label}", entry.path));
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
                    services.documents,
                    requests,
                );
            }
            if ui.is_item_hovered() {
                ui.tooltip_text(match staging {
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
                });
            }
        }
        ui.set_cursor_pos([
            row_origin[0],
            row_origin[1] + row_height + ui.clone_style().item_spacing()[1],
        ]);
    }
    ui.dummy([0.0, fs * 0.5]);
}

/// No comparison has been selected yet. Unlike a loaded diff, this surface
/// registers no repository interest and does not queue background work.
pub(crate) struct EmptyDiffPanel;
impl ModulePanel for EmptyDiffPanel {
    fn is_input_empty(&self, _: &HostContext<'_>) -> bool {
        true
    }
    fn title(&self, _: &HostContext<'_>) -> String {
        "Git diff".into()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        ui.text_wrapped("Select a changed file in Git Changes to compare it.");
        if ui.button("Choose comparison…") {
            requests.push(HostRequest::ShowPanel {
                panel_type: crate::PANEL_ID.into(),
                document: None,
                state: Value::Null,
                action: None,
            });
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct DiffPanel {
    state: Rc<RefCell<GitController>>,
    config: Rc<RefCell<EditorConfig>>,
    runtime: Rc<RefCell<EditorRuntime>>,
    key: DiffKey,
    editor: Option<TextPanel>,
    editor_state: Value,
    snapshot: Option<String>,
    owns_snapshot: bool,
    baseline: Arc<[u8]>,
    baseline_comparison: Option<u64>,
    saved: Option<DiffSnapshot>,
    selected_hunk: usize,
    confirm: Option<Operation>,
}
impl DiffPanel {
    pub fn new(
        state: Rc<RefCell<GitController>>,
        config: Rc<RefCell<EditorConfig>>,
        runtime: Rc<RefCell<EditorRuntime>>,
        _document: Option<DocumentId>,
        restored: &Value,
        _: &mut ModuleServices<'_>,
    ) -> io::Result<Self> {
        let path = restored["path"]
            .as_str()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| io::Error::other("Git diff requires a repository-relative path"))?
            .to_owned();
        if Path::new(&path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(io::Error::other(
                "Git diff path must stay within the repository",
            ));
        }
        let side = if restored["side"].as_str() == Some("staged") {
            DiffSide::Staged
        } else {
            DiffSide::Unstaged
        };
        let key = (path, side);
        state.borrow_mut().visible += 1;
        *state
            .borrow_mut()
            .diff_users
            .entry(key.clone())
            .or_default() += 1;
        state.borrow_mut().request_diff(key.clone());
        Ok(Self {
            state,
            config,
            runtime,
            key,
            editor: None,
            editor_state: restored["editor"].clone(),
            snapshot: None,
            owns_snapshot: false,
            baseline: Arc::from([]),
            baseline_comparison: None,
            saved: None,
            selected_hunk: 0,
            confirm: None,
        })
    }
    fn sync_editor(&mut self, diff: &Diff, services: &mut ModuleServices<'_>) -> io::Result<()> {
        if diff.binary {
            return Ok(());
        }
        // A deleted file is represented by a snapshot. Once it reappears (for
        // example after a branch switch), reconnect this surface to the actual
        // shared document instead of leaving it permanently read-only.
        if self.editor.is_some()
            && self.owns_snapshot
            && self.key.1 == DiffSide::Unstaged
            && self.snapshot.as_ref() != Some(&diff.snapshot)
        {
            let path = self.state.borrow().absolute(&self.key.0);
            match services.documents.open_file(&path) {
                Ok(document) => {
                    let mut previous = self.editor.take().unwrap();
                    self.editor_state = previous.save_state_with_services(services)?;
                    let previous_document = previous.attached_document().unwrap();
                    previous.close_with_services(services, &mut Vec::new())?;
                    let _ = services.documents.close_document(
                        previous_document,
                        bed_document_session::editor_session::ClosePolicy::Discard,
                    );
                    self.editor = Some(TextPanel::with_runtime(
                        services.documents,
                        document,
                        &self.editor_state,
                        self.config.clone(),
                        self.runtime.clone(),
                    )?);
                    self.owns_snapshot = false;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if self.editor.is_some() {
            if self.owns_snapshot && self.snapshot.as_ref() != Some(&diff.snapshot) {
                services.documents.replace_snapshot_document(
                    self.editor.as_ref().unwrap().attached_document().unwrap(),
                    &diff.new_bytes,
                )?;
            }
            self.snapshot = Some(diff.snapshot.clone());
            return Ok(());
        }
        let path = self.state.borrow().absolute(&self.key.0);
        let document = if self.key.1 == DiffSide::Staged {
            self.owns_snapshot = true;
            let language = services
                .documents
                .document_for_path(&path)
                .and_then(|id| services.documents.snapshot(id).ok())
                .map(|s| s.language_id)
                .unwrap_or_else(|| {
                    bed_editing::editor_state::EditorState::language_id_from_path(&self.key.0)
                });
            services
                .documents
                .create_snapshot_document(&diff.new_bytes, &language)?
        } else {
            match services.documents.open_file(&path) {
                Ok(document) => document,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.owns_snapshot = true;
                    services
                        .documents
                        .create_snapshot_document(&diff.new_bytes, "")?
                }
                Err(error) => return Err(error),
            }
        };
        self.editor = Some(TextPanel::with_runtime(
            services.documents,
            document,
            &self.editor_state,
            self.config.clone(),
            self.runtime.clone(),
        )?);
        self.snapshot = Some(diff.snapshot.clone());
        Ok(())
    }
    fn navigate_hunk(
        &mut self,
        services: &mut ModuleServices<'_>,
        diff: &Diff,
        next: bool,
    ) -> io::Result<()> {
        if diff.hunks.is_empty() {
            return Ok(());
        }
        if let Some(editor) = &self.editor {
            let caret_row = services
                .documents
                .view_snapshot(editor.view().id())?
                .primary()
                .head_row
                .max(0) as usize;
            self.selected_hunk = if next {
                diff.hunks
                    .iter()
                    .position(|h| h.new_start > caret_row)
                    .unwrap_or(0)
            } else {
                diff.hunks
                    .iter()
                    .rposition(|h| h.new_start < caret_row)
                    .unwrap_or(diff.hunks.len() - 1)
            };
            let row = diff.hunks[self.selected_hunk].new_start as i32;
            services
                .documents
                .with_commands(editor.view().id(), |commands| {
                    commands.set_cursor(row, 0, false, CursorReveal::Center)
                })?;
        }
        Ok(())
    }
}
impl Drop for DiffPanel {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.visible = state.visible.saturating_sub(1);
        if let Some(users) = state.diff_users.get_mut(&self.key) {
            *users = users.saturating_sub(1);
        }
        if state.diff_users.get(&self.key).copied() == Some(0) {
            state.diff_users.remove(&self.key);
            state.diffs.remove(&self.key);
            state.comparisons.remove(&self.key);
        }
    }
}
impl ModulePanel for DiffPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        format!(
            "{} · {}",
            self.key.0,
            if self.key.1 == DiffSide::Staged {
                "Staged"
            } else {
                "Unstaged"
            }
        )
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Git diff requires workbench services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let diff = self.state.borrow().diffs.get(&self.key).cloned();
        let Some(diff) = diff else {
            ui.dummy([0.0, ui.text_line_height()]);
            let mut state = self.state.borrow_mut();
            state.request_diff(self.key.clone());
            show_error(ui, &mut state, services, requests);
            return Ok(());
        };
        {
            let mut state = self.state.borrow_mut();
            ui.text(if self.key.1 == DiffSide::Staged {
                "HEAD → Index"
            } else {
                "Index → Working file"
            });
            ui.same_line();
            if ui.small_button("Open file") {
                requests.push(HostRequest::OpenFileIn {
                    path: state.absolute(&self.key.0).to_string_lossy().into(),
                    viewer: None,
                    target: PanelTarget::LastFocused,
                });
            }
            ui.same_line();
            if ui.small_button("Refresh") {
                state.request_diff(self.key.clone());
            }
            let conflicted = state.status.as_ref().is_some_and(|status| {
                status
                    .entries
                    .iter()
                    .any(|entry| entry.path == self.key.0 && entry.conflicted)
            });
            let enabled = state.writable() && !state.mutation_pending();
            {
                let _disabled = ui.begin_disabled_with_cond(!enabled);
                if ui.button(if self.key.1 == DiffSide::Staged {
                    "Unstage file"
                } else if conflicted {
                    "Save and stage resolution"
                } else {
                    "Stage file"
                }) {
                    let markers = self
                        .editor
                        .as_ref()
                        .and_then(|editor| {
                            services
                                .documents
                                .snapshot(editor.attached_document().unwrap())
                                .ok()
                        })
                        .is_some_and(|s| has_conflict_markers(&s.bytes));
                    if conflicted && markers {
                        state.error = Some(
                            "Remove all conflict markers before staging the resolution.".into(),
                        );
                    } else {
                        state.mutate(
                            if self.key.1 == DiffSide::Staged {
                                Operation::Unstage {
                                    paths: vec![self.key.0.clone()],
                                }
                            } else {
                                Operation::Stage {
                                    paths: vec![self.key.0.clone()],
                                }
                            },
                            services.documents,
                            requests,
                        );
                    }
                }
                if self.key.1 == DiffSide::Unstaged && !conflicted && !diff.old_bytes.is_empty() {
                    ui.same_line();
                    if ui.button("Discard file…") {
                        self.confirm = Some(Operation::Discard {
                            path: self.key.0.clone(),
                            snapshot: diff.snapshot.clone(),
                        });
                    }
                }
                if conflicted && (diff.binary || !diff.can_apply_hunks) {
                    ui.text_wrapped("Choose a whole-file version, then stage the resolution. Text conflicts can also be edited manually.");
                    for (index, (label, choice)) in [
                        ("Use current file", ConflictChoice::Current),
                        ("Use incoming file", ConflictChoice::Incoming),
                        ("Delete file", ConflictChoice::Delete),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if index > 0 {
                            ui.same_line();
                        }
                        if ui.button(label) {
                            if self
                                .editor
                                .as_ref()
                                .and_then(|editor| {
                                    services
                                        .documents
                                        .snapshot(editor.attached_document().unwrap())
                                        .ok()
                                })
                                .is_some_and(|s| s.dirty)
                            {
                                state.error = Some("Save or discard editor changes before choosing a whole-file version.".into());
                            } else if choice == ConflictChoice::Delete {
                                self.confirm = Some(Operation::Resolve {
                                    path: self.key.0.clone(),
                                    choice,
                                });
                            } else {
                                state.mutate(
                                    Operation::Resolve {
                                        path: self.key.0.clone(),
                                        choice,
                                    },
                                    services.documents,
                                    requests,
                                );
                            }
                        }
                    }
                }
            }
            show_error(ui, &mut state, services, requests);
            if let Some(operation) = self.confirm.clone() {
                let (message, button) = confirmation_text(&operation);
                ui.text_wrapped(message);
                if ui.button(button) {
                    state.mutate(operation, services.documents, requests);
                    self.confirm = None;
                }
                ui.same_line();
                if ui.button("Keep changes") {
                    self.confirm = None;
                }
            }
        }
        if diff.binary {
            ui.text_wrapped("Binary or non-text change. Use the whole-file actions above.");
            return Ok(());
        }
        if diff.hunks.is_empty() && diff.old_bytes == diff.new_bytes {
            ui.text_disabled("No textual changes. File mode or other metadata may have changed.");
        }
        match self.sync_editor(&diff, services) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                ui.dummy([0.0, ui.text_line_height()]);
                return Ok(());
            }
            Err(error) => {
                ui.text_wrapped(format!("Cannot open diff editor: {error}"));
                return Ok(());
            }
        }
        if ui.small_button("Previous change") {
            self.navigate_hunk(services, &diff, false)?;
        }
        ui.same_line();
        if ui.small_button("Next change") {
            self.navigate_hunk(services, &diff, true)?;
        }
        ui.same_line();
        ui.text_disabled(format!(
            "{} {}",
            diff.hunks.len(),
            if diff.hunks.len() == 1 {
                "change"
            } else {
                "changes"
            }
        ));
        ui.separator();
        let mut options = self.config.borrow().options.clone();
        options.read_only = self.owns_snapshot;
        let state = self.state.borrow();
        let actions = if diff.can_apply_hunks && state.writable() {
            if self.key.1 == DiffSide::Staged {
                vec![DiffActionKind::Unstage]
            } else {
                vec![DiffActionKind::Stage, DiffActionKind::Discard]
            }
        } else {
            vec![]
        };
        let comparison = state
            .comparisons
            .get(&self.key)
            .copied()
            .unwrap_or_default();
        if self.baseline_comparison != Some(comparison) {
            self.baseline = Arc::from(diff.old_bytes.clone());
            self.baseline_comparison = Some(comparison);
            self.saved = Some(DiffSnapshot {
                current: Arc::from(diff.new_bytes.clone()),
                hunks: diff
                    .hunks
                    .iter()
                    .map(|hunk| DiffHunkRange {
                        old_range: hunk.old_start..hunk.old_start + hunk.old_count,
                        new_range: hunk.new_start..hunk.new_start + hunk.new_count,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            });
        }
        options.diff = Some(DiffPresentation {
            baseline: self.baseline.clone(),
            comparison,
            actions,
            actions_enabled: !state.mutation_pending(),
            saved: self.saved.clone(),
        });
        drop(state);
        if let Some(editor) = &mut self.editor {
            let response = editor.draw_with_options(ui, services, requests, &options)?;
            for action in response.diff_actions {
                if action.kind == DiffActionKind::Discard {
                    // Preserve the exact rendered snapshot for the confirmation.
                    let mut state = self.state.borrow_mut();
                    let dirty = editor
                        .attached_document()
                        .and_then(|document| services.documents.snapshot(document).ok())
                        .is_some_and(|document| document.dirty);
                    if dirty {
                        state.error = Some("Save and refresh before discarding these changes, or use Undo for unsaved edits.".into());
                    } else if state.comparisons.get(&self.key).copied() != Some(action.comparison) {
                        state.error = Some(
                            "This diff changed. Refresh before discarding these changes.".into(),
                        );
                    } else if let Some(diff) = state.diffs.get(&self.key) {
                        self.confirm = Some(Operation::DiscardRange {
                            path: self.key.0.clone(),
                            snapshot: diff.snapshot.clone(),
                            old_range: action.old_range,
                            new_range: action.new_range,
                        });
                    } else {
                        state.error = Some("The displayed changes differ from the saved file. Save and refresh before discarding them.".into());
                    }
                } else {
                    self.state.borrow_mut().hunk_action(
                        &self.key,
                        action,
                        services.documents,
                        requests,
                    );
                }
            }
        }
        Ok(())
    }
    fn action_with_services(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        self.editor
            .as_mut()
            .map(|editor| editor.action_with_services(action, host, services, requests))
            .unwrap_or(Ok(action == PanelAction::CommitEdit))
    }
    fn focus_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<()> {
        if let Some(editor) = &mut self.editor {
            editor.focus_with_services(services)?;
        }
        Ok(())
    }
    fn close_with_services(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if let Some(mut editor) = self.editor.take() {
            let document = editor.attached_document().unwrap();
            editor.close_with_services(services, requests)?;
            if self.owns_snapshot {
                let _ = services.documents.close_document(
                    document,
                    bed_document_session::editor_session::ClosePolicy::Discard,
                );
            }
        }
        Ok(())
    }
    fn view_id(&self) -> Option<ViewId> {
        self.editor.as_ref().and_then(ModulePanel::view_id)
    }
    fn attached_document(&self) -> Option<DocumentId> {
        self.editor
            .as_ref()
            .and_then(ModulePanel::attached_document)
    }
    fn save_state(&self) -> Value {
        json!({"path":self.key.0,"side":if self.key.1 == DiffSide::Staged { "staged" } else { "unstaged" },"editor":self.editor.as_ref().map(|editor| editor.save_state()).unwrap_or_else(||self.editor_state.clone())})
    }
    fn save_state_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<Value> {
        if let Some(editor) = &mut self.editor {
            editor.refresh_state(services.documents)?;
        }
        Ok(self.save_state())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
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
fn confirmation_text(operation: &Operation) -> (&'static str, &'static str) {
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
fn show_error(
    ui: &Ui,
    state: &mut GitController,
    services: &mut ModuleServices<'_>,
    requests: &mut Vec<HostRequest>,
) {
    if let Some(error) = state.error.clone() {
        let _wrap = ui.push_text_wrap_pos(0.0);
        ui.text_colored(
            bed_ui::presentation::readable_color(ui, [0.95, 0.35, 0.3, 1.0]),
            error,
        );
        if ui.small_button("Dismiss") {
            state.error = None;
        }
        same_line_if_room(ui, button_width(ui, "Open Git terminal"));
        if ui.small_button("Open Git terminal") {
            match services
                .terminals
                .new_shell(Some(Path::new(state.repo_root())))
            {
                Ok(id) => requests.push(HostRequest::ShowTerminal { id }),
                Err(error) => state.error = Some(format!("Could not open terminal: {error}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::EditorSession;
    use bed_workbench_api::{FileDialogService, ScopedServices, TerminalLaunch, TerminalService};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Terminals;
    impl TerminalService for Terminals {
        fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
            Err(io::Error::other("unused"))
        }
        fn stop(&mut self, _: u64) {}
        fn release(&mut self, _: u64) {}
    }
    struct Dialogs;
    impl FileDialogService for Dialogs {
        fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<PathBuf> {
            None
        }
    }
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-git-panel-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn sample_diff(side: DiffSide, snapshot: &str, bytes: &[u8]) -> Diff {
        Diff {
            path: "a.rs".into(),
            old_path: None,
            side,
            snapshot: snapshot.into(),
            old_bytes: b"old\n".to_vec(),
            new_bytes: bytes.to_vec(),
            hunks: vec![],
            binary: false,
            can_apply_hunks: false,
        }
    }
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
        controller.status = Some(bed_git::Status {
            root: "/preview".into(),
            branch: "feature/long-branch-name".into(),
            writable: true,
            entries: (0..100)
                .map(|index| StatusEntry {
                    path: format!("nested/long/Unicode_é_文件_{index}.rs"),
                    old_path: None,
                    index_status: 'M',
                    worktree_status: 'M',
                    conflicted: index == 0,
                })
                .collect(),
            ..Default::default()
        });
        controller.draft = "Keep this draft".into();
        let state = Rc::new(RefCell::new(controller));
        let mut panel = GitPanel::new(state.clone());
        let mut session = EditorSession::default();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut icons = bed_ui::icons::Icons::default();
        for key in ["default", "rust", "git-branch", "plus", "minus"] {
            icons.set_texture(key, TextureId::new(1));
        }
        let textures = Default::default();
        let host = HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 0,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        let mut requests = Vec::new();
        for width in [160.0, 240.0, 400.0, 1000.0] {
            let mut idle_position = None;
            for pushing in [false, true] {
                if pushing {
                    state
                        .borrow_mut()
                        .mutate(Operation::Push, &session, &mut requests);
                }
                // Settle native content and scrollbar measurements after each resize.
                for _ in 0..3 {
                    context.prepare_frame(FramePrepareOptions::new([1200.0, 700.0], 1.0 / 60.0));
                    let ui = context.frame();
                    ui.window("Git panel layout fixture")
                        .position([0.0; 2], Condition::Always)
                        .size([width, 650.0], Condition::Always)
                        .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_RESIZE)
                        .build(|| {
                            let mut resources = ScopedServices::default();
                            resources.insert(&mut icons);
                            let mut services = ModuleServices {
                                documents: &mut session,
                                terminals: &mut terminals,
                                dialogs: &mut dialogs,
                                project_root: "/preview",
                                working_directory: "/preview",
                                active_view: None,
                                resources,
                                settings_ui: None,
                            };
                            panel
                                .draw_with_services(ui, &host, &mut services, &mut requests)
                                .unwrap();
                        });
                    assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
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
                    let position = [(*list).Pos.x, (*list).Pos.y];
                    if let Some(idle) = idle_position {
                        assert_eq!(position, idle, "push progress does not shift the file list");
                    } else {
                        idle_position = Some(position);
                    }
                });
            }
            state.borrow_mut().cancel_operation();
        }
        assert_eq!(state.borrow().draft, "Keep this draft");
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
        state.borrow_mut().status.as_mut().unwrap().branches =
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
                    let mut resources = ScopedServices::default();
                    resources.insert(&mut icons);
                    let mut services = ModuleServices {
                        documents: &mut session,
                        terminals: &mut terminals,
                        dialogs: &mut dialogs,
                        project_root: "/preview",
                        working_directory: "/preview",
                        active_view: None,
                        resources,
                        settings_ui: None,
                    };
                    panel
                        .draw_with_services(ui, &host, &mut services, &mut requests)
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
            state.borrow().mutation_pending(),
            "selecting another branch submits the switch"
        );
        state.borrow_mut().cancel_operation();
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
    #[test]
    fn deleted_file_diff_reconnects_to_shared_editable_document_when_file_reappears() {
        let directory = Directory::new();
        let root = directory.0.to_string_lossy().to_string();
        let mut controller = GitController::default();
        controller.root = root.clone();
        let state = Rc::new(RefCell::new(controller));
        let mut session = EditorSession::default();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: &root,
            working_directory: &root,
            active_view: None,
            resources: ScopedServices::default(),
            settings_ui: None,
        };
        let mut panel = DiffPanel::new(
            state,
            Rc::new(RefCell::new(EditorConfig::default())),
            Rc::new(RefCell::new(EditorRuntime::default())),
            None,
            &json!({"path":"a.rs"}),
            &mut services,
        )
        .unwrap();
        panel
            .sync_editor(
                &sample_diff(DiffSide::Unstaged, "deleted", b""),
                &mut services,
            )
            .unwrap();
        let old = panel.attached_document().unwrap();
        assert!(services.documents.is_snapshot_document(old));
        fs::write(directory.0.join("a.rs"), b"restored\n").unwrap();
        panel
            .sync_editor(
                &sample_diff(DiffSide::Unstaged, "restored", b"restored\n"),
                &mut services,
            )
            .unwrap();
        let document = panel.attached_document().unwrap();
        assert_ne!(document, old);
        assert!(!services.documents.is_snapshot_document(document));
        assert_eq!(
            services
                .documents
                .document_for_path(&directory.0.join("a.rs")),
            Some(document)
        );
        assert!(services.documents.snapshot(old).is_err());
        panel
            .close_with_services(&mut services, &mut vec![])
            .unwrap();
        assert_eq!(services.documents.view_count(document), 0);
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"restored\n"
        );
    }
    #[test]
    fn staged_refresh_preserves_view_and_snapshot_remains_read_only_until_closed() {
        let directory = Directory::new();
        let root = directory.0.to_string_lossy().to_string();
        let mut controller = GitController::default();
        controller.root = root.clone();
        let state = Rc::new(RefCell::new(controller));
        let mut session = EditorSession::default();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: &root,
            working_directory: &root,
            active_view: None,
            resources: ScopedServices::default(),
            settings_ui: None,
        };
        let mut panel = DiffPanel::new(
            state,
            Rc::new(RefCell::new(EditorConfig::default())),
            Rc::new(RefCell::new(EditorRuntime::default())),
            None,
            &json!({"path":"a.rs","side":"staged"}),
            &mut services,
        )
        .unwrap();
        panel
            .sync_editor(
                &sample_diff(DiffSide::Staged, "index1", b"one\n"),
                &mut services,
            )
            .unwrap();
        let document = panel.attached_document().unwrap();
        let view = panel.view_id().unwrap();
        panel
            .sync_editor(
                &sample_diff(DiffSide::Staged, "index2", b"two\n"),
                &mut services,
            )
            .unwrap();
        assert_eq!(panel.view_id(), Some(view));
        assert_eq!(
            services.documents.snapshot(document).unwrap().bytes,
            b"two\n"
        );
        assert!(services.documents.is_snapshot_document(document));
        assert!(services.documents.save(document).is_err());
        panel
            .close_with_services(&mut services, &mut vec![])
            .unwrap();
        assert!(services.documents.snapshot(document).is_err());
    }
}
