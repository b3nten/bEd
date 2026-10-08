//! Platform-independent command presentation for native menus and titlebars.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarAction {
    Sidebar,
    Terminal,
    Settings,
    Search,
    Diagnostics,
    Debug,
    Structure,
    SplitRight,
    SplitDown,
}

impl TitlebarAction {
    pub fn command_id(self) -> &'static str {
        match self {
            Self::Sidebar => "bed.files.new",
            Self::Terminal => "bed.terminal.new",
            Self::Settings => "bed.settings.new",
            Self::Search => "bed.search.new",
            Self::Diagnostics => "bed.diagnostics.new",
            Self::Debug => "bed.debug.show",
            Self::Structure => "bed.structure.new",
            Self::SplitRight => "bed.editor.split_right",
            Self::SplitDown => "bed.editor.split_down",
        }
    }

    pub fn from_command_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.command_id() == id)
    }

    pub const ALL: [Self; 9] = [
        Self::Sidebar,
        Self::Terminal,
        Self::Search,
        Self::Structure,
        Self::Diagnostics,
        Self::Debug,
        Self::SplitRight,
        Self::SplitDown,
        Self::Settings,
    ];
}

/// Legacy callers use the same descriptor path as contributed controls.
pub fn core_toolbar_commands() -> Vec<CommandItem> {
    TitlebarAction::ALL
        .into_iter()
        .zip([
            ("New File Explorer", "files"),
            ("New Terminal", "terminal"),
            ("New Project Search", "search"),
            ("New Structure", "structure"),
            ("New Diagnostics", "diagnostics"),
            ("Debug", "debug"),
            ("Split Editor Right", "split_right"),
            ("Split Editor Down", "split_down"),
            ("New Settings", "gear"),
        ])
        .map(|(action, (label, icon))| CommandItem {
            id: action.command_id().into(),
            label: label.into(),
            icon: Some(icon.into()),
            enabled: true,
        })
        .collect()
}
