//! bEd menu policy. Native construction and activation live in bed-desktop.
use bed_desktop::{
    DesktopMenu, DesktopMenuCommand, DesktopMenuItem, DesktopModifiers, DesktopShortcut,
};
use bed_keybindings::{Bindings, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewWindow,
    NewDocument,
    NewTerminal,
    NewExplorer,
    NewSettings,
    NewProjects,
    NewDiagnostics,
    Debug,
    NewStructure,
    NewReferences,
    NewLspDashboard,
    NewContentSearch,
    DuplicateView,
    SplitRight,
    SplitDown,
    ResetLayout,
    SaveDefaultLayout,
    ResetDefaultLayout,
    Projects,
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
    FindProject,
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
            NewWindow => "bed.desktop.new_window".into(),
            Settings => "bed.settings.show".into(),
            NewSettings => "bed.settings.new".into(),
            Terminal => "bed.terminal.show".into(),
            NewTerminal => "bed.terminal.new".into(),
            Explorer => "bed.explorer.show".into(),
            NewExplorer => "bed.explorer.new".into(),
            FindFile => "bed.explorer.find_file".into(),
            FindProject => "bed.search.show".into(),
            Diagnostics => "bed.editor.show_diagnostics".into(),
            Debug => "bed.debug.show".into(),
            Undo => "bed.editor.undo".into(),
            Redo => "bed.editor.redo".into(),
            Cut => "bed.editor.cut".into(),
            Copy => "bed.editor.copy".into(),
            Paste => "bed.editor.paste".into(),
            SelectAll => "bed.editor.select_all".into(),
            Find => "bed.editor.find".into(),
            GoToLine => "bed.editor.go_to_line".into(),
            _ => format!("bed.app.{self:?}"),
        }
    }
    pub fn from_command(id: &str) -> Option<Self> {
        use MenuAction::*;
        [
            NewWindow,
            NewDocument,
            NewTerminal,
            NewExplorer,
            NewSettings,
            NewProjects,
            NewDiagnostics,
            Debug,
            NewStructure,
            NewReferences,
            NewLspDashboard,
            NewContentSearch,
            DuplicateView,
            SplitRight,
            SplitDown,
            ResetLayout,
            SaveDefaultLayout,
            ResetDefaultLayout,
            Projects,
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
            FindProject,
        ]
        .into_iter()
        .find(|action| action.command_id() == id)
    }
}
fn requires_workspace(action: MenuAction) -> bool {
    matches!(
        action,
        MenuAction::LspDashboard
            | MenuAction::NewLspDashboard
            | MenuAction::Debug
            | MenuAction::Diagnostics
            | MenuAction::NewDiagnostics
            | MenuAction::NewReferences
    )
}

/// Route consumed native key equivalents through the original global/focused
/// shortcut order; menu clicks still perform their single named action.
pub fn input_shortcut(action: MenuAction, bindings: &Bindings) -> Option<Stroke> {
    bindings
        .for_action(&action.command_id())
        .into_iter()
        .find(|binding| {
            bindings
                .winner(&binding.scope, binding.shortcut)
                .is_some_and(|winner| winner.action == binding.action)
        })
        .map(|binding| binding.shortcut)
}

/// Native accelerators must not reserve another panel's local keymap.
pub fn menu_shortcut(
    action: MenuAction,
    bindings: &Bindings,
    editor_focused: bool,
    text_input: bool,
) -> Option<Stroke> {
    use MenuAction::*;
    if text_input {
        let key = match action {
            Undo => Some("Primary+Z"),
            Redo => Some("Primary+Shift+Z"),
            Cut => Some("Primary+X"),
            Copy => Some("Primary+C"),
            Paste => Some("Primary+V"),
            SelectAll => Some("Primary+A"),
            _ => None,
        };
        if let Some(key) = key {
            return Some(key.parse::<Stroke>().unwrap().resolved(bindings.platform));
        }
    }
    let declaration = bindings
        .actions
        .iter()
        .find(|a| a.id == action.command_id())?;
    if declaration.scope != bed_keybindings::Scope::Global && !editor_focused {
        return None;
    }
    input_shortcut(action, bindings)
}

pub fn menus(
    bindings: &Bindings,
    workspace: bool,
    document: bool,
    clipboard: bool,
    text_input: bool,
    editor_focused: bool,
    commands: &[bed_editor_workbench::commands::CommandItem],
) -> Vec<DesktopMenu> {
    use DesktopMenuItem::*;
    use MenuAction as A;
    let command = |action: A, label: &str| {
        let enabled = (!requires_workspace(action) || workspace)
            && match action {
                A::Undo | A::Redo => text_input || (!clipboard && document),
                A::Cut => text_input || !clipboard,
                _ => true,
            };
        let item = DesktopMenuCommand {
            id: action.command_id(),
            label: label.into(),
            enabled,
            shortcut: menu_shortcut(action, bindings, editor_focused, text_input).map(|stroke| {
                DesktopShortcut {
                    key: stroke.key,
                    modifiers: DesktopModifiers {
                        ctrl: stroke.modifiers.ctrl,
                        super_key: stroke.modifiers.super_key,
                        alt: stroke.modifiers.alt,
                        shift: stroke.modifiers.shift,
                        ..Default::default()
                    },
                }
            }),
        };
        if action == A::Quit {
            Quit(item)
        } else {
            Command(item)
        }
    };
    let make = |label: &str, items: Vec<DesktopMenuItem>, windows_menu| DesktopMenu {
        label: label.into(),
        items,
        windows_menu,
    };
    let mapped = |items: &[(A, &str)]| {
        items
            .iter()
            .map(|(action, label)| command(*action, label))
            .collect::<Vec<_>>()
    };
    let app = vec![
        About {
            label: "About bEd".into(),
            name: "bEd".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            comments: "Desktop text editor with embeddable document views.".into(),
            credits: include_str!("../NOTICE").into(),
        },
        Separator,
        command(A::Settings, "Settings…"),
        Separator,
        Services,
        Separator,
        Hide("Hide bEd".into()),
        HideOthers,
        ShowAll,
        Separator,
        command(A::Quit, "Quit bEd"),
    ];
    let file = mapped(&[
        (A::NewWindow, "New Window"),
        (A::NewDocument, "New Document"),
        (A::OpenFolder, "Open Folder…"),
        (A::OpenFile, "Open File…"),
        (A::Save, "Save"),
        (A::SaveAs, "Save As…"),
        (A::Close, "Close Panel"),
    ]);
    let edit = mapped(&[
        (A::Undo, "Undo"),
        (A::Redo, "Redo"),
        (A::Cut, "Cut"),
        (A::Copy, "Copy"),
        (A::Paste, "Paste"),
        (A::SelectAll, "Select All"),
        (A::Find, "Find…"),
        (A::GoToLine, "Go to Line…"),
    ]);
    let view = mapped(&[
        (A::Explorer, "File Explorer"),
        (A::Terminal, "Terminal"),
        (A::FindFile, "Find File…"),
        (A::FindProject, "Find in Project…"),
        (A::LspDashboard, "Language Server Dashboard"),
        (A::Projects, "Startup"),
        (A::Diagnostics, "Diagnostics"),
        (A::Debug, "Debug"),
        (A::Structure, "Structure"),
    ]);
    let mut window = mapped(&[
        (A::NewExplorer, "New File Explorer"),
        (A::NewTerminal, "New Terminal"),
        (A::NewSettings, "New Settings"),
        (A::NewProjects, "New Startup"),
        (A::NewContentSearch, "New Project Search"),
        (A::NewDiagnostics, "New Diagnostics"),
        (A::NewStructure, "New Structure"),
        (A::NewReferences, "New References"),
        (A::NewLspDashboard, "New Language Server Dashboard"),
        (A::DuplicateView, "New View of Document"),
        (A::SplitRight, "Split Right"),
        (A::SplitDown, "Split Down"),
    ]);
    window.push(Separator);
    window.extend(mapped(&[
        (A::SaveDefaultLayout, "Save Default Layout"),
        (A::ResetDefaultLayout, "Reset Default Layout"),
        (A::ResetLayout, "Reset Layout"),
    ]));
    window.extend([Separator, Minimize, Maximize, Separator, BringAllToFront]);
    let tools = commands
        .iter()
        .map(|item| {
            Command(DesktopMenuCommand {
                id: item.id.clone(),
                label: item.label.clone(),
                enabled: item.enabled,
                shortcut: bindings
                    .for_action(&item.id)
                    .into_iter()
                    .find(|binding| {
                        (binding.scope == bed_keybindings::Scope::Global
                            || (editor_focused
                                && binding.scope
                                    == bed_keybindings::Scope::panel(
                                        bed_keybindings::EDITOR_SCOPE,
                                    )))
                            && bindings
                                .winner(&binding.scope, binding.shortcut)
                                .is_some_and(|winner| winner.action == item.id)
                    })
                    .map(|binding| DesktopShortcut {
                        key: binding.shortcut.key,
                        modifiers: DesktopModifiers {
                            ctrl: binding.shortcut.modifiers.ctrl,
                            shift: binding.shortcut.modifiers.shift,
                            alt: binding.shortcut.modifiers.alt,
                            super_key: binding.shortcut.modifiers.super_key,
                        },
                    }),
            })
        })
        .collect();
    vec![
        make("bEd", app, false),
        make("File", file, false),
        make("Edit", edit, false),
        make("View", view, false),
        make("Tools", tools, false),
        make("Window", window, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_keybindings::{Action, Overrides, Platform, Scope};
    #[test]
    fn native_accelerators_preserve_all_modifiers_and_skip_shadowed_or_cleared_bindings() {
        let save = Action::new(
            "bed.app.Save",
            "Save",
            "bed.app",
            Scope::Global,
            &["Ctrl+S", "Ctrl+Command+Alt+Shift+F12"],
        );
        let other = Action::new(
            "bed.app.ZOther",
            "Other",
            "bed.app",
            Scope::Global,
            &["Ctrl+S"],
        );
        let model = Bindings::new(
            vec![save.clone(), other.clone()],
            &Overrides::default(),
            Platform::Mac,
        );
        let stroke = input_shortcut(MenuAction::Save, &model).unwrap();
        assert_eq!(stroke.to_string(), "Ctrl+Command+Alt+Shift+F12");
        let editor_action = Action::new(
            "bed.editor.copy",
            "Copy",
            "bed.editor",
            Scope::panel(bed_keybindings::EDITOR_SCOPE),
            &["Alt+F12"],
        );
        let editor_map = Bindings::new(vec![editor_action], &Overrides::default(), Platform::Mac);
        assert!(
            menu_shortcut(MenuAction::Copy, &editor_map, false, false).is_none(),
            "another panel must keep its local keys"
        );
        assert_eq!(
            menu_shortcut(MenuAction::Copy, &editor_map, true, false)
                .unwrap()
                .to_string(),
            "Alt+F12"
        );
        assert_eq!(
            menu_shortcut(MenuAction::Copy, &editor_map, false, true)
                .unwrap()
                .to_string(),
            "Command+C",
            "native text fields keep their widget shortcuts"
        );
        let mut overrides = Overrides::default();
        overrides
            .replace(&save, Some(0), "Ctrl+S".parse().unwrap())
            .unwrap();
        assert_eq!(
            input_shortcut(
                MenuAction::Save,
                &Bindings::new(vec![save.clone(), other], &overrides, Platform::Mac)
            )
            .unwrap()
            .to_string(),
            "Ctrl+S"
        );
        overrides.actions.insert(save.id.clone(), Vec::new());
        assert!(
            input_shortcut(
                MenuAction::Save,
                &Bindings::new(vec![save], &overrides, Platform::Mac)
            )
            .is_none()
        );
    }
}
