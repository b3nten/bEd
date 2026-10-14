//! bEd menu arrangement. The shared core resolves commands and native shortcuts.
use crate::config::LayoutSlot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewWindow,
    NewDocument,
    NewTerminal,
    NewExplorer,
    NewSettings,
    NewWorkspaces,
    NewDiagnostics,
    Debug,
    NewStructure,
    NewReferences,
    NewLspDashboard,
    NewContentSearch,
    DuplicateView,
    SplitRight,
    SplitDown,
    SetCurrentLayout,
    ResetLayout,
    UseLayout(LayoutSlot),
    SetLayoutDefault(LayoutSlot),
    RestoreBuiltinLayout(LayoutSlot),
    SaveDefaultLayout,
    ResetDefaultLayout,
    Workspaces,
    Diagnostics,
    Structure,
    OpenFolder,
    OpenFile,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    GoToLine,
    Explorer,
    Terminal,
    Settings,
    LspDashboard,
    FindFile,
    FindWorkspace,
}

#[derive(Clone, Copy, Debug)]
pub struct MenuDispatch {
    pub action: MenuAction,
    pub keyboard: bool,
    pub shift: bool,
}

impl MenuAction {
    pub fn command_id(self) -> String {
        use MenuAction::*;
        match self {
            UseLayout(slot) => format!("bed.app.layout_{slot}"),
            SetLayoutDefault(slot) => format!("bed.app.set_layout_{slot}"),
            RestoreBuiltinLayout(slot) => format!("bed.app.restore_layout_{slot}"),
            NewWindow => "bed.desktop.new_window".into(),
            NewDocument => "bedtime.documents.new".into(),
            DuplicateView => "bedtime.workbench.duplicate_view".into(),
            Close => "bedtime.core.close".into(),
            Quit => "bedtime.core.quit".into(),
            OpenFolder => "bedtime.workspaces.lifecycle.action".into(),
            NewContentSearch => "bedtime.search.new".into(),
            NewDiagnostics => "bedtime.editor.new_diagnostics".into(),
            NewReferences => "bedtime.editor.new_references".into(),
            NewLspDashboard => "bedtime.editor.new_language_servers".into(),
            NewStructure | Structure => "bedtime.structure.open".into(),
            OpenFile => "bedtime.documents.open_file".into(),
            Save => "bedtime.documents.save".into(),
            SaveAs => "bedtime.documents.save_as".into(),
            SplitRight => "bedtime.workbench.split_right".into(),
            SplitDown => "bedtime.workbench.split_down".into(),
            NewWorkspaces | Workspaces => "bedtime.workspaces.new".into(),
            Settings => "bedtime.settings.show".into(),
            NewSettings => "bedtime.settings.new".into(),
            Terminal => "bedtime.terminal.show".into(),
            NewTerminal => "bedtime.terminal.new".into(),
            Explorer => "bedtime.explorer.show".into(),
            NewExplorer => "bedtime.explorer.new".into(),
            FindFile => "bedtime.explorer.find_file".into(),
            FindWorkspace => "bedtime.search.show".into(),
            Diagnostics => "bedtime.editor.show_diagnostics".into(),
            Debug => "bedtime.debug.show".into(),
            Undo => "bedtime.edit.undo".into(),
            Redo => "bedtime.edit.redo".into(),
            Cut => "bedtime.edit.cut".into(),
            Copy => "bedtime.edit.copy".into(),
            Paste => "bedtime.edit.paste".into(),
            SelectAll => "bedtime.edit.select_all".into(),
            Find => "bedtime.edit.find".into(),
            GoToLine => "bedtime.edit.go_to_line".into(),
            _ => format!("bed.app.{self:?}"),
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        use MenuAction::*;
        for (prefix, action) in [
            ("bed.app.layout_", UseLayout as fn(LayoutSlot) -> Self),
            ("bed.app.set_layout_", SetLayoutDefault),
            ("bed.app.restore_layout_", RestoreBuiltinLayout),
        ] {
            if let Some(slot) = id.strip_prefix(prefix) {
                return LayoutSlot::from_key(slot).map(action);
            }
        }
        [
            NewWindow,
            NewDocument,
            NewTerminal,
            NewExplorer,
            NewSettings,
            NewWorkspaces,
            NewDiagnostics,
            Debug,
            NewStructure,
            NewReferences,
            NewLspDashboard,
            NewContentSearch,
            DuplicateView,
            SplitRight,
            SplitDown,
            SetCurrentLayout,
            ResetLayout,
            SaveDefaultLayout,
            ResetDefaultLayout,
            Workspaces,
            Diagnostics,
            Structure,
            OpenFolder,
            OpenFile,
            Save,
            SaveAs,
            Close,
            Quit,
            Undo,
            Redo,
            Cut,
            Copy,
            Paste,
            SelectAll,
            Find,
            GoToLine,
            Explorer,
            Terminal,
            Settings,
            LspDashboard,
            FindFile,
            FindWorkspace,
        ]
        .into_iter()
        .find(|action| action.command_id() == id)
    }
}

/// bEd chooses menu arrangement; command owners publish availability and effective shortcuts.
pub fn layout(active_layout: LayoutSlot) -> bedtime_api::menus::MenuLayout {
    use MenuAction as A;
    use bedtime_api::menus::{Menu, MenuCommand, MenuEntry as E, MenuLayout};
    let item = |action: A, label: &str| {
        let mut command = MenuCommand::new(action.command_id()).labeled(label);
        if action == A::OpenFolder {
            command.arguments = serde_json::json!({"action":"open_folder"});
        }
        E::Command(command)
    };
    let mapped = |items: &[(A, &str)]| {
        items
            .iter()
            .map(|(action, label)| item(*action, label))
            .collect::<Vec<_>>()
    };
    let application = vec![
        E::About {
            label: "About bEd".into(),
            name: "bEd".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            comments: "Desktop text editor with embeddable document views.".into(),
            credits: include_str!("../../NOTICE").into(),
        },
        E::Separator,
        item(A::Settings, "Settings…"),
        E::Separator,
        E::Services,
        E::Separator,
        E::Hide("Hide bEd".into()),
        E::HideOthers,
        E::ShowAll,
        E::Separator,
        E::Quit(MenuCommand::new("bedtime.core.quit").labeled("Quit bEd")),
    ];
    let mut file = mapped(&[
        (A::NewWindow, "New Window"),
        (A::NewDocument, "New Document"),
        (A::OpenFolder, "Open Folder…"),
        (A::OpenFile, "Open File…"),
        (A::Save, "Save"),
        (A::SaveAs, "Save As…"),
        (A::Close, "Close Panel"),
    ]);
    file.push(E::Separator);
    file.extend(mapped(&[
        (A::DuplicateView, "New View of Document"),
        (A::SplitRight, "Split Right"),
        (A::SplitDown, "Split Down"),
    ]));
    let edit = mapped(&[
        (A::Undo, "Undo"),
        (A::Redo, "Redo"),
        (A::Cut, "Cut"),
        (A::Copy, "Copy"),
        (A::Paste, "Paste"),
        (A::SelectAll, "Select All"),
        (A::Find, "Find…"),
        (A::FindFile, "Find File…"),
        (A::FindWorkspace, "Find in Project…"),
        (A::GoToLine, "Go to Line…"),
    ]);
    let tools = vec![
        item(A::NewExplorer, "File Explorer"),
        item(A::NewTerminal, "Terminal"),
        item(A::NewSettings, "Settings"),
        item(A::NewWorkspaces, "Workspaces"),
        item(A::NewContentSearch, "Project Search"),
        item(A::NewDiagnostics, "Diagnostics"),
        item(A::NewReferences, "References"),
        item(A::NewLspDashboard, "Language Server Dashboard"),
        E::Separator,
        E::Contributions,
    ];
    let mut window = LayoutSlot::ALL
        .into_iter()
        .map(|slot| {
            let mut command =
                MenuCommand::new(A::UseLayout(slot).command_id()).labeled(slot.label());
            command.selected = slot == active_layout;
            E::Command(command)
        })
        .collect::<Vec<_>>();
    window.extend([
        item(A::SetCurrentLayout, "Set Current Layout"),
        item(A::ResetLayout, "Reset Current Layout"),
        E::Submenu(Menu::new(
            "Set Other Layout",
            LayoutSlot::ALL
                .into_iter()
                .map(|slot| item(A::SetLayoutDefault(slot), slot.label()))
                .collect(),
        )),
        E::Submenu(Menu::new(
            "Reset Other Layout",
            LayoutSlot::ALL
                .into_iter()
                .map(|slot| item(A::RestoreBuiltinLayout(slot), slot.label()))
                .collect(),
        )),
        E::Separator,
        E::Minimize,
        E::Maximize,
        E::Separator,
        E::BringAllToFront,
    ]);
    let mut window = Menu::new("Window", window);
    window.windows_menu = true;
    MenuLayout(vec![
        Menu::new("bEd", application),
        Menu::new("File", file),
        Menu::new("Edit", edit),
        Menu::new("Tools", tools),
        window,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::menus::MenuEntry;

    #[test]
    fn layout_menu_tracks_the_selected_slot_without_owning_commands() {
        for selected in [
            LayoutSlot::Startup,
            LayoutSlot::Layout1,
            LayoutSlot::Layout9,
        ] {
            let layout = layout(selected);
            let window = layout.0.iter().find(|menu| menu.label == "Window").unwrap();
            let selected_ids = window
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    MenuEntry::Command(command) if command.selected => Some(command.id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                selected_ids,
                vec![MenuAction::UseLayout(selected).command_id()]
            );
            assert!(window.windows_menu);
        }
    }

    #[test]
    fn folder_menu_preserves_the_apps_saved_workspace_policy() {
        let layout = layout(LayoutSlot::Layout1);
        let id = MenuAction::OpenFolder.command_id();
        assert_eq!(id, "bedtime.workspaces.lifecycle.action");
        assert_eq!(
            layout.arguments(&id).unwrap(),
            serde_json::json!({"action":"open_folder"})
        );
    }

    #[test]
    fn application_menu_keeps_branding_and_tools_accept_plugin_contributions() {
        let layout = layout(LayoutSlot::Layout1);
        let app = &layout.0[0];
        assert_eq!(app.label, "bEd");
        assert!(
            app.entries
                .iter()
                .any(|entry| matches!(entry,MenuEntry::About {name,..} if name=="bEd"))
        );
        let tools = layout.0.iter().find(|menu| menu.label == "Tools").unwrap();
        assert!(
            tools
                .entries
                .iter()
                .any(|entry| matches!(entry, MenuEntry::Contributions))
        );
    }
}
