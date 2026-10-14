//! AppKit menus preserve Winit's delegate and report application command IDs.
use super::{
    DesktopMenu, DesktopMenuCommand, DesktopMenuItem, DesktopNativeAction, DesktopShortcut,
};
use crate::DesktopModifiers;
use dear_imgui_rs::Key;
use muda::{
    AboutMetadata, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ClassBuilder, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSEvent, NSEventModifierFlags, NSEventType, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
use std::{cell::RefCell, io, rc::Rc};

struct MenuTargetState {
    original_target: Retained<AnyObject>,
    original_selector: Sel,
    command: String,
    quit: bool,
    pending: Rc<RefCell<Vec<DesktopNativeAction>>>,
}
define_class!(
    // SAFETY: This target forwards the item's original selector and records owned metadata.
    #[unsafe(super = NSObject)]
    #[name = "BedDesktopMenuTarget"]
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
            let flags = event.as_ref().map_or(NSEventModifierFlags::empty(), |event| event.modifierFlags());
            self.ivars().pending.borrow_mut().push(DesktopNativeAction {
                command: self.ivars().command.clone(), keyboard,
                modifiers: modifier_flags(flags), quit: self.ivars().quit,
            });
            // SAFETY: Target and selector were retained from this same NSMenuItem.
            unsafe { app.sendAction_to_from(self.ivars().original_selector, Some(&self.ivars().original_target), sender); }
        }
    }
);

struct DockActionsState {
    commands: Vec<DesktopMenuCommand>,
    pending: Rc<RefCell<Vec<DesktopNativeAction>>>,
}
define_class!(
    // SAFETY: AppKit invokes this retained target on its main thread.
    #[unsafe(super = NSObject)]
    #[name = "BedDesktopDockMenuActions"]
    #[thread_kind = MainThreadOnly]
    #[ivars = DockActionsState]
    struct DockMenuActions;

    unsafe impl NSObjectProtocol for DockMenuActions {}

    impl DockMenuActions {
        #[unsafe(method(performDockCommand:))]
        fn perform(&self, sender: Option<&NSMenuItem>) {
            let Some(sender) = sender else { return; };
            if !sender.isEnabled() { return; }
            let Some(command) = self.ivars().commands.get(sender.tag() as usize) else { return; };
            self.ivars().pending.borrow_mut().push(DesktopNativeAction {
                command: command.id.clone(), keyboard: false,
                modifiers: modifier_flags(NSEvent::modifierFlags_class()), quit: false,
            });
        }
    }
);
static DOCK_MENU_KEY: u8 = 0;
extern "C-unwind" fn application_dock_menu(
    delegate: &AnyObject,
    _selector: Sel,
    _application: &NSApplication,
) -> *mut NSMenu {
    // SAFETY: Installation associates a retained menu with this live delegate.
    unsafe {
        objc2::ffi::objc_getAssociatedObject(delegate, (&DOCK_MENU_KEY as *const u8).cast())
            .cast_mut()
            .cast()
    }
}

struct DockMenu {
    _actions: Retained<DockMenuActions>,
    delegate: Retained<ProtocolObject<dyn NSApplicationDelegate>>,
    original_class: &'static AnyClass,
    menu_class: &'static AnyClass,
}

impl DockMenu {
    fn install(
        application: &NSApplication,
        mtm: MainThreadMarker,
        commands: &[DesktopMenuCommand],
        pending: Rc<RefCell<Vec<DesktopNativeAction>>>,
    ) -> io::Result<Self> {
        let delegate = application
            .delegate()
            .ok_or_else(|| io::Error::other("native application delegate is missing"))?;
        let object: &AnyObject = delegate.as_ref();
        let original_class = object.class();
        // Keep Winit's delegate identity and ivars; add only an ivar-free subclass.
        let menu_class = if let Some(class) = AnyClass::get(c"BedDesktopDockMenuDelegate") {
            if class.superclass() != Some(original_class) {
                return Err(io::Error::other(
                    "native application delegate class changed",
                ));
            }
            class
        } else {
            let mut builder = ClassBuilder::new(c"BedDesktopDockMenuDelegate", original_class)
                .ok_or_else(|| io::Error::other("could not create Dock menu delegate subclass"))?;
            // SAFETY: Exact ABI of application's Dock menu callback.
            unsafe {
                builder.add_method(
                    sel!(applicationDockMenu:),
                    application_dock_menu as extern "C-unwind" fn(_, _, _) -> _,
                );
            }
            builder.register()
        };
        let menu = NSMenu::new(mtm);
        let actions = DockMenuActions::alloc(mtm).set_ivars(DockActionsState {
            commands: commands.to_vec(),
            pending,
        });
        // SAFETY: NSObject initialization adds no subclass requirements.
        let actions: Retained<DockMenuActions> = unsafe { msg_send![super(actions), init] };
        for (index, command) in commands.iter().enumerate() {
            // SAFETY: Retained target implements the selector below.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&command.label),
                    Some(sel!(performDockCommand:)),
                    &NSString::new(),
                )
            };
            unsafe {
                item.setTarget(Some(&actions));
            }
            item.setTag(index as isize);
            item.setEnabled(command.enabled);
            menu.addItem(&item);
        }
        // SAFETY: Subclass adds no ivars; associated object retains menu until teardown.
        unsafe {
            objc2::ffi::objc_setAssociatedObject(
                (object as *const AnyObject).cast_mut(),
                (&DOCK_MENU_KEY as *const u8).cast(),
                &*menu as *const NSMenu as *mut AnyObject,
                objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
            AnyObject::set_class(object, menu_class);
        }
        Ok(Self {
            _actions: actions,
            delegate,
            original_class,
            menu_class,
        })
    }
}

impl Drop for DockMenu {
    fn drop(&mut self) {
        let object: &AnyObject = self.delegate.as_ref();
        // SAFETY: Restore only our ivar-free subclass and association.
        unsafe {
            objc2::ffi::objc_setAssociatedObject(
                (object as *const AnyObject).cast_mut(),
                (&DOCK_MENU_KEY as *const u8).cast(),
                std::ptr::null_mut(),
                objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
            if object.class() == self.menu_class {
                AnyObject::set_class(object, self.original_class);
            }
        }
    }
}

pub struct MacOsMenu {
    menu: Menu,
    items: Vec<MenuItem>,
    definitions: Vec<DesktopMenu>,
    dock_definitions: Vec<DesktopMenuCommand>,
    previous: Option<Retained<NSMenu>>,
    targets: Vec<(Retained<NSMenuItem>, Retained<MenuTarget>)>,
    pending: Rc<RefCell<Vec<DesktopNativeAction>>>,
    dock: Option<DockMenu>,
}

fn append_items(
    submenu: &Submenu,
    entries: &[DesktopMenuItem],
    items: &mut Vec<MenuItem>,
) -> io::Result<()> {
    for entry in entries {
        match entry {
            DesktopMenuItem::Submenu {
                label,
                items: children,
            } => {
                let child = Submenu::new(label, true);
                append_items(&child, children, items)?;
                submenu.append(&child).map_err(io::Error::other)?;
            }
            DesktopMenuItem::Command(command)
            | DesktopMenuItem::SelectedCommand(command)
            | DesktopMenuItem::Quit(command) => {
                let item = MenuItem::with_id(
                    &command.id,
                    &command.label,
                    command.enabled,
                    command.shortcut.as_ref().map(accelerator).transpose()?,
                );
                submenu.append(&item).map_err(io::Error::other)?;
                items.push(item);
            }
            DesktopMenuItem::Separator => submenu
                .append(&PredefinedMenuItem::separator())
                .map_err(io::Error::other)?,
            DesktopMenuItem::About {
                label,
                name,
                version,
                comments,
                credits,
            } => submenu
                .append(&PredefinedMenuItem::about(
                    Some(label),
                    Some(AboutMetadata {
                        name: Some(name.clone()),
                        version: Some(version.clone()),
                        comments: Some(comments.clone()),
                        credits: Some(credits.clone()),
                        ..Default::default()
                    }),
                ))
                .map_err(io::Error::other)?,
            DesktopMenuItem::Services => submenu
                .append(&PredefinedMenuItem::services(None))
                .map_err(io::Error::other)?,
            DesktopMenuItem::Hide(label) => submenu
                .append(&PredefinedMenuItem::hide(Some(label)))
                .map_err(io::Error::other)?,
            DesktopMenuItem::HideOthers => submenu
                .append(&PredefinedMenuItem::hide_others(None))
                .map_err(io::Error::other)?,
            DesktopMenuItem::ShowAll => submenu
                .append(&PredefinedMenuItem::show_all(None))
                .map_err(io::Error::other)?,
            DesktopMenuItem::Minimize => submenu
                .append(&PredefinedMenuItem::minimize(None))
                .map_err(io::Error::other)?,
            DesktopMenuItem::Maximize => submenu
                .append(&PredefinedMenuItem::maximize(Some("Zoom")))
                .map_err(io::Error::other)?,
            DesktopMenuItem::BringAllToFront => submenu
                .append(&PredefinedMenuItem::bring_all_to_front(None))
                .map_err(io::Error::other)?,
        }
    }
    Ok(())
}

fn commands(entries: &[DesktopMenuItem]) -> Vec<&DesktopMenuCommand> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            DesktopMenuItem::Command(command)
            | DesktopMenuItem::SelectedCommand(command)
            | DesktopMenuItem::Quit(command) => vec![command],
            DesktopMenuItem::Submenu { items, .. } => commands(items),
            _ => Vec::new(),
        })
        .collect()
}

fn normalize_enabled(entries: &mut [DesktopMenuItem]) {
    for entry in entries {
        match entry {
            DesktopMenuItem::Command(command)
            | DesktopMenuItem::SelectedCommand(command)
            | DesktopMenuItem::Quit(command) => command.enabled = true,
            DesktopMenuItem::Submenu { items, .. } => normalize_enabled(items),
            _ => {}
        }
    }
}

fn perform_command(menu: &NSMenu, entries: &[DesktopMenuItem], id: &str) -> bool {
    for (index, (entry, item)) in entries.iter().zip(menu.itemArray().iter()).enumerate() {
        match entry {
            DesktopMenuItem::Command(command)
            | DesktopMenuItem::SelectedCommand(command)
            | DesktopMenuItem::Quit(command)
                if command.id == id =>
            {
                menu.performActionForItemAtIndex(index as isize);
                return true;
            }
            DesktopMenuItem::Submenu { items, .. } => {
                if let Some(submenu) = item.submenu()
                    && perform_command(&submenu, items, id)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

impl MacOsMenu {
    pub fn install(definitions: &[DesktopMenu], dock: &[DesktopMenuCommand]) -> io::Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("native menus require AppKit's main thread"))?;
        let application = NSApplication::sharedApplication(mtm);
        let previous = application.mainMenu();
        let menu = Menu::new();
        let mut items = Vec::new();
        for definition in definitions {
            let submenu = Submenu::new(&definition.label, true);
            append_items(&submenu, &definition.items, &mut items)?;
            menu.append(&submenu).map_err(io::Error::other)?;
            if definition.windows_menu {
                submenu.set_as_windows_menu_for_nsapp();
            }
        }
        menu.init_for_nsapp();
        let pending = Rc::new(RefCell::new(Vec::new()));
        let mut this = Self {
            menu,
            items,
            definitions: definitions.to_vec(),
            dock_definitions: dock.to_vec(),
            previous,
            targets: Vec::new(),
            pending,
            dock: None,
        };
        if !dock.is_empty() {
            this.dock = Some(DockMenu::install(
                &application,
                mtm,
                dock,
                Rc::clone(&this.pending),
            )?);
        }
        if !this.is_installed() {
            return Err(io::Error::other("native menu installation failed"));
        }
        this.attach_native_targets(mtm)?;
        Ok(this)
    }

    pub fn update(
        &mut self,
        definitions: &[DesktopMenu],
        dock: &[DesktopMenuCommand],
    ) -> io::Result<()> {
        let mut old = self.definitions.clone();
        let mut new = definitions.to_vec();
        for list in [&mut old, &mut new] {
            for menu in list {
                normalize_enabled(&mut menu.items);
            }
        }
        if old == new && self.dock_definitions == dock {
            let commands = definitions.iter().flat_map(|menu| commands(&menu.items));
            for (item, command) in self.items.iter_mut().zip(commands) {
                item.set_enabled(command.enabled);
            }
            self.definitions = definitions.to_vec();
            return Ok(());
        }
        // Detach our proxies and Dock subclass before constructing replacement native objects.
        let queued = self.poll();
        self.restore();
        self.dock.take();
        let replacement = Self::install(definitions, dock)?;
        let previous = self.previous.take();
        *self = replacement;
        self.previous = previous;
        self.pending.borrow_mut().extend(queued);
        Ok(())
    }

    pub fn poll(&self) -> Vec<DesktopNativeAction> {
        // Drain Muda's delivery for our IDs; proxies already recorded complete origin metadata.
        for _event in MenuEvent::receiver().try_iter() {}
        std::mem::take(&mut *self.pending.borrow_mut())
    }

    fn attach_native_targets(&mut self, mtm: MainThreadMarker) -> io::Result<()> {
        // SAFETY: Muda owns this live native menu.
        let root = unsafe { self.menu.ns_menu().cast::<NSMenu>().as_ref() }
            .ok_or_else(|| io::Error::other("native menu is missing"))?;
        for (definition, root_item) in self.definitions.clone().iter().zip(root.itemArray().iter())
        {
            if let Some(submenu) = root_item.submenu() {
                self.attach_menu_targets(&submenu, &definition.items, mtm)?;
            }
        }
        Ok(())
    }

    fn attach_menu_targets(
        &mut self,
        submenu: &NSMenu,
        entries: &[DesktopMenuItem],
        mtm: MainThreadMarker,
    ) -> io::Result<()> {
        for (entry, item) in entries.iter().zip(submenu.itemArray().iter()) {
            let (command, quit) = match entry {
                DesktopMenuItem::Command(command) | DesktopMenuItem::SelectedCommand(command) => {
                    (command, false)
                }
                DesktopMenuItem::Submenu { items, .. } => {
                    if let Some(child) = item.submenu() {
                        self.attach_menu_targets(&child, items, mtm)?;
                    }
                    continue;
                }
                DesktopMenuItem::Quit(command) => (command, true),
                _ => continue,
            };
            item.setState(if matches!(entry, DesktopMenuItem::SelectedCommand(_)) {
                1
            } else {
                0
            });
            let target = MenuTarget::alloc(mtm).set_ivars(MenuTargetState {
                original_target: item
                    .target()
                    .ok_or_else(|| io::Error::other("native menu item target is missing"))?,
                original_selector: item
                    .action()
                    .ok_or_else(|| io::Error::other("native menu item action is missing"))?,
                command: command.id.clone(),
                quit,
                pending: Rc::clone(&self.pending),
            });
            // SAFETY: NSObject initialization preserves ivars.
            let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
            unsafe {
                item.setTarget(Some(&target));
                item.setAction(Some(sel!(performMenuAction:)));
            }
            self.targets.push((item, target));
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

    pub fn perform_for_smoke(&self, id: &str) -> io::Result<()> {
        let root = unsafe { self.menu.ns_menu().cast::<NSMenu>().as_ref() }
            .ok_or_else(|| io::Error::other("native menu is missing"))?;
        for (definition, item) in self.definitions.iter().zip(root.itemArray().iter()) {
            let Some(submenu) = item.submenu() else {
                continue;
            };
            if perform_command(&submenu, &definition.items, id) {
                return Ok(());
            }
        }
        Err(io::Error::other(format!(
            "native menu command is missing: {id}"
        )))
    }

    pub fn perform_plugin_for_smoke(&self, id: &str) -> io::Result<()> {
        self.perform_for_smoke(id)
    }

    pub fn perform_dock_for_smoke(&self, id: &str) -> io::Result<()> {
        let index = self
            .dock_definitions
            .iter()
            .position(|command| command.id == id)
            .ok_or_else(|| io::Error::other("Dock command is missing"))?;
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("native Dock menu requires the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        let delegate = app
            .delegate()
            .ok_or_else(|| io::Error::other("native Dock menu delegate is missing"))?;
        let menu = delegate
            .applicationDockMenu(&app)
            .ok_or_else(|| io::Error::other("native Dock menu is missing"))?;
        menu.performActionForItemAtIndex(index as isize);
        Ok(())
    }

    pub fn key_equivalent_for_smoke(
        &self,
        characters: &str,
        key_code: u16,
        shift: bool,
    ) -> io::Result<()> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("native menu requires the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        let window = app
            .keyWindow()
            .ok_or_else(|| io::Error::other("native key fixture has no key window"))?;
        let characters = NSString::from_str(characters);
        let modifiers = NSEventModifierFlags::Command
            | if shift {
                NSEventModifierFlags::Shift
            } else {
                NSEventModifierFlags::empty()
            };
        for event_type in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(event_type,NSPoint::ZERO,modifiers,0.0,window.windowNumber(),None,&characters,&characters,false,key_code).ok_or_else(|| io::Error::other("AppKit could not create native key event"))?;
            app.postEvent_atStart(&event, false);
        }
        Ok(())
    }

    fn restore(&mut self) {
        for (item, target) in self.targets.drain(..) {
            // SAFETY: Restore retained original targets before proxies are released.
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

impl Drop for MacOsMenu {
    fn drop(&mut self) {
        self.restore();
    }
}

fn modifier_flags(flags: NSEventModifierFlags) -> DesktopModifiers {
    DesktopModifiers {
        ctrl: flags.contains(NSEventModifierFlags::Control),
        shift: flags.contains(NSEventModifierFlags::Shift),
        alt: flags.contains(NSEventModifierFlags::Option),
        super_key: flags.contains(NSEventModifierFlags::Command),
    }
}

fn accelerator(shortcut: &DesktopShortcut) -> io::Result<Accelerator> {
    let mut modifiers = Modifiers::empty();
    if shortcut.modifiers.ctrl {
        modifiers |= Modifiers::CONTROL;
    }
    if shortcut.modifiers.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if shortcut.modifiers.alt {
        modifiers |= Modifiers::ALT;
    }
    if shortcut.modifiers.super_key {
        modifiers |= Modifiers::META;
    }
    let code = key_code(shortcut.key).ok_or_else(|| {
        io::Error::other(format!(
            "unsupported native menu shortcut key: {:?}",
            shortcut.key
        ))
    })?;
    Ok(Accelerator::new(modifiers, code))
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
        Key0 => Code::Digit0,
        Key1 => Code::Digit1,
        Key2 => Code::Digit2,
        Key3 => Code::Digit3,
        Key4 => Code::Digit4,
        Key5 => Code::Digit5,
        Key6 => Code::Digit6,
        Key7 => Code::Digit7,
        Key8 => Code::Digit8,
        Key9 => Code::Digit9,
        F1 => Code::F1,
        F2 => Code::F2,
        F3 => Code::F3,
        F4 => Code::F4,
        F5 => Code::F5,
        F6 => Code::F6,
        F7 => Code::F7,
        F8 => Code::F8,
        F9 => Code::F9,
        F10 => Code::F10,
        F11 => Code::F11,
        F12 => Code::F12,
        F13 => Code::F13,
        F14 => Code::F14,
        F15 => Code::F15,
        F16 => Code::F16,
        F17 => Code::F17,
        F18 => Code::F18,
        F19 => Code::F19,
        F20 => Code::F20,
        F21 => Code::F21,
        F22 => Code::F22,
        F23 => Code::F23,
        F24 => Code::F24,
        LeftArrow => Code::ArrowLeft,
        RightArrow => Code::ArrowRight,
        UpArrow => Code::ArrowUp,
        DownArrow => Code::ArrowDown,
        Home => Code::Home,
        End => Code::End,
        PageUp => Code::PageUp,
        PageDown => Code::PageDown,
        Insert => Code::Insert,
        Delete => Code::Delete,
        Backspace => Code::Backspace,
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
