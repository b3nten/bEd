//! The transcript: what was said in a chat, the agent's thinking and tool
//! calls, and what it's waiting to be allowed.
use crate::{
    connection::ConnectionStatus,
    intents::AcpIntent,
    panel::{add_fitted_button, one_line_preview},
    thread::{AcpThread, PendingPermission, SessionState, TranscriptItem},
};
use agent_client_protocol::v1::schema::{
    ContentBlock, ToolCall, ToolCallContent, ToolCallStatus, ToolKind,
};
use bedtime_ui::{
    BuiltinIcon, Button, Card, Row, Size, Space, Spinner, Text, TextRole, Tip, Tone, UiContext,
    egui,
    markdown::{MarkdownStyle, MarkdownView},
    theme,
};
use bevy::prelude::*;

/// The transcript of `thread`, scrolling, under the connection's error if
/// it has one. Wide code blocks and tables scroll sideways by themselves.
pub(crate) fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    thread: &mut Option<AcpThread>,
    status: &ConnectionStatus,
    client_name: &str,
    intents: &mut Vec<AcpIntent>,
) {
    let scroll_to_bottom = thread
        .as_mut()
        .is_some_and(|thread| std::mem::take(&mut thread.scroll_to_bottom));
    if let ConnectionStatus::Error(err) = status {
        Card::new().tone(Tone::Error).show(ui, |ui| {
            Row::new().show(ui, |ui| {
                ui.add(Text::new(err).wrap());
                if ui.add(Button::new("Retry").small()).clicked() {
                    intents.push(AcpIntent::Reconnect);
                }
            });
        });
        ui.add_space(Space::Sm.px(ui));
    }

    egui::ScrollArea::vertical()
        .id_salt((panel, "acp-transcript"))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let wrap_width = ui.available_width().max(120.0);
            ui.set_max_width(wrap_width);
            let Some(chat) = thread.as_mut() else {
                ui.add_space(Space::Lg.px(ui));
                ui.add(
                    Text::new(format!(
                        "ACP client \"{client_name}\" is not configured in Settings."
                    ))
                    .muted()
                    .wrap(),
                );
                return;
            };

            if chat.session == SessionState::Loading {
                ui.add_space(Space::Lg.px(ui));
                Row::new().show(ui, |ui| {
                    ui.add(Spinner::new().size(Size::Sm));
                    ui.add(Text::new("Loading session…").muted());
                });
                return;
            }

            if chat.items.is_empty() && chat.error.is_none() {
                ui.add_space(Space::Lg.px(ui));
                ui.add(Text::new(format!("Start a conversation with {client_name}.")).muted());
            }

            let item_count = chat.items.len();
            let streaming = chat.streaming;
            for (idx, item) in chat.items.iter_mut().enumerate() {
                let is_last = idx + 1 == item_count;
                let is_compact = matches!(
                    item,
                    TranscriptItem::Thought { .. } | TranscriptItem::Tool { .. }
                );
                draw_transcript_item(ui, context, idx, item, streaming && is_last, intents);
                ui.add_space(if is_compact {
                    Space::Xs.px(ui)
                } else {
                    Space::Sm.px(ui)
                });
            }

            for perm in &chat.pending_permissions {
                if let Some(reply) = draw_permission_card(ui, perm) {
                    intents.push(AcpIntent::RespondPermission {
                        request_id: perm.request_id,
                        option_id: reply,
                    });
                }
                ui.add_space(Space::Sm.px(ui));
            }

            if let Some(err) = &chat.error {
                Card::new().tone(Tone::Error).show(ui, |ui| {
                    ui.add(Text::new(err).wrap());
                });
            }

            if scroll_to_bottom {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
}

fn draw_transcript_item(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    idx: usize,
    item: &mut TranscriptItem,
    is_streaming_last: bool,
    intents: &mut Vec<AcpIntent>,
) {
    let palette = &context.theme.palette;
    match item {
        TranscriptItem::User { text, attachments } => {
            Card::new().fill_width().show(ui, |ui| {
                if !attachments.is_empty() {
                    Row::new().wrap().gap(Space::Sm).show(ui, |ui| {
                        for (label, is_image) in attachments.iter() {
                            Card::new().small().show(ui, |ui| {
                                let prefix = if *is_image { "Image: " } else { "@" };
                                ui.add(Text::caption(format!("{prefix}{label}")).muted());
                            });
                        }
                    });
                    if !text.is_empty() {
                        ui.add_space(Space::Xs.px(ui));
                    }
                }
                if !text.is_empty() {
                    ui.add(Text::new(text.as_str()).wrap().selectable());
                }
            });
        }
        TranscriptItem::Agent { text, view } => {
            let markdown_view = view.get_or_insert_with(|| {
                MarkdownView::new(text)
                    .style(MarkdownStyle::Compact)
                    .code_language("")
            });
            ui.push_id(("acp-md", idx), |ui| {
                if let Some(link) = markdown_view.show(ui, context) {
                    intents.push(AcpIntent::OpenLink(link));
                }
            });
        }
        TranscriptItem::Thought { text, expanded } => {
            let icon = if *expanded {
                BuiltinIcon::ChevronDown
            } else {
                BuiltinIcon::ChevronRight
            };
            let title = if is_streaming_last {
                "Thinking…".to_owned()
            } else if *expanded {
                "Thought".to_owned()
            } else {
                let preview = one_line_preview(text, 72);
                if preview.is_empty() {
                    "Thought".to_owned()
                } else {
                    format!("Thought · {preview}")
                }
            };

            Row::new().gap(Space::Xs).show(ui, |ui| {
                if is_streaming_last {
                    ui.add(Spinner::new().size(Size::Sm));
                }
                let btn = add_fitted_button(
                    ui,
                    Button::new(&title).icon(icon).ghost().small(),
                    ui.available_width(),
                    Some(&title),
                );
                if btn.clicked() {
                    *expanded = !*expanded;
                }
            });

            if *expanded && !text.trim().is_empty() {
                let response = ui.indent(("acp-thought-body", idx), |ui| {
                    ui.add(Text::caption(text.trim()).muted().wrap().selectable());
                });
                let rect = response.response.rect;
                ui.painter().vline(
                    rect.left() - 8.0,
                    rect.y_range(),
                    egui::Stroke::new(1.5, theme::color(palette.border)),
                );
            }
        }
        TranscriptItem::Tool { call, expanded } => {
            let has_details = !call.content.is_empty()
                || !call.locations.is_empty()
                || call.raw_input.is_some()
                || call.raw_output.is_some();

            let label = compact_tool_label(call);
            let icon = if has_details {
                if *expanded {
                    BuiltinIcon::ChevronDown
                } else {
                    BuiltinIcon::ChevronRight
                }
            } else {
                tool_kind_icon(call.kind)
            };

            Row::new().gap(Space::Xs).show(ui, |ui| {
                match call.status {
                    ToolCallStatus::Pending | ToolCallStatus::InProgress => {
                        ui.add(Spinner::new().size(Size::Sm));
                    }
                    ToolCallStatus::Completed => {
                        ui.add(Text::caption("✓").tone(Tone::Success));
                    }
                    ToolCallStatus::Failed => {
                        ui.add(Text::caption("✕").tone(Tone::Error));
                    }
                    _ => {}
                }

                if has_details {
                    let btn = add_fitted_button(
                        ui,
                        Button::new(&label).icon(icon).ghost().small(),
                        ui.available_width(),
                        Some(&label),
                    );
                    if btn.clicked() {
                        *expanded = !*expanded;
                    }
                } else {
                    let mut text = Text::caption(&label).truncate();
                    if call.status == ToolCallStatus::Completed {
                        text = text.muted();
                    } else if call.status == ToolCallStatus::Failed {
                        text = text.tone(Tone::Error);
                    }
                    ui.add(text);
                }
            });

            if *expanded && has_details {
                let response = ui.indent(("acp-tool-body", idx), |ui| {
                    Card::new().small().fill_width().show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = Space::Xs.px(ui);

                        if !call.locations.is_empty() {
                            Row::new().wrap().gap(Space::Sm).show(ui, |ui| {
                                for loc in &call.locations {
                                    let loc_label = match loc.line {
                                        Some(line) => format!("{}:{line}", loc.path.display()),
                                        None => loc.path.display().to_string(),
                                    };
                                    let link = ui.add(
                                        Text::caption(&loc_label).tone(Tone::Accent).clickable(),
                                    );
                                    (&link).tip("Open file");
                                    if link.clicked() {
                                        intents.push(AcpIntent::OpenFile {
                                            path: loc.path.clone(),
                                            line: loc.line,
                                        });
                                    }
                                }
                            });
                        }

                        for content in &call.content {
                            match content {
                                ToolCallContent::Content(inner) => {
                                    if let ContentBlock::Text(t) = &inner.content {
                                        ui.add(
                                            Text::caption(truncate_text(&t.text, 1200))
                                                .wrap()
                                                .selectable(),
                                        );
                                    }
                                }
                                ToolCallContent::Diff(diff) => {
                                    let diff_link = ui.add(
                                        Text::caption(format!("Diff: {}", diff.path.display()))
                                            .tone(Tone::Accent)
                                            .strong()
                                            .clickable(),
                                    );
                                    if diff_link.clicked() {
                                        intents.push(AcpIntent::OpenFile {
                                            path: diff.path.clone(),
                                            line: None,
                                        });
                                    }
                                    ui.add(
                                        Text::caption(truncate_text(&diff.new_text, 1000))
                                            .muted()
                                            .wrap()
                                            .selectable(),
                                    );
                                }
                                _ => {}
                            }
                        }

                        if call.content.is_empty() {
                            if let Some(input) = &call.raw_input
                                && let Ok(pretty) = serde_json::to_string_pretty(input)
                            {
                                ui.add(
                                    Text::caption(truncate_text(&pretty, 800))
                                        .muted()
                                        .wrap()
                                        .selectable(),
                                );
                            }
                            if let Some(output) = &call.raw_output
                                && let Ok(pretty) = serde_json::to_string_pretty(output)
                            {
                                ui.add(
                                    Text::caption(truncate_text(&pretty, 1000))
                                        .wrap()
                                        .selectable(),
                                );
                            }
                        }
                    });
                });
                let rect = response.response.rect;
                ui.painter().vline(
                    rect.left() - 8.0,
                    rect.y_range(),
                    egui::Stroke::new(1.5, theme::color(palette.border)),
                );
            }
        }
    }
}

fn draw_permission_card(ui: &mut egui::Ui, perm: &PendingPermission) -> Option<Option<String>> {
    let mut chosen = None;
    Card::new().tone(Tone::Warning).fill_width().show(ui, |ui| {
        ui.add(Text::new("Permission requested").role(TextRole::Title));
        ui.add_space(Space::Xs.px(ui));
        ui.add(Text::new(&perm.title).wrap());
        ui.add_space(Space::Sm.px(ui));
        Row::new().wrap().show(ui, |ui| {
            for opt in &perm.options {
                let button = if opt.is_allow && !opt.is_always {
                    Button::new(&opt.name).primary().small()
                } else {
                    Button::new(&opt.name).small()
                };
                if ui.add(button).clicked() {
                    chosen = Some(Some(opt.option_id.clone()));
                }
            }
            if ui.add(Button::new("Cancel").ghost().small()).clicked() {
                chosen = Some(None);
            }
        });
    });
    chosen
}

fn tool_kind_icon(kind: ToolKind) -> BuiltinIcon {
    match kind {
        ToolKind::Execute => BuiltinIcon::Terminal,
        ToolKind::Search => BuiltinIcon::Search,
        ToolKind::Edit | ToolKind::Move => BuiltinIcon::Code,
        ToolKind::Read => BuiltinIcon::Text,
        ToolKind::Delete => BuiltinIcon::Trash,
        ToolKind::SwitchMode => BuiltinIcon::Sliders,
        _ => BuiltinIcon::Box,
    }
}

fn compact_tool_label(call: &ToolCall) -> String {
    let title = call.title.trim();
    if let Some(first_loc) = call.locations.first()
        && let Some(file_name) = first_loc.path.file_name().and_then(|n| n.to_str())
        && !title.contains(file_name)
    {
        return format!("{title} · {file_name}");
    }
    title.to_owned()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let short: String = text.chars().take(max_chars).collect();
        format!("{short}\n… (truncated)")
    } else {
        text.to_owned()
    }
}
