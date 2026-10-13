//! Platform-independent command presentation for native menus and titlebars.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomCommand {
    In,
    Out,
    Reset,
}

impl ZoomCommand {
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
