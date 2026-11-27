//! ACP client configuration and its settings UI.
use bedtime_persistence::StoreId;
use bedtime_ui::{
    BuiltinIcon, Button, Card, Size, Space, Text, TextField, TextRole, Tip, UiContext, draw, egui,
};
use bedtime_workbench::{
    BedtimeWorldExt as _,
    settings::{SettingsError, settings, ui::Marks},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One configured ACP agent executable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpClientConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Active resolved list of configured ACP clients.
#[derive(Resource, Default, Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredClients(pub Vec<AcpClientConfig>);

impl ConfiguredClients {
    pub fn find(&self, name: &str) -> Option<&AcpClientConfig> {
        self.0.iter().find(|client| client.name == name)
    }
}

settings! {
    "Agents" {
        /// External coding agents launched via the Agent Client Protocol.
        pub AcpClients: Vec<AcpClientConfig> {
            default: Vec::new(),
            key: "acp.clients",
            label: "ACP clients",
            description: "External coding agents launched via the Agent Client Protocol (e.g. claude, codex, agy).",
            ui: custom(clients_row),
            apply: |world, clients| {
                world.resource_mut::<ConfiguredClients>().0 = clients.clone();
            },
        }
    }
}

/// Draft state for adding a new client in the Settings UI.
#[derive(Clone, Debug, Default)]
struct NewClientDraft {
    name: String,
    command: String,
    args: String,
}

fn clients_row(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, layer: StoreId) {
    let settings = world.bedtime().settings();
    let mut clients = settings.get_at::<AcpClients>(layer);
    let set_here = settings.get_in::<AcpClients>(layer).is_some();
    let marks = Marks::new(
        settings,
        layer,
        settings.overridden_above::<AcpClients>(layer),
    );

    let mut changed = false;
    let mut reset = false;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = Space::Sm.px(ui);

        // Header row: label, overridden warning, reset button.
        ui.horizontal(|ui| {
            ui.add(Text::new("ACP clients").role(TextRole::Title));
            marks.overridden(ui, context);
            if set_here && marks.reset(ui) {
                reset = true;
            }
        });
        ui.add(
            Text::new(
                "Configure ACP agents (name, command, and optional arguments and environment). Each client is a group in the Agents panel.",
            )
            .muted(),
        );

        // Existing clients list.
        let mut remove_index = None;
        let layer_salt = settings.label(layer).unwrap_or("layer").to_owned();

        for (index, client) in clients.iter_mut().enumerate() {
            Card::new().show(ui, |ui| {
                    ui.set_max_width(580.0);
                    ui.horizontal_wrapped(|ui| {
                        let name_id = ui.id().with(("acp-name", &layer_salt, index));
                        if ui
                            .add(
                                TextField::new(&mut client.name)
                                    .id(name_id)
                                    .hint("Name")
                                    .label("Client name")
                                    .width(120.0),
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        let cmd_id = ui.id().with(("acp-cmd", &layer_salt, index));
                        if ui
                            .add(
                                TextField::new(&mut client.command)
                                    .id(cmd_id)
                                    .hint("Command (e.g. claude-code-acp)")
                                    .label("Command")
                                    .width(180.0),
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        // Edit arguments as a single space-separated string.
                        let args_id = ui.id().with(("acp-args", &layer_salt, index));
                        let mut args_text = ui
                            .data(|data| data.get_temp::<String>(args_id))
                            .unwrap_or_else(|| join_args(&client.args));
                        let args_resp = ui.add(
                            TextField::new(&mut args_text)
                                .id(args_id)
                                .hint("Args (optional)")
                                .label("Arguments")
                                .width(180.0),
                        );
                        if args_resp.changed() {
                            client.args = split_args(&args_text);
                            changed = true;
                        }
                        if args_resp.has_focus() {
                            ui.data_mut(|data| data.insert_temp(args_id, args_text));
                        } else {
                            ui.data_mut(|data| data.remove::<String>(args_id));
                        }

                        // Environment, as `NAME=value` words.
                        let env_id = ui.id().with(("acp-env", &layer_salt, index));
                        let mut env_text = ui
                            .data(|data| data.get_temp::<String>(env_id))
                            .unwrap_or_else(|| join_env(&client.env));
                        let env_resp = ui.add(
                            TextField::new(&mut env_text)
                                .id(env_id)
                                .hint("Env (NAME=value …)")
                                .label("Environment")
                                .width(180.0),
                        );
                        if env_resp.changed() {
                            client.env = split_env(&env_text);
                            changed = true;
                        }
                        if env_resp.has_focus() {
                            ui.data_mut(|data| data.insert_temp(env_id, env_text));
                        } else {
                            ui.data_mut(|data| data.remove::<String>(env_id));
                        }

                        let del = ui.add(
                            Button::icon_only(BuiltinIcon::Trash, "Remove client")
                                .ghost()
                                .small(),
                        );
                        (&del).tip("Remove client");
                        if del.clicked() {
                            remove_index = Some(index);
                        }
                    });
                });
        }

        if let Some(index) = remove_index {
            clients.remove(index);
            changed = true;
        }

        // Add new client row.
        let draft_id = ui.id().with(("acp-draft", &layer_salt));
        let mut draft = ui
            .data(|data| data.get_temp::<NewClientDraft>(draft_id))
            .unwrap_or_default();

        Card::new().show(ui, |ui| {
                ui.set_max_width(580.0);
                ui.horizontal(|ui| {
                    ui.add(
                        TextField::new(&mut draft.name)
                            .hint("Name (e.g. Claude)")
                            .label("New client name")
                            .width(120.0),
                    );
                    ui.add(
                        TextField::new(&mut draft.command)
                            .hint("Command")
                            .label("New client command")
                            .width(180.0),
                    );
                    ui.add(
                        TextField::new(&mut draft.args)
                            .hint("Args (optional)")
                            .label("New client arguments")
                            .width(180.0),
                    );
                    let can_add = !draft.name.trim().is_empty() && !draft.command.trim().is_empty();
                    let add_btn = ui.add_enabled(
                        can_add,
                        Button::new("Add")
                            .icon(BuiltinIcon::Plus)
                            .primary()
                            .size(Size::Sm),
                    );
                    if add_btn.clicked() {
                        clients.push(AcpClientConfig {
                            name: draft.name.trim().to_owned(),
                            command: draft.command.trim().to_owned(),
                            args: split_args(&draft.args),
                            env: BTreeMap::new(),
                        });
                        draft = NewClientDraft::default();
                        changed = true;
                    }
                });
            });

        ui.data_mut(|data| data.insert_temp(draft_id, draft));
    });

    if changed {
        draw::queue(ui, move |world: &mut World| {
            if let Err(error) = world
                .bedtime_mut()
                .settings_mut()
                .set::<AcpClients>(layer, clients)
            {
                world.write_message(SettingsError(format!(
                    "Couldn't update ACP clients: {error}"
                )));
            }
        });
    }
    if reset {
        draw::queue(ui, move |world: &mut World| {
            if let Err(error) = world
                .bedtime_mut()
                .settings_mut()
                .clear::<AcpClients>(layer)
            {
                world.write_message(SettingsError(format!(
                    "Couldn't reset ACP clients: {error}"
                )));
            }
        });
    }
}

/// Split an argument string into tokens, respecting single and double quotes.
pub fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Join arguments into a single string, quoting any argument with whitespace.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read `NAME=value` words (quoted like arguments) as environment variables.
/// A word with no `=`, or no name, is left out.
pub fn split_env(input: &str) -> BTreeMap<String, String> {
    split_args(input)
        .into_iter()
        .filter_map(|word| {
            let (name, value) = word.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Environment variables as `NAME=value` words, for `split_env`.
pub fn join_env(env: &BTreeMap<String, String>) -> String {
    let words: Vec<String> = env
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    join_args(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_join_args_roundtrip() {
        let args = split_args("--stdio --model \"claude sonnet\" 'foo bar'");
        assert_eq!(args, vec!["--stdio", "--model", "claude sonnet", "foo bar"]);
        let joined = join_args(&args);
        assert_eq!(split_args(&joined), args);
    }

    #[test]
    fn env_is_read_from_words_and_written_back() {
        let env = split_env("API_KEY=abc NAME=\"two words\" stray =nameless EMPTY=");
        let expected: BTreeMap<String, String> =
            [("API_KEY", "abc"), ("NAME", "two words"), ("EMPTY", "")]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
        assert_eq!(env, expected);
        assert_eq!(split_env(&join_env(&env)), env);
    }
}
