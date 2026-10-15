//! Plugin-owned status contributions. Producers decide when their items exist.
use crate::CommandId;
use crate::workbench::{CommandInvocation, WorkbenchAction};
use bevy::prelude::*;
use dear_imgui_rs::{StyleColor, Ui};
use serde_json::Value;

#[derive(Component, Clone, Default)]
pub struct StatusItems(pub Vec<StatusItem>);

/// IDs must be unique within a publisher and should use its qualified namespace.
#[derive(Clone, Copy)]
pub struct StatusItem {
    pub id: &'static str,
    pub label: &'static str,
    /// Larger priorities retain inline space longer.
    pub priority: i32,
    /// Required icon key; the renderer places it before the measured content.
    pub icon: &'static str,
    pub prefer_icon: bool,
    /// Read-only content width, excluding the icon; None withdraws the item.
    pub measure: fn(&World, &Ui, StatusContext) -> Option<f32>,
    pub draw: fn(&mut World, &Ui, StatusDrawContext, &mut Vec<WorkbenchAction>),
    pub tooltip: fn(&World, &Ui, StatusContext),
}

#[derive(Clone, Copy, Debug)]
pub struct StatusContext {
    pub publisher: Entity,
    pub item_id: &'static str,
    pub focused_panel: Option<Entity>,
    pub focused_frontend: Option<Entity>,
    /// Panel-backed publishers retain their frontend source; ambient sources are None.
    pub source: Option<Entity>,
    pub area: u32,
}

impl StatusContext {
    pub fn is_focused(self) -> bool {
        self.focused_frontend == Some(self.publisher) || self.focused_panel == Some(self.publisher)
    }

    pub fn invoke(self, command: impl Into<CommandId>, arguments: Value) -> WorkbenchAction {
        WorkbenchAction::Invoke(CommandInvocation {
            command: command.into(),
            source: self.source,
            area: self.area,
            arguments,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusDrawMode {
    Inline,
    Popup,
}

#[derive(Clone, Copy, Debug)]
pub struct StatusDrawContext {
    pub context: StatusContext,
    pub mode: StatusDrawMode,
    pub width: f32,
}

/// A small default presentation for the common text-and-commands case.
/// Custom callbacks may draw any widgets within their allocated bounds.
#[derive(Clone, Debug, Default)]
pub struct StatusText {
    pub summary: String,
    pub details: String,
    pub error: bool,
    pub commands: Vec<StatusCommand>,
}

#[derive(Clone, Debug)]
pub struct StatusCommand {
    pub label: &'static str,
    pub command: CommandId,
    pub arguments: Value,
    pub enabled: bool,
}

impl StatusText {
    pub fn new(summary: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: details.into(),
            ..Default::default()
        }
    }

    pub fn command(mut self, label: &'static str, command: impl Into<CommandId>) -> Self {
        self.commands.push(StatusCommand {
            label,
            command: command.into(),
            arguments: Value::Null,
            enabled: true,
        });
        self
    }

    pub fn measure(&self, ui: &Ui) -> f32 {
        ui.calc_text_size(
            self.summary
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )[0]
        .max(1.0)
    }

    pub fn draw(&self, ui: &Ui, ctx: StatusDrawContext, actions: &mut Vec<WorkbenchAction>) {
        if ctx.mode == StatusDrawMode::Inline {
            let point = ui.cursor_screen_pos();
            let clicked =
                ui.invisible_button("summary", [ctx.width.max(1.0), ui.text_line_height()]);
            let text = self
                .summary
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            ui.get_window_draw_list().add_text(
                point,
                ui.style_color(if self.error {
                    StyleColor::PlotHistogram
                } else {
                    StyleColor::TextDisabled
                }),
                text,
            );
            if clicked
                && let Some(command) = self.commands.first().filter(|command| command.enabled)
            {
                actions.push(
                    ctx.context
                        .invoke(&command.command, command.arguments.clone()),
                );
            }
        } else {
            self.tooltip(ui);
            for (index, command) in self.commands.iter().enumerate() {
                let _id = ui.push_id(index as i32);
                let _disabled = ui.begin_disabled_with_cond(!command.enabled);
                if ui.button(command.label) {
                    actions.push(
                        ctx.context
                            .invoke(&command.command, command.arguments.clone()),
                    );
                }
            }
        }
    }

    pub fn tooltip(&self, ui: &Ui) {
        // The renderer supplies a wrap width for tooltips and widget popups.
        ui.text(&self.summary);
        if !self.details.is_empty() && self.details != self.summary {
            ui.text(&self.details);
        }
    }
}
/// Optional permanent identity displayed beside plugin status contributions.
#[derive(bevy::prelude::Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBarIdentity {
    pub icon: String,
    pub summary: String,
    pub details: String,
}
