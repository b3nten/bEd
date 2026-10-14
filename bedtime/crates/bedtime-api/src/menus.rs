//! Application menu arrangement refers to commands owned by the installed plugins.
use bevy::prelude::*;
use serde_json::Value;

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct MenuLayout(pub Vec<Menu>);

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub label: String,
    pub entries: Vec<MenuEntry>,
    pub windows_menu: bool,
}
impl Menu {
    pub fn new(label: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            entries,
            windows_menu: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuCommand {
    pub id: String,
    pub label: Option<String>,
    pub arguments: Value,
    pub selected: bool,
}
impl MenuCommand {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            arguments: Value::Null,
            selected: false,
        }
    }
    pub fn labeled(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn with_arguments(mut self, arguments: Value) -> Self {
        self.arguments = arguments;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Command(MenuCommand),
    Quit(MenuCommand),
    Submenu(Menu),
    /// Include application-menu contributions from the installed plugins.
    Contributions,
    Separator,
    About {
        label: String,
        name: String,
        version: String,
        comments: String,
        credits: String,
    },
    Services,
    Hide(String),
    HideOthers,
    ShowAll,
    Minimize,
    Maximize,
    BringAllToFront,
}

impl MenuLayout {
    /// Native menus activate by command ID, so every reference to a command must
    /// supply the same arguments. Captions and selection may vary by location.
    pub fn validate(&self) -> Result<(), String> {
        fn visit<'a>(
            entries: &'a [MenuEntry],
            arguments: &mut std::collections::HashMap<&'a str, &'a Value>,
        ) -> Result<(), String> {
            for entry in entries {
                match entry {
                    MenuEntry::Command(command) | MenuEntry::Quit(command) => {
                        if let Some(previous) = arguments.insert(&command.id, &command.arguments)
                            && previous != &command.arguments
                        {
                            return Err(format!(
                                "Menu command '{}' has conflicting argument payloads",
                                command.id
                            ));
                        }
                    }
                    MenuEntry::Submenu(menu) => visit(&menu.entries, arguments)?,
                    _ => {}
                }
            }
            Ok(())
        }
        let mut arguments = std::collections::HashMap::new();
        for menu in &self.0 {
            visit(&menu.entries, &mut arguments)?;
        }
        Ok(())
    }

    /// Native activation identifies the command; preserve the application's payload.
    pub fn arguments(&self, id: &str) -> Result<Value, String> {
        self.validate()?;
        fn find(entries: &[MenuEntry], id: &str) -> Option<Value> {
            entries.iter().find_map(|entry| match entry {
                MenuEntry::Command(command) | MenuEntry::Quit(command) if command.id == id => {
                    Some(command.arguments.clone())
                }
                MenuEntry::Submenu(menu) => find(&menu.entries, id),
                _ => None,
            })
        }
        Ok(self
            .0
            .iter()
            .find_map(|menu| find(&menu.entries, id))
            .unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repeated_commands_share_payloads_across_menus_and_submenus() {
        let entry = MenuCommand::new("example.open").with_arguments(json!({"target":"database"}));
        let mut layout = MenuLayout(vec![
            Menu::new("File", vec![MenuEntry::Command(entry.clone())]),
            Menu::new(
                "Tools",
                vec![MenuEntry::Submenu(Menu::new(
                    "Database",
                    vec![MenuEntry::Command(entry.labeled("Reconnect"))],
                ))],
            ),
        ]);
        assert!(layout.validate().is_ok());
        assert_eq!(
            layout.arguments("example.open").unwrap(),
            json!({"target":"database"})
        );
        layout.0[1].entries.push(MenuEntry::Command(
            MenuCommand::new("example.open").with_arguments(json!({"target":"other"})),
        ));
        assert_eq!(
            layout.validate().unwrap_err(),
            "Menu command 'example.open' has conflicting argument payloads"
        );
        assert!(layout.arguments("example.open").is_err());
    }
}
