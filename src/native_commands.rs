//! bEd menu policy. Native construction and activation live in bed-desktop.
use crate::config::LayoutSlot;
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
            OpenFile => "bed.documents.open_file".into(),
            Save => "bed.documents.save".into(),
            SaveAs => "bed.documents.save_as".into(),
            SplitRight => "bed.workbench.split_right".into(),
            SplitDown => "bed.workbench.split_down".into(),
            NewWorkspaces => "bed.app.NewProjects".into(),
            Workspaces => "bed.app.Projects".into(),
            Settings => "bed.settings.show".into(),
            NewSettings => "bed.settings.new".into(),
            Terminal => "bed.terminal.show".into(),
            NewTerminal => "bed.terminal.new".into(),
            Explorer => "bed.explorer.show".into(),
            NewExplorer => "bed.explorer.new".into(),
            FindFile => "bed.explorer.find_file".into(),
            FindWorkspace => "bed.search.show".into(),
            Diagnostics => "bed.editor.show_diagnostics".into(),
            Debug => "bed.debug.show".into(),
            Undo => "bed.edit.undo".into(),
            Redo => "bed.edit.redo".into(),
            Cut => "bed.edit.cut".into(),
            Copy => "bed.edit.copy".into(),
            Paste => "bed.edit.paste".into(),
            SelectAll => "bed.edit.select_all".into(),
            Find => "bed.edit.find".into(),
            GoToLine => "bed.editor.go_to_line".into(),
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

fn requires_directory(action: MenuAction) -> bool {
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
                .active_winner(&binding.scope, binding.shortcut)
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

pub struct MenuContext {
    pub directory_available: bool,
    pub document: bool,
    pub clipboard: bool,
    pub text_input: bool,
    pub editor_focused: bool,
    pub active_layout: LayoutSlot,
}

pub fn menus(
    bindings: &Bindings,
    commands: &[crate::application::commands::CommandItem],
    context: MenuContext,
) -> Vec<DesktopMenu> {
    let MenuContext {
        directory_available,
        document,
        clipboard,
        text_input,
        editor_focused,
        active_layout,
    } = context;
    use DesktopMenuItem::*;
    use MenuAction as A;
    let command = |action: A, label: &str| {
        let enabled = (!requires_directory(action) || directory_available)
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
    let mut file = mapped(&[
        (A::NewWindow, "New Window"),
        (A::NewDocument, "New Document"),
        (A::OpenFolder, "Open Folder…"),
        (A::OpenFile, "Open File…"),
        (A::Save, "Save"),
        (A::SaveAs, "Save As…"),
        (A::Close, "Close Panel"),
    ]);
    file.push(Separator);
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
    let mut window = Vec::new();
    for slot in LayoutSlot::ALL {
        let item = command(A::UseLayout(slot), slot.label());
        window.push(if slot == active_layout {
            match item {
                Command(command) => SelectedCommand(command),
                item => item,
            }
        } else {
            item
        });
    }
    window.push(command(A::SetCurrentLayout, "Set Current Layout"));
    window.push(command(A::ResetLayout, "Reset Current Layout"));
    window.push(Submenu {
        label: "Set Other Layout".into(),
        items: LayoutSlot::ALL
            .into_iter()
            .map(|slot| command(A::SetLayoutDefault(slot), slot.label()))
            .collect(),
    });
    window.push(Submenu {
        label: "Reset Other Layout".into(),
        items: LayoutSlot::ALL
            .into_iter()
            .map(|slot| command(A::RestoreBuiltinLayout(slot), slot.label()))
            .collect(),
    });
    window.extend([Separator, Minimize, Maximize, Separator, BringAllToFront]);
    let mut tools = mapped(&[
        (A::NewExplorer, "File Explorer"),
        (A::NewTerminal, "Terminal"),
        (A::NewSettings, "Settings"),
        #[cfg(feature = "workspaces")]
        (A::NewWorkspaces, "Workspaces"),
        (A::NewContentSearch, "Project Search"),
        (A::NewDiagnostics, "Diagnostics"),
        (A::NewReferences, "References"),
        (A::NewLspDashboard, "Language Server Dashboard"),
    ]);
    if !commands.is_empty() {
        tools.push(Separator);
    }
    tools.extend(
        commands
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
            .collect::<Vec<_>>(),
    );
    vec![
        make("bEd", app, false),
        make("File", file, false),
        make("Edit", edit, false),
        make("Tools", tools, false),
        make("Window", window, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_keybindings::{Overrides, Platform, Scope, ShortcutAction};

    #[test]
    fn stored_workspace_command_ids_resolve_to_workspace_actions() {
        for (id, action) in [
            ("bed.app.NewProjects", MenuAction::NewWorkspaces),
            ("bed.app.Projects", MenuAction::Workspaces),
            ("bed.search.show", MenuAction::FindWorkspace),
        ] {
            assert_eq!(MenuAction::from_command(id), Some(action));
            assert_eq!(action.command_id(), id);
        }
    }

    #[test]
    fn tools_own_panel_creation_and_window_owns_layouts() {
        let bindings = Bindings::new(Vec::new(), &Overrides::default(), Platform::Mac);
        let commands = vec![crate::application::commands::CommandItem {
            id: "bed.http.open".into(),
            label: "HTTP Client".into(),
            icon: None,
            enabled: true,
        }];
        let menus = menus(
            &bindings,
            &commands,
            MenuContext {
                directory_available: true,
                document: true,
                clipboard: false,
                text_input: false,
                editor_focused: true,
                active_layout: LayoutSlot::Layout1,
            },
        );
        assert!(!menus.iter().any(|menu| menu.label == "View"));
        let window = menus.iter().find(|menu| menu.label == "Window").unwrap();
        for item in &window.items {
            if let DesktopMenuItem::Command(command) | DesktopMenuItem::SelectedCommand(command) =
                item
            {
                assert!(matches!(
                    MenuAction::from_command(&command.id),
                    Some(
                        MenuAction::UseLayout(_)
                            | MenuAction::SetCurrentLayout
                            | MenuAction::ResetLayout
                    )
                ));
            }
        }
        let tools = menus.iter().find(|menu| menu.label == "Tools").unwrap();
        let ids = tools
            .items
            .iter()
            .filter_map(|item| match item {
                DesktopMenuItem::Command(command) => Some(command.id.as_str()),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>();
        assert!(ids.contains("bed.terminal.new"));
        assert!(ids.contains("bed.explorer.new"));
        assert!(ids.contains("bed.http.open"));
        assert!(!ids.contains("bed.app.NewStructure"));
        let file = menus.iter().find(|menu| menu.label == "File").unwrap();
        assert!(file.items.iter().any(|item| matches!(item, DesktopMenuItem::Command(command) if command.id == MenuAction::SplitRight.command_id())));
    }

    #[test]
    fn layout_menus_select_the_active_slot_and_keep_save_reset_unbound() {
        let actions = (1..=9)
            .map(|slot| {
                ShortcutAction::new(
                    &format!("bed.app.layout_{slot}"),
                    &format!("Switch to Layout {slot}"),
                    "bed.app",
                    Scope::Global,
                    &[&format!("Primary+{slot}")],
                )
            })
            .collect();
        let bindings = Bindings::new(actions, &Overrides::default(), Platform::Mac);
        let menus = menus(
            &bindings,
            &[],
            MenuContext {
                directory_available: true,
                document: true,
                clipboard: false,
                text_input: false,
                editor_focused: true,
                active_layout: LayoutSlot::Layout3,
            },
        );
        let window = menus.iter().find(|menu| menu.label == "Window").unwrap();
        let selected = window
            .items
            .iter()
            .filter_map(|item| match item {
                DesktopMenuItem::SelectedCommand(command) => Some(command),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "bed.app.layout_3");
        let labels = window
            .items
            .iter()
            .filter_map(|item| match item {
                DesktopMenuItem::Command(command) | DesktopMenuItem::SelectedCommand(command) => {
                    Some(command.label.as_str())
                }
                DesktopMenuItem::Submenu { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            &labels[..14],
            &[
                "Layout 1",
                "Layout 2",
                "Layout 3",
                "Layout 4",
                "Layout 5",
                "Layout 6",
                "Layout 7",
                "Layout 8",
                "Layout 9",
                "Startup",
                "Set Current Layout",
                "Reset Current Layout",
                "Set Other Layout",
                "Reset Other Layout",
            ]
        );
        for item in &window.items {
            if let DesktopMenuItem::Submenu { label, items } = item {
                assert!(matches!(
                    label.as_str(),
                    "Set Other Layout" | "Reset Other Layout"
                ));
                assert_eq!(items.len(), 10);
                assert!(
                    matches!(items.last().unwrap(), DesktopMenuItem::Command(command) if command.label == "Startup")
                );
                for item in items {
                    let DesktopMenuItem::Command(command) = item else {
                        panic!("expected a menu command")
                    };
                    assert!(command.shortcut.is_none());
                    assert!(MenuAction::from_command(&command.id).is_some());
                }
            }
        }
        assert!(MenuAction::from_command("bed.app.layout_0").is_none());
        assert!(MenuAction::from_command("bed.app.layout_10").is_none());
        for action in [
            MenuAction::UseLayout(LayoutSlot::Startup),
            MenuAction::SetLayoutDefault(LayoutSlot::Startup),
            MenuAction::RestoreBuiltinLayout(LayoutSlot::Startup),
            MenuAction::SetCurrentLayout,
        ] {
            assert_eq!(MenuAction::from_command(&action.command_id()), Some(action));
        }
        let startup_menus = self::menus(
            &bindings,
            &[],
            MenuContext {
                directory_available: true,
                document: false,
                clipboard: false,
                text_input: false,
                editor_focused: false,
                active_layout: LayoutSlot::Startup,
            },
        );
        let startup_window = startup_menus
            .iter()
            .find(|menu| menu.label == "Window")
            .unwrap();
        assert!(
            matches!(&startup_window.items[9], DesktopMenuItem::SelectedCommand(command) if command.label == "Startup" && command.shortcut.is_none())
        );
    }

    #[test]
    fn native_accelerators_preserve_all_modifiers_and_skip_shadowed_or_cleared_bindings() {
        let save = ShortcutAction::new(
            "bed.documents.save",
            "Save",
            "bed.documents",
            Scope::Global,
            &["Ctrl+S", "Ctrl+Command+Alt+Shift+F12"],
        );
        let other = ShortcutAction::new(
            "fixture.other",
            "Other",
            "fixture",
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
        let editor_action = ShortcutAction::new(
            "bed.edit.copy",
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
