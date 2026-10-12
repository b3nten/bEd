//! Native mechanisms accept application-owned presentation and emit owned intents.
#[cfg(target_os = "linux")]
pub mod linux_window;
#[cfg(target_os = "macos")]
pub mod macos_menu;
#[cfg(target_os = "macos")]
pub mod macos_window;

use crate::{DesktopContext, DesktopModifiers, DesktopSystems};
#[cfg(target_os = "macos")]
use bevy::ecs::system::SystemState;
use bevy::{prelude::*, window::PrimaryWindow};
use dear_imgui_rs::{ClipboardBackend, Key};
use std::{io, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopShortcut {
    pub key: Key,
    pub modifiers: DesktopModifiers,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopMenuCommand {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub shortcut: Option<DesktopShortcut>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum DesktopMenuItem {
    Command(DesktopMenuCommand),
    SelectedCommand(DesktopMenuCommand),
    Submenu {
        label: String,
        items: Vec<DesktopMenuItem>,
    },
    /// Same activation protocol as commands; the host applies its configured close policy.
    Quit(DesktopMenuCommand),
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
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopMenu {
    pub label: String,
    pub items: Vec<DesktopMenuItem>,
    pub windows_menu: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopToolbarItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: bool,
}
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct DesktopNativePresentation {
    pub opacity: f32,
    pub blur: bool,
    pub foreground: [f32; 4],
    pub background: [f32; 4],
    pub menus: Vec<DesktopMenu>,
    pub toolbar: Vec<DesktopToolbarItem>,
    pub dock: Vec<DesktopMenuCommand>,
}
impl Default for DesktopNativePresentation {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blur: false,
            foreground: [0.9, 0.9, 0.9, 1.0],
            background: [0.1, 0.1, 0.1, 1.0],
            menus: Vec::new(),
            toolbar: Vec::new(),
            dock: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopNativeAction {
    pub command: String,
    pub keyboard: bool,
    pub modifiers: DesktopModifiers,
    pub quit: bool,
}
#[derive(Resource, Default)]
pub struct DesktopNativeActions(pub Vec<DesktopNativeAction>);
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct DesktopContentInsets {
    pub top: f32,
}
#[derive(Resource, Clone, Copy, Debug)]
pub struct DesktopNativeCapabilities {
    pub menus: bool,
    pub toolbar: bool,
    pub dock_menu: bool,
    pub clipboard: bool,
    pub file_clipboard: bool,
}
impl Default for DesktopNativeCapabilities {
    fn default() -> Self {
        Self {
            menus: cfg!(target_os = "macos"),
            toolbar: cfg!(target_os = "macos"),
            dock_menu: cfg!(target_os = "macos"),
            clipboard: false,
            file_clipboard: false,
        }
    }
}
#[derive(Resource, Default)]
pub struct DesktopNativeErrors(pub Vec<String>);
#[cfg(target_os = "macos")]
#[derive(Resource)]
pub(crate) struct NativeClosePolicy(pub bool);

/// Clipboard ownership remains on the native/UI thread.
#[derive(Clone)]
pub struct DesktopFileClipboard(std::rc::Rc<std::cell::RefCell<arboard::Clipboard>>);
impl DesktopFileClipboard {
    pub fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0
            .borrow_mut()
            .get()
            .file_list()
            .map_err(io::Error::other)
    }
    pub fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.0
            .borrow_mut()
            .set()
            .file_list(paths)
            .map_err(io::Error::other)
    }
}
struct TextClipboard(arboard::Clipboard);
impl ClipboardBackend for TextClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }
    fn set(&mut self, text: &str) {
        let _ = self.0.set_text(text);
    }
}

/// Native objects become available after Winit creates the primary window.
#[derive(Default)]
pub struct DesktopNativePlatform {
    initialized: bool,
    #[cfg(target_os = "macos")]
    pub window: Option<macos_window::MacOsWindow>,
    #[cfg(target_os = "macos")]
    pub menu: Option<macos_menu::MacOsMenu>,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesktopNativeSystems {
    Poll,
    Prepare,
}
pub struct DesktopNativePlugin;
impl Plugin for DesktopNativePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DesktopNativePresentation>()
            .init_resource::<DesktopNativeActions>()
            .init_resource::<DesktopContentInsets>()
            .init_resource::<DesktopNativeCapabilities>()
            .init_resource::<DesktopNativeErrors>()
            .init_non_send::<DesktopNativePlatform>()
            .add_systems(
                PreUpdate,
                poll.in_set(DesktopNativeSystems::Poll)
                    .after(DesktopSystems::InjectInput)
                    .before(DesktopSystems::InterceptInput),
            )
            .add_systems(
                PreUpdate,
                prepare
                    .in_set(DesktopNativeSystems::Prepare)
                    .in_set(DesktopSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}
fn poll(world: &mut World) {
    #[cfg(target_os = "macos")]
    {
        let mut actions = Vec::new();
        let mut native = world.non_send_mut::<DesktopNativePlatform>();
        if let Some(window) = &mut native.window {
            actions.extend(window.take_actions());
        }
        if let Some(menu) = &native.menu {
            actions.extend(menu.poll());
        }
        let close = world.resource::<NativeClosePolicy>().0;
        if close && actions.iter().any(|action| action.quit) {
            world.write_message(AppExit::Success);
        }
        world
            .resource_mut::<DesktopNativeActions>()
            .0
            .extend(actions);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = world;
}
fn prepare(world: &mut World) {
    let primary = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .iter(world)
        .next();
    let window_exists = bevy::winit::WINIT_WINDOWS.with(|windows| {
        primary.is_some_and(|entity| windows.borrow().get_window(entity).is_some())
    });
    if !window_exists {
        return;
    }
    if !world.non_send::<DesktopNativePlatform>().initialized {
        if let Ok(clipboard) = arboard::Clipboard::new() {
            world
                .non_send_mut::<DesktopContext>()
                .context
                .set_clipboard_backend(TextClipboard(clipboard));
            world.resource_mut::<DesktopNativeCapabilities>().clipboard = true;
        }
        if let Ok(clipboard) = arboard::Clipboard::new() {
            world.insert_non_send(DesktopFileClipboard(std::rc::Rc::new(
                std::cell::RefCell::new(clipboard),
            )));
            world
                .resource_mut::<DesktopNativeCapabilities>()
                .file_clipboard = true;
        }
        world.non_send_mut::<DesktopNativePlatform>().initialized = true;
    }
    #[cfg(target_os = "macos")]
    {
        let title = primary
            .and_then(|entity| world.get::<Window>(entity))
            .map(|window| window.title.clone())
            .unwrap_or_else(|| "Desktop".into());
        let mut requested = world.resource::<DesktopNativePresentation>().clone();
        if requested.menus.is_empty() {
            requested.menus.push(DesktopMenu {
                label: title.clone(),
                windows_menu: false,
                items: vec![DesktopMenuItem::Quit(DesktopMenuCommand {
                    id: "desktop.quit".into(),
                    label: format!("Quit {title}"),
                    enabled: true,
                    shortcut: Some(DesktopShortcut {
                        key: Key::Q,
                        modifiers: DesktopModifiers {
                            super_key: true,
                            ..default()
                        },
                    }),
                })],
            });
        }
        let empty_icons = bed_ui::icons::Icons::default();
        let result = {
            let mut state = SystemState::<(
                NonSendMut<DesktopNativePlatform>,
                Option<NonSend<bed_ui::icons::Icons>>,
            )>::new(world);
            let (mut native, icons) = state.get_mut(world).expect("native resources installed");
            let icons = icons.as_deref().unwrap_or(&empty_icons);
            bevy::winit::WINIT_WINDOWS.with(|windows| -> io::Result<f32> {
                let windows = windows.borrow();
                let window = primary
                    .and_then(|entity| windows.get_window(entity))
                    .expect("primary native window checked");
                if native.window.is_none() {
                    native.window = Some(macos_window::MacOsWindow::configure(
                        window,
                        requested.opacity,
                        requested.blur,
                        icons,
                        &requested.toolbar,
                    )?);
                }
                let chrome = native.window.as_mut().expect("native chrome installed");
                chrome.update(requested.opacity, requested.blur)?;
                chrome.set_commands(&requested.toolbar)?;
                chrome.update_theme(requested.foreground, requested.background)?;
                chrome.set_title(&title);
                if let Some(menu) = &mut native.menu {
                    menu.update(&requested.menus, &requested.dock)?;
                } else {
                    native.menu = Some(macos_menu::MacOsMenu::install(
                        &requested.menus,
                        &requested.dock,
                    )?);
                }
                Ok(native
                    .window
                    .as_ref()
                    .expect("native chrome installed")
                    .titlebar_inset())
            })
        };
        match result {
            Ok(top) => world.resource_mut::<DesktopContentInsets>().top = top,
            Err(error) => {
                let error = error.to_string();
                let mut errors = world.resource_mut::<DesktopNativeErrors>();
                if errors.0.last() != Some(&error) {
                    errors.0.push(error);
                }
            }
        }
    }
}
fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        world.remove_non_send::<DesktopNativePlatform>();
        world.remove_non_send::<DesktopFileClipboard>();
    }
}
