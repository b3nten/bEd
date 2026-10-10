//! Platform-independent command presentation for native menus and titlebars.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorZoomCommand {
    In,
    Out,
    Reset,
}

impl EditorZoomCommand {
    pub fn command_id(self) -> &'static str {
        match self {
            Self::In => "bed.editor.zoom_in",
            Self::Out => "bed.editor.zoom_out",
            Self::Reset => "bed.editor.reset_zoom",
        }
    }

    pub fn from_command_id(id: &str) -> Option<Self> {
        match id {
            "bed.editor.zoom_in" => Some(Self::In),
            "bed.editor.zoom_out" => Some(Self::Out),
            "bed.editor.reset_zoom" => Some(Self::Reset),
            _ => None,
        }
    }
}

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
        Self::Search,
        Self::Debug,
        Self::Diagnostics,
        Self::Terminal,
        Self::Sidebar,
        Self::Structure,
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
            ("New Project Search", "search"),
            ("Debug", "debug"),
            ("New Diagnostics", "diagnostics"),
            ("New Terminal", "terminal"),
            ("New File Explorer", "files"),
            ("New Structure", "structure"),
            ("Split Right", "split_right"),
            ("Split Down", "split_down"),
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
