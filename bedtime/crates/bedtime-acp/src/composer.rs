//! The composer: what's under a chat's transcript. The agent's plan, the
//! prompts waiting their turn, what's attached, the prompt itself (with `/`
//! commands and `@` files completed in a list over it), and the session's
//! options beside Send.
use crate::{
    files::{FileRows, WorkspaceFile, active_at_mention},
    intents::AcpIntent,
    panel::{AcpPanel, AcpRuntime, add_fitted_button, one_line_preview},
    thread::{AcpThread, QueuedPrompt, SessionState},
    worker::ApprovalMode,
};
use agent_client_protocol::v1::schema::{
    AvailableCommand, AvailableCommandInput, PlanEntry, PlanEntryStatus, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOptions, SessionConfigValueId, SessionModeId,
};
use bedtime_ui::{
    BuiltinIcon, Button, Card, Column, Row, Select, Separator, Size, Space, Spinner, Text,
    TextArea, Tip, Toggle, Tone, UiContext, egui,
    picker::{self, Picker, PickerEvent, PickerRows, PickerState},
    theme,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

/// What the composer is drawn with, beside the panel's state and its thread.
pub(crate) struct Composer<'a> {
    pub panel: Entity,
    pub saved: &'a AcpPanel,
    /// The workspace's files, for `@`.
    pub files: &'a [WorkspaceFile],
    pub cwd: &'a Path,
    pub supports_image: bool,
    pub has_worker: bool,
    pub pointer_in_panel: bool,
}

pub(crate) fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rt: &mut AcpRuntime,
    thread: &mut Option<AcpThread>,
    with: Composer<'_>,
) {
    let palette = &context.theme.palette;
    let saved = with.saved;
    let has_worker = with.has_worker;
    let is_streaming = thread.as_ref().is_some_and(|thread| thread.streaming);

    if let Some(chat) = thread.as_mut()
        && !chat.plan.is_empty()
    {
        draw_plan_strip(ui, context, &chat.plan, &mut chat.plan_expanded);
        ui.add(Separator::new().space(Space::Xs));
    }
    if let Some(chat) = thread.as_ref() {
        queued(ui, chat, is_streaming, &mut rt.intents);
    }
    attached(ui, rt, with.supports_image);

    // The `/` commands and `@` files that fit what's typed. Their keys are
    // taken here, before the text area can have them.
    let input_id = egui::Id::new((with.panel, "acp-prompt-input"));
    let has_focus = ui.memory(|m| m.has_focus(input_id));
    let commands: &[AvailableCommand] = thread
        .as_ref()
        .map_or(&[], |chat| chat.available_commands.as_slice());

    let mut chosen_slash: Option<AvailableCommand> = None;
    let mut slash_open = false;
    match active_slash_command(&rt.input).map(str::to_owned) {
        Some(query) if !rt.slash_dismissed && !commands.is_empty() => {
            if rt.slash_seen != commands.len() {
                rt.slash_seen = commands.len();
                rt.slash.stale();
            }
            rt.slash.query = query;
            let mut rows = SlashRows {
                all: commands,
                shown: &mut rt.slash_rows,
            };
            let event = if has_focus {
                Picker::keys(ui, &mut rt.slash, &mut rows)
            } else {
                rt.slash.filter(&mut rows);
                None
            };
            match event {
                Some(PickerEvent::Chosen(row)) => chosen_slash = rows.command(row),
                Some(PickerEvent::Escaped) => rt.slash_dismissed = true,
                _ => slash_open = !rows.is_empty(),
            }
        }
        Some(_) => {}
        None => {
            rt.slash_dismissed = false;
            rt.slash = PickerState::default();
        }
    }

    let slash_active = slash_open || chosen_slash.is_some();
    let mention = if slash_active {
        None
    } else {
        active_at_mention(&rt.input).map(|(at, query)| (at, query.to_owned()))
    };
    if mention.as_ref().map(|(at, _)| *at) != rt.mention_dismissed_at {
        rt.mention_dismissed_at = None;
    }
    let mut chosen_mention: Option<(usize, PathBuf)> = None;
    let mut mention_open: Option<usize> = None;
    match mention {
        Some((at, query)) if rt.mention_dismissed_at != Some(at) => {
            if rt.mention_seen != with.files.len() {
                rt.mention_seen = with.files.len();
                rt.mention.stale();
            }
            rt.mention.query = query;
            let mut rows = FileRows {
                files: with.files,
                shown: &mut rt.mention_rows,
            };
            let event = if has_focus {
                Picker::keys(ui, &mut rt.mention, &mut rows)
            } else {
                rt.mention.filter(&mut rows);
                None
            };
            match event {
                Some(PickerEvent::Chosen(row)) => {
                    chosen_mention = rows.path(row).map(|path| (at, path));
                }
                Some(PickerEvent::Escaped) => rt.mention_dismissed_at = Some(at),
                _ if !rows.is_empty() => mention_open = Some(at),
                _ => {}
            }
        }
        Some(_) => {}
        None => rt.mention = PickerState::default(),
    }

    let overlay_open = slash_active || mention_open.is_some() || chosen_mention.is_some();
    let paste_image_shortcut = ui.input(|i| {
        let has_text_paste = i
            .events
            .iter()
            .any(|ev| matches!(ev, egui::Event::Paste(_)));
        let paste_key = i.modifiers.command && i.key_pressed(egui::Key::V);
        paste_key
            && !has_text_paste
            && (has_focus || (with.pointer_in_panel && !ui.ctx().egui_wants_keyboard_input()))
    });
    if paste_image_shortcut {
        rt.intents.push(AcpIntent::PasteClipboardImage);
    }
    // Enter sends, so it's taken before the text area can make a line of
    // it; Shift+Enter is left to it.
    let (enter_pressed, cmd_enter_pressed) = if has_focus && !overlay_open {
        ui.input_mut(|i| {
            if i.modifiers.shift {
                return (false, false);
            }
            let cmd = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
            let plain = cmd || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            (plain, cmd)
        })
    } else {
        (false, false)
    };

    let hint = if is_streaming {
        "Queue a follow-up… (Enter to queue, Cmd/Ctrl+Enter to interrupt & send)"
    } else if !commands.is_empty() {
        "Message the agent… (/ for commands, @ for files, Cmd/Ctrl+V for images)"
    } else {
        "Message the agent… (@ for files, Cmd/Ctrl+V for images, Shift+Enter for newline)"
    };
    let input = ui.add(
        TextArea::new(&mut rt.input)
            .id(input_id)
            .hint(hint)
            .label("Prompt")
            .rows(3)
            .fill_width(),
    );

    if slash_open {
        let mut rows = SlashRows {
            all: commands,
            shown: &mut rt.slash_rows,
        };
        let picked = Picker::new(
            egui::Id::new((with.panel, "acp-slash")),
            input.rect.center_top(),
            480.0,
        )
        .hint("Commands: Enter inserts, Esc closes")
        .max_rows(8)
        .show_above(ui, context, input.rect, &mut rt.slash, &mut rows);
        if let Some(PickerEvent::Chosen(row)) = picked {
            chosen_slash = rows.command(row);
        }
    }
    if let Some(command) = chosen_slash {
        if command.input.is_some() {
            rt.input = format!("/{} ", command.name);
            rt.slash_dismissed = false;
        } else {
            rt.input = format!("/{}", command.name);
            rt.slash_dismissed = true;
        }
        rt.slash = PickerState::default();
        ui.memory_mut(|m| m.request_focus(input_id));
    }

    if let Some(at) = mention_open {
        let mut rows = FileRows {
            files: with.files,
            shown: &mut rt.mention_rows,
        };
        let picked = Picker::new(
            egui::Id::new((with.panel, "acp-at")),
            input.rect.center_top(),
            440.0,
        )
        .hint("Files: Enter attaches, Esc closes")
        .max_rows(6)
        .show_above(ui, context, input.rect, &mut rt.mention, &mut rows);
        if let Some(PickerEvent::Chosen(row)) = picked {
            chosen_mention = rows.path(row).map(|path| (at, path));
        }
    }
    if let Some((at, path)) = chosen_mention {
        rt.input.truncate(at);
        rt.intents
            .push(AcpIntent::AttachPaths(vec![with.cwd.join(path)]));
        rt.mention = PickerState::default();
        rt.mention_dismissed_at = None;
        ui.memory_mut(|m| m.request_focus(input_id));
    }

    let has_payload = !rt.input.trim().is_empty() || !rt.attachments.is_empty();
    if enter_pressed && has_payload {
        rt.intents.push(AcpIntent::SubmitPrompt {
            interrupt: is_streaming && cmd_enter_pressed,
        });
    }

    ui.add_space(Space::Xs.px(ui));

    // Model, reasoning, mode and approval on the left; Queue, Stop and Send
    // on the right.
    let config_options = thread
        .as_ref()
        .map(|c| c.config_options.clone())
        .unwrap_or_default();
    let modes = thread.as_ref().and_then(|c| c.modes.clone());
    let has_session = thread
        .as_ref()
        .is_some_and(|c| c.session == SessionState::Ready);

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_streaming {
                if ui.add(Button::new("Stop").danger().small()).clicked() {
                    rt.intents.push(AcpIntent::CancelPrompt);
                }
                if has_payload {
                    let queue_btn = ui.add(Button::new("Queue").primary().small());
                    (&queue_btn)
                        .tip("Queue message for next turn (or Cmd/Ctrl+Enter to interrupt & send)");
                    if queue_btn.clicked() {
                        rt.intents
                            .push(AcpIntent::SubmitPrompt { interrupt: false });
                    }
                }
                ui.add(Spinner::new().size(Size::Sm));
            } else {
                let can_send = has_payload && has_worker;
                if ui
                    .add_enabled(can_send, Button::new("Send").primary().small())
                    .clicked()
                {
                    rt.intents
                        .push(AcpIntent::SubmitPrompt { interrupt: false });
                }
            }

            // Remaining width for wrapping selector controls.
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                |ui| {
                    let mut ordered: Vec<&SessionConfigOption> = config_options.iter().collect();
                    ordered.sort_by_key(|opt| config_category_rank(opt.category.as_ref()));

                    let has_mode_config = ordered
                        .iter()
                        .any(|opt| matches!(opt.category, Some(SessionConfigOptionCategory::Mode)));

                    for opt in ordered {
                        ui.add_enabled_ui(has_session, |ui| {
                            if let Some(new_val) = draw_config_option_control(ui, opt) {
                                rt.intents
                                    .push(AcpIntent::SetConfigOption(opt.id.clone(), new_val));
                            }
                        });
                    }

                    // Fallback ACP modes (if agent didn't provide a Mode config option).
                    if !has_mode_config
                        && let Some(mode_state) = &modes
                        && !mode_state.available_modes.is_empty()
                    {
                        ui.add_enabled_ui(has_session, |ui| {
                            let ids: Vec<SessionModeId> = mode_state
                                .available_modes
                                .iter()
                                .map(|m| m.id.clone())
                                .collect();
                            let mut current = mode_state.current_mode_id.clone();
                            let resp = ui.add(
                                Select::with(&mut current, &ids, |id| {
                                    mode_state
                                        .available_modes
                                        .iter()
                                        .find(|m| &m.id == id)
                                        .map(|m| m.name.clone())
                                        .unwrap_or_else(|| id.0.to_string())
                                })
                                .size(Size::Sm),
                            );
                            (&resp).tip("Session mode");
                            if resp.changed() {
                                rt.intents.push(AcpIntent::SetMode(current));
                            }
                        });
                    }

                    // Client-side tool approval policy (always available).
                    let mut approval = thread
                        .as_ref()
                        .map_or(saved.approval_mode, |t| t.approval_mode);
                    let is_auto = approval == ApprovalMode::AutoApprove;
                    let mut select =
                        Select::with(&mut approval, &ApprovalMode::ALL, |m| m.label().to_owned())
                            .size(Size::Sm);
                    if is_auto {
                        select = select.color(theme::color(palette.warning));
                    }
                    let resp = ui.add(select);
                    (&resp).tip(match approval {
                        ApprovalMode::Ask => "Approval mode: Ask before running tools",
                        ApprovalMode::AutoApprove => {
                            "Approval mode: Automatically allow tool calls"
                        }
                    });
                    if resp.changed() {
                        rt.intents.push(AcpIntent::SetApprovalMode(approval));
                    }
                },
            );
        });
    });
}

/// The prompts waiting for the agent to finish its turn.
fn queued(ui: &mut egui::Ui, chat: &AcpThread, is_streaming: bool, intents: &mut Vec<AcpIntent>) {
    if chat.queued_prompts.is_empty() {
        return;
    }
    Column::new().gap(Space::Xs).show(ui, |ui| {
        for (idx, queued) in chat.queued_prompts.iter().enumerate() {
            Card::new().small().show(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let remove = ui.add(
                        Button::icon_only(BuiltinIcon::Close, "Remove queued message")
                            .ghost()
                            .small(),
                    );
                    if remove.clicked() {
                        intents.push(AcpIntent::RemoveQueued(idx));
                    }
                    if is_streaming {
                        let now = ui.add(Button::new("Send now").ghost().small());
                        (&now).tip("Cancel current turn and send this message next");
                        if now.clicked() {
                            intents.push(AcpIntent::SendQueuedNow(idx));
                        }
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(Text::caption("Queued").tone(Tone::Accent));
                        ui.add(Text::caption(queued_preview(queued)).muted().truncate());
                    });
                });
            });
        }
    });
    ui.add_space(Space::Xs.px(ui));
}

/// The files and images attached to the prompt being written, and what
/// went wrong attaching the last one.
fn attached(ui: &mut egui::Ui, rt: &mut AcpRuntime, supports_image: bool) {
    if !rt.attachments.is_empty() {
        let mut remove = None;
        Row::new().wrap().gap(Space::Sm).show(ui, |ui| {
            for (idx, attachment) in rt.attachments.iter().enumerate() {
                let icon = if attachment.is_image() {
                    BuiltinIcon::ViewerImage
                } else {
                    BuiltinIcon::Text
                };
                let unsupported = attachment.is_image() && !supports_image;
                let tone = if unsupported {
                    Tone::Warning
                } else {
                    Tone::Neutral
                };
                Card::new().small().tone(tone).show(ui, |ui| {
                    Row::new().gap(Space::Xs).show(ui, |ui| {
                        let chip = add_fitted_button(
                            ui,
                            Button::new(attachment.label()).icon(icon).ghost().small(),
                            (ui.available_width() - 28.0).max(40.0),
                            Some(attachment.label()),
                        );
                        if unsupported {
                            (&chip).tip("Agent does not advertise ACP image support");
                        }
                        let close = ui.add(
                            Button::icon_only(BuiltinIcon::Close, "Remove attachment")
                                .ghost()
                                .small(),
                        );
                        if close.clicked() {
                            remove = Some(idx);
                        }
                    });
                });
            }
        });
        if let Some(idx) = remove {
            rt.attachments.remove(idx);
        }
        ui.add_space(Space::Xs.px(ui));
    }

    if let Some(err) = &rt.attachment_error {
        let mut dismiss = false;
        Row::new().gap(Space::Xs).show(ui, |ui| {
            ui.add(Text::caption(err).tone(Tone::Warning).wrap());
            let close = ui.add(
                Button::icon_only(BuiltinIcon::Close, "Dismiss")
                    .ghost()
                    .small(),
            );
            if close.clicked() {
                dismiss = true;
            }
        });
        if dismiss {
            rt.attachment_error = None;
        }
        ui.add_space(Space::Xs.px(ui));
    }
}

/// The agent's `/` commands that fit what's typed, as a picker's rows. The
/// rows found are kept by the panel (`shown`), with the characters matched.
struct SlashRows<'a> {
    all: &'a [AvailableCommand],
    shown: &'a mut Vec<(AvailableCommand, Vec<usize>)>,
}

impl SlashRows<'_> {
    fn command(&self, row: usize) -> Option<AvailableCommand> {
        self.shown.get(row).map(|(command, _)| command.clone())
    }
}

impl PickerRows for SlashRows<'_> {
    fn filter(&mut self, query: &str) {
        *self.shown = filter_slash_commands(self.all, query);
    }

    fn len(&self) -> usize {
        self.shown.len()
    }

    fn label(&self, row: usize) -> String {
        format!("/{}", self.shown[row].0.name)
    }

    fn paint(&self, ui: &mut egui::Ui, context: &UiContext<'_>, rect: egui::Rect, row: usize) {
        let (command, matched) = &self.shown[row];
        // The `/` isn't part of what's matched.
        let matched: Vec<usize> = matched.iter().map(|at| at + 1).collect();
        paint_row(
            ui,
            context,
            rect,
            &format_slash_label(command),
            &matched,
            &command.description,
        );
    }
}

/// A completion's row: `label` with the matched characters marked, then
/// `detail`, quieter, in what room is left.
pub(crate) fn paint_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    label: &str,
    matched: &[usize],
    detail: &str,
) {
    let palette = &context.theme.palette;
    let text = theme::color(palette.text);
    let muted = theme::color(palette.muted);
    let font = egui::FontId::proportional(context.metrics.font_size);
    let mut job = picker::highlighted(
        label,
        matched,
        font.clone(),
        text,
        theme::color(palette.accent),
    );
    if !detail.is_empty() {
        job.append(detail, 12.0, egui::TextFormat::simple(font, muted));
    }
    job.wrap.max_width = (rect.width() - 16.0).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
}

fn config_category_rank(category: Option<&SessionConfigOptionCategory>) -> u8 {
    match category {
        Some(SessionConfigOptionCategory::Model) => 0,
        Some(SessionConfigOptionCategory::ThoughtLevel) => 1,
        Some(SessionConfigOptionCategory::Mode) => 2,
        Some(SessionConfigOptionCategory::ModelConfig) => 3,
        _ => 4,
    }
}

fn draw_config_option_control(
    ui: &mut egui::Ui,
    opt: &SessionConfigOption,
) -> Option<SessionConfigOptionValue> {
    let tooltip = match &opt.description {
        Some(desc) if !desc.trim().is_empty() => format!("{}: {}", opt.name, desc),
        _ => opt.name.clone(),
    };

    match &opt.kind {
        SessionConfigKind::Select(select) => {
            let mut values: Vec<SessionConfigValueId> = Vec::new();
            let mut labels: Vec<String> = Vec::new();
            let mut dividers: Vec<usize> = Vec::new();

            match &select.options {
                SessionConfigSelectOptions::Ungrouped(items) => {
                    for item in items {
                        values.push(item.value.clone());
                        labels.push(item.name.clone());
                    }
                }
                SessionConfigSelectOptions::Grouped(groups) => {
                    for (group_idx, group) in groups.iter().enumerate() {
                        if group_idx > 0 && !group.options.is_empty() {
                            dividers.push(values.len());
                        }
                        for item in &group.options {
                            values.push(item.value.clone());
                            labels.push(item.name.clone());
                        }
                    }
                }
                _ => return None,
            }

            if values.is_empty() {
                return None;
            }

            let mut current = select.current_value.clone();
            let mut widget = Select::with(&mut current, &values, |val| {
                values
                    .iter()
                    .position(|v| v == val)
                    .map(|idx| labels[idx].clone())
                    .unwrap_or_else(|| val.0.to_string())
            })
            .size(Size::Sm);

            if !dividers.is_empty() {
                widget = widget.dividers(dividers);
            }
            if values.len() > 8 {
                widget = widget.searchable();
            }

            let resp = ui.add(widget);
            (&resp).tip(tooltip);
            if resp.changed() {
                Some(SessionConfigOptionValue::value_id(current))
            } else {
                None
            }
        }
        SessionConfigKind::Boolean(boolean) => {
            let mut current = boolean.current_value;
            let resp = ui.add(Toggle::new(&mut current, &opt.name).size(Size::Sm));
            (&resp).tip(tooltip);
            if resp.changed() {
                Some(SessionConfigOptionValue::boolean(current))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn draw_plan_strip(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    entries: &[PlanEntry],
    expanded: &mut bool,
) {
    let palette = &context.theme.palette;
    let completed = entries
        .iter()
        .filter(|e| e.status == PlanEntryStatus::Completed)
        .count();
    let total = entries.len();
    let active = entries
        .iter()
        .find(|e| e.status == PlanEntryStatus::InProgress);

    let icon = if *expanded {
        BuiltinIcon::ChevronDown
    } else {
        BuiltinIcon::ChevronRight
    };
    let header_label = match active {
        Some(entry) if !*expanded => {
            format!("Plan ({completed}/{total}) — {}", entry.content)
        }
        _ => format!("Plan ({completed}/{total})"),
    };

    let header_btn = add_fitted_button(
        ui,
        Button::new(&header_label).icon(icon).ghost().small(),
        ui.available_width().max(40.0),
        Some(&header_label),
    );
    if header_btn.clicked() {
        *expanded = !*expanded;
    }

    if *expanded {
        let indent = 12.0;
        let response = ui.indent(ui.id().with("acp-plan-items"), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for entry in entries {
                Row::new().gap(Space::Xs).show(ui, |ui| match entry.status {
                    PlanEntryStatus::Completed => {
                        ui.add(Text::caption("✓").tone(Tone::Success));
                        ui.add(
                            Text::caption(&entry.content)
                                .muted()
                                .strikethrough(true)
                                .wrap(),
                        );
                    }
                    PlanEntryStatus::InProgress => {
                        ui.add(Spinner::new().size(Size::Sm));
                        ui.add(Text::caption(&entry.content).wrap());
                    }
                    PlanEntryStatus::Pending => {
                        ui.add(Text::caption("○").muted());
                        ui.add(Text::caption(&entry.content).muted().wrap());
                    }
                    _ => {
                        ui.add(Text::caption(&entry.content).muted().wrap());
                    }
                });
            }
        });
        let rect = response.response.rect;
        ui.painter().vline(
            rect.left() - indent * 0.5,
            rect.y_range(),
            egui::Stroke::new(1.5, theme::color(palette.border)),
        );
    }
}

fn queued_preview(queued: &QueuedPrompt) -> String {
    let text_part = one_line_preview(&queued.text, 60);
    match (text_part.is_empty(), queued.attachments.len()) {
        (true, 1) => format!("[{}]", queued.attachments[0].label()),
        (true, n) => format!("[{n} attachments]"),
        (false, 0) => text_part,
        (false, 1) => format!("{text_part} (+1 attachment)"),
        (false, n) => format!("{text_part} (+{n} attachments)"),
    }
}

/// Extract an active `/command` query when `input` starts with `/` and has no whitespace yet.
fn active_slash_command(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// The commands `query` fits, best first, each with the characters of its
/// name that matched: by name, then those that only mention it in their
/// description.
fn filter_slash_commands(
    commands: &[AvailableCommand],
    query: &str,
) -> Vec<(AvailableCommand, Vec<usize>)> {
    let lower = query.to_lowercase();
    let mut scored: Vec<(i32, usize, Vec<usize>)> = commands
        .iter()
        .enumerate()
        .filter_map(
            |(index, command)| match picker::fuzzy(&command.name, query) {
                Some((score, matched)) => Some((score, index, matched)),
                None => command
                    .description
                    .to_lowercase()
                    .contains(&lower)
                    .then(|| (i32::MIN, index, Vec::new())),
            },
        )
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .map(|(_, index, matched)| (commands[index].clone(), matched))
        .collect()
}

fn format_slash_label(cmd: &AvailableCommand) -> String {
    match &cmd.input {
        Some(AvailableCommandInput::Unstructured(input)) if !input.hint.trim().is_empty() => {
            format!("/{} <{}>", cmd.name, input.hint.trim())
        }
        _ => format!("/{}", cmd.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_command_detection() {
        assert_eq!(active_slash_command("/"), Some(""));
        assert_eq!(active_slash_command("/plan"), Some("plan"));
        assert_eq!(active_slash_command("/plan fix bug"), None);
        assert_eq!(active_slash_command("hello /plan"), None);
    }

    #[test]
    fn slash_commands_are_ranked_by_name_then_description() {
        let commands = vec![
            AvailableCommand::new("research_plan", "Research and plan"),
            AvailableCommand::new("plan", "Create an implementation plan"),
            AvailableCommand::new("compact", "Compact conversation history"),
            AvailableCommand::new("review", "Look over the plan's diff"),
        ];
        let names = |query: &str| -> Vec<String> {
            filter_slash_commands(&commands, query)
                .into_iter()
                .map(|(command, _)| command.name)
                .collect()
        };
        assert_eq!(names("pl"), ["plan", "research_plan", "review"]);
        assert_eq!(names("").len(), 4, "everything, before anything is typed");
        let (_, matched) = &filter_slash_commands(&commands, "pl")[0];
        assert_eq!(matched, &[0, 1]);
    }
}
