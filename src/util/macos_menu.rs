//! Standalone Bed's native application menu. Embedded hosts own their menu bar.
//! This is the explicitly requested macOS infrastructure addition, not an
//! upstream editor algorithm replacement.
use crate::util::settings::Settings;
use dear_imgui_rs::Key;
use muda::{
    AboutMetadata, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
use std::{cell::RefCell, io, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewDocument,
    NewTerminal,
    NewExplorer,
    NewSettings,
    NewProjects,
    NewDiagnostics,
    NewReferences,
    NewLspDashboard,
    NewContentSearch,
    DuplicateView,
    SplitRight,
    SplitDown,
    ResetLayout,
    Projects,
    Diagnostics,
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
struct MenuTargetState {
    original_target: Retained<AnyObject>,
    original_selector: Sel,
    action: MenuAction,
    pending: Rc<RefCell<Vec<MenuDispatch>>>,
}
define_class!(
    // SAFETY: This main-thread NSObject forwards a menu item's existing target
    // and action; its only added behavior records owned event origin metadata.
    #[unsafe(super = NSObject)]
    #[name = "BedMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuTargetState]
    struct MenuTarget;
    unsafe impl NSObjectProtocol for MenuTarget {}
    impl MenuTarget {
        #[unsafe(method(performMenuAction:))]
        fn perform(&self, sender: Option<&AnyObject>) {
            let app = NSApplication::sharedApplication(self.mtm());
            let event = app.currentEvent();
            let keyboard = event.as_ref().is_some_and(|event| event.r#type() == NSEventType::KeyDown);
            let shift = event.as_ref().is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Shift));
            self.ivars().pending.borrow_mut().push(MenuDispatch { action: self.ivars().action, keyboard, shift });
            // SAFETY: The retained original target and selector were read from
            // this very NSMenuItem, and AppKit supplies its original sender.
            unsafe { app.sendAction_to_from(self.ivars().original_selector, Some(&self.ivars().original_target), sender); }
        }
    }
);

pub struct MacOsMenu {
    menu: Menu,
    items: Vec<(MenuAction, MenuItem, Option<Accelerator>)>,
    previous: Option<Retained<NSMenu>>,
    targets: Vec<(Retained<NSMenuItem>, Retained<MenuTarget>)>,
    pending: Rc<RefCell<Vec<MenuDispatch>>>,
}
impl MacOsMenu {
    pub fn install(settings: &Settings) -> io::Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("native menus require AppKit's main thread"))?;
        let application = NSApplication::sharedApplication(mtm);
        let previous = application.mainMenu();
        let menu = Menu::new();
        let app = Submenu::new("Bed", true);
        let file = Submenu::new("File", true);
        let edit = Submenu::new("Edit", true);
        let view = Submenu::new("View", true);
        let window = Submenu::new("Window", true);
        let mut items = Vec::new();
        let mut add = |parent: &Submenu, action: MenuAction, title: &str| -> io::Result<()> {
            let accelerator = accelerator(action, settings);
            let item = MenuItem::with_id(format!("bed.{action:?}"), title, true, accelerator);
            parent.append(&item).map_err(io::Error::other)?;
            items.push((action, item, accelerator));
            Ok(())
        };
        app.append(&PredefinedMenuItem::about(
            Some("About Bed"),
            Some(AboutMetadata {
                name: Some("Bed".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                comments: Some("Desktop text editor with embeddable document views.".into()),
                credits: Some(include_str!("../../NOTICE").into()),
                ..Default::default()
            }),
        ))
        .map_err(io::Error::other)?;
        app.append(&PredefinedMenuItem::separator())
            .map_err(io::Error::other)?;
        add(&app, MenuAction::Settings, "Settings…")?;
        app.append(&PredefinedMenuItem::separator())
            .map_err(io::Error::other)?;
        app.append_items(&[
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some("Hide Bed")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
        ])
        .map_err(io::Error::other)?;
        add(&app, MenuAction::Quit, "Quit Bed")?;
        for (action, title) in [
            (MenuAction::NewDocument, "New Document"),
            (MenuAction::OpenFolder, "Open Folder…"),
            (MenuAction::OpenFile, "Open File…"),
            (MenuAction::Save, "Save"),
            (MenuAction::SaveAs, "Save As…"),
            (MenuAction::Close, "Close Panel"),
        ] {
            add(&file, action, title)?;
        }
        for (action, title) in [
            (MenuAction::Undo, "Undo"),
            (MenuAction::Redo, "Redo"),
            (MenuAction::Cut, "Cut"),
            (MenuAction::Copy, "Copy"),
            (MenuAction::Paste, "Paste"),
            (MenuAction::SelectAll, "Select All"),
            (MenuAction::Find, "Find…"),
            (MenuAction::GoToLine, "Go to Line…"),
        ] {
            add(&edit, action, title)?;
        }
        add(&view, MenuAction::Explorer, "File Explorer")?;
        add(&view, MenuAction::Terminal, "Terminal")?;
        add(&view, MenuAction::FindFile, "Find File…")?;
        add(&view, MenuAction::FindProject, "Find in Project…")?;
        add(&view, MenuAction::LspDashboard, "Language Server Dashboard")?;
        add(&view, MenuAction::Projects, "Projects")?;
        add(&view, MenuAction::Diagnostics, "Diagnostics")?;
        for (action, title) in [
            (MenuAction::NewExplorer, "New File Explorer"),
            (MenuAction::NewTerminal, "New Terminal"),
            (MenuAction::NewSettings, "New Settings"),
            (MenuAction::NewProjects, "New Projects"),
            (MenuAction::NewContentSearch, "New Project Search"),
            (MenuAction::NewDiagnostics, "New Diagnostics"),
            (MenuAction::NewReferences, "New References"),
            (MenuAction::NewLspDashboard, "New Language Server Dashboard"),
            (MenuAction::DuplicateView, "New View of Document"),
            (MenuAction::SplitRight, "Split Right"),
            (MenuAction::SplitDown, "Split Down"),
            (MenuAction::ResetLayout, "Reset Layout"),
        ] {
            add(&window, action, title)?;
        }
        window
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ])
            .map_err(io::Error::other)?;
        menu.append_items(&[&app, &file, &edit, &view, &window])
            .map_err(io::Error::other)?;
        window.set_as_windows_menu_for_nsapp();
        menu.init_for_nsapp();
        let mut this = Self {
            menu,
            items,
            previous,
            targets: Vec::new(),
            pending: Rc::new(RefCell::new(Vec::new())),
        };
        if !this.is_installed() {
            return Err(io::Error::other("native menu installation failed"));
        }
        this.attach_native_targets(mtm)?;
        Ok(this)
    }
    pub fn poll(&self) -> Vec<MenuDispatch> {
        MenuEvent::receiver()
            .try_iter()
            .filter_map(|event| {
                self.items
                    .iter()
                    .find(|(_, item, _)| event.id == item.id())
                    .map(|(action, _, _)| *action)
            })
            .map(|action| {
                let mut pending = self.pending.borrow_mut();
                pending
                    .iter()
                    .position(|dispatch| dispatch.action == action)
                    .map(|index| pending.remove(index))
                    .unwrap_or(MenuDispatch {
                        action,
                        keyboard: false,
                        shift: false,
                    })
            })
            .collect()
    }
    fn attach_native_targets(&mut self, mtm: MainThreadMarker) -> io::Result<()> {
        // SAFETY: Muda retains this live root menu for this adapter's lifetime.
        let menu = unsafe { self.menu.ns_menu().cast::<NSMenu>().as_ref() }
            .ok_or_else(|| io::Error::other("native menu is missing"))?;
        for root_item in menu.itemArray().iter() {
            let Some(submenu) = root_item.submenu() else {
                continue;
            };
            for item in submenu.itemArray().iter() {
                let title = item.title().to_string();
                let action = self
                    .items
                    .iter()
                    .find(|(_, value, _)| value.text() == title)
                    .map(|(action, _, _)| *action);
                let Some(action) = action else {
                    continue;
                };
                let target = MenuTarget::alloc(mtm).set_ivars(MenuTargetState {
                    original_target: item
                        .target()
                        .ok_or_else(|| io::Error::other("native menu item target is missing"))?,
                    original_selector: item
                        .action()
                        .ok_or_else(|| io::Error::other("native menu item action is missing"))?,
                    action,
                    pending: Rc::clone(&self.pending),
                });
                let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
                // SAFETY: The retained target implements this exact selector.
                unsafe {
                    item.setTarget(Some(&target));
                    item.setAction(Some(sel!(performMenuAction:)));
                }
                self.targets.push((item, target));
            }
        }
        Ok(())
    }
    pub fn update(
        &mut self,
        settings: &Settings,
        has_document: bool,
        terminal_focused: bool,
        text_input_focused: bool,
    ) -> io::Result<()> {
        for (action, item, previous) in &mut self.items {
            // The source terminal uses Ctrl+Shift+C/V for clipboard actions;
            // plain Ctrl+C/V keep their raw terminal-control behavior.
            let value =
                if terminal_focused && matches!(action, MenuAction::Copy | MenuAction::Paste) {
                    Some(Accelerator::new(
                        Modifiers::META | Modifiers::SHIFT,
                        if *action == MenuAction::Copy {
                            Code::KeyC
                        } else {
                            Code::KeyV
                        },
                    ))
                } else {
                    accelerator(*action, settings)
                };
            if value != *previous {
                item.set_accelerator(value).map_err(io::Error::other)?;
                *previous = value;
            }
            if matches!(action, MenuAction::Undo | MenuAction::Redo) {
                item.set_enabled(!terminal_focused && (has_document || text_input_focused));
            }
            if matches!(action, MenuAction::Cut | MenuAction::SelectAll) {
                item.set_enabled(!terminal_focused);
            }
        }
        Ok(())
    }
    pub fn is_installed(&self) -> bool {
        MainThreadMarker::new().is_some_and(|mtm| {
            NSApplication::sharedApplication(mtm)
                .mainMenu()
                .is_some_and(|menu| {
                    Retained::as_ptr(&menu).cast::<std::ffi::c_void>() == self.menu.ns_menu()
                })
        })
    }
    /// Native NSMenu target/action dispatch used by the standalone smoke fixture.
    pub fn perform_for_smoke(&self, action: MenuAction) -> io::Result<()> {
        // SAFETY: Muda retains this live root NSMenu for the adapter's lifetime.
        let menu = unsafe { self.menu.ns_menu().cast::<NSMenu>().as_ref() }
            .ok_or_else(|| io::Error::other("native menu is missing"))?;
        let title = self
            .items
            .iter()
            .find(|(value, _, _)| *value == action)
            .map(|(_, item, _)| item.text())
            .ok_or_else(|| io::Error::other("native menu smoke item was not found"))?;
        for root_item in menu.itemArray().iter() {
            if let Some(submenu) = root_item.submenu() {
                for (index, item) in submenu.itemArray().iter().enumerate() {
                    if item.title().to_string() == title {
                        submenu.performActionForItemAtIndex(index as _);
                        return Ok(());
                    }
                }
            }
        }
        Err(io::Error::other("native menu smoke item was not found"))
    }
    /// Send a locally constructed event only to this application's own window.
    /// No OS input injection or interaction with another running Bed is used.
    pub fn key_equivalent_for_smoke(&self, characters: &str, key_code: u16) -> io::Result<()> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("native menu requires the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        let window = app
            .keyWindow()
            .ok_or_else(|| io::Error::other("native key fixture has no key window"))?;
        let characters = NSString::from_str(characters);
        for event_type in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                event_type, NSPoint::ZERO, NSEventModifierFlags::Command, 0.0,
                window.windowNumber(), None, &characters, &characters, false, key_code,
            ).ok_or_else(|| io::Error::other("AppKit could not create the native key event"))?;
            app.postEvent_atStart(&event, false);
        }
        Ok(())
    }
}
impl Drop for MacOsMenu {
    fn drop(&mut self) {
        for (item, target) in &self.targets {
            // SAFETY: Restore the same retained original targets and selectors
            // before our proxies or Muda's underlying items are dropped.
            unsafe {
                item.setTarget(Some(&target.ivars().original_target));
                item.setAction(Some(target.ivars().original_selector));
            }
        }
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            if self.is_installed() {
                app.setMainMenu(self.previous.as_deref());
            }
        }
    }
}

fn accelerator(action: MenuAction, settings: &Settings) -> Option<Accelerator> {
    let (key, shift) = input_shortcut(action, settings)?;
    let code = key_code(key)?;
    Some(Accelerator::new(
        Modifiers::META
            | if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::empty()
            },
        code,
    ))
}

/// Route consumed native key equivalents through the original global/focused
/// shortcut order; menu clicks still perform their single named action.
pub fn input_shortcut(action: MenuAction, settings: &Settings) -> Option<(Key, bool)> {
    use MenuAction::*;
    let (key, shift) = match action {
        NewDocument => (Key::N, false),
        NewTerminal => (Key::T, true),
        NewExplorer | NewSettings | NewProjects | NewDiagnostics | NewReferences
        | NewLspDashboard | NewContentSearch | DuplicateView | SplitRight | SplitDown
        | ResetLayout | Projects | Diagnostics => {
            return None;
        }
        OpenFolder => (Key::O, false),
        OpenFile | LspDashboard => return None,
        FindFile => (
            settings.keybinds.get_action_key("toggle_file_finder")?,
            false,
        ),
        FindProject => (Key::F, true),
        Save => (Key::S, false),
        SaveAs => (Key::S, true),
        Close => (Key::W, false),
        Quit => (Key::Q, false),
        Undo => (Key::Z, false),
        Redo => (Key::Z, true),
        Cut => (Key::X, false),
        Copy => (Key::C, false),
        Paste => (Key::V, false),
        SelectAll => (Key::A, false),
        Find => (Key::F, false),
        GoToLine => (settings.keybinds.get_action_key("line_jump_key")?, false),
        Explorer => (settings.keybinds.get_action_key("toggle_sidebar")?, false),
        Terminal => (
            settings
                .keybinds
                .get_action_key("toggle_terminal")
                .unwrap_or(Key::T),
            false,
        ),
        Settings => (
            settings.keybinds.get_action_key("toggle_settings_window")?,
            false,
        ),
    };
    Some((key, shift))
}
fn key_code(key: Key) -> Option<Code> {
    use Key::*;
    Some(match key {
        A => Code::KeyA,
        B => Code::KeyB,
        C => Code::KeyC,
        D => Code::KeyD,
        E => Code::KeyE,
        F => Code::KeyF,
        G => Code::KeyG,
        H => Code::KeyH,
        I => Code::KeyI,
        J => Code::KeyJ,
        K => Code::KeyK,
        L => Code::KeyL,
        M => Code::KeyM,
        N => Code::KeyN,
        O => Code::KeyO,
        P => Code::KeyP,
        Q => Code::KeyQ,
        R => Code::KeyR,
        S => Code::KeyS,
        T => Code::KeyT,
        U => Code::KeyU,
        V => Code::KeyV,
        W => Code::KeyW,
        X => Code::KeyX,
        Y => Code::KeyY,
        Z => Code::KeyZ,
        Semicolon => Code::Semicolon,
        Comma => Code::Comma,
        Period => Code::Period,
        Slash => Code::Slash,
        Backslash => Code::Backslash,
        Minus => Code::Minus,
        Equal => Code::Equal,
        Apostrophe => Code::Quote,
        GraveAccent => Code::Backquote,
        LeftBracket => Code::BracketLeft,
        RightBracket => Code::BracketRight,
        Enter => Code::Enter,
        Space => Code::Space,
        Tab => Code::Tab,
        Escape => Code::Escape,
        _ => return Option::None,
    })
}
