use bedtime_ui::UiPlugin;
use bedtime_workbench::{
    AreaBackground, DockMenuAppExt, LayoutCamera, LayoutSurface, SurfaceBody, TabBarVisibility,
    Titlebar, WorkbenchPlugins, status::StatusAppExt,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::PathBuf;
use workspace::{Start, Workspace};

fn main() -> AppExit {
    if let Some(exit) = bedtime_terminal::handle_shell_command() {
        return exit;
    }
    let argument = std::env::args_os().nth(1).map(PathBuf::from);
    app(Start::CommandLine(argument)).run()
}

pub fn app(start: Start) -> App {
    let config = config_directory();
    let workspaces = workspace::WorkspacesPlugin::new(start, config.clone(), app);
    let automation = bedtime_automation::AutomationPlugin::from_env();
    // A script is driving: don't take focus from what you're doing.
    let window = window(workspaces.workspace(), !automation.active());

    // Logging is set up once per process; a relaunched App leaves it alone.
    let mut defaults = DefaultPlugins.build();
    if bevy::winit::relaunching() {
        defaults = defaults.disable::<bevy::log::LogPlugin>();
    }

    let mut app = App::new();
    // Models are handed to Bevy's glTF loader from memory.
    bedtime_model::register_asset_source(&mut app);
    app.add_plugins((
        defaults.set(WindowPlugin {
            primary_window: Some(window),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        // The menu bar, its App menu under bEd's name.
        WorkbenchPlugins.set(bedtime_workbench::StandardMenusPlugin { app_name: "bEd" }),
        workspaces,
        themes::plugin(),
        // Language servers as settings, and their panel.
        bedtime_code::lsp::LanguageServersPanelPlugin,
        settings::BedSettingsPlugin,
        menus::MenusPlugin,
        layouts::LayoutSlotsPlugin,
        effects::EffectsPlugin,
        workspace::WorkspacesPanelPlugin,
        (
            files::FilesPlugin,
            terminal::TerminalsPlugin,
            mascots::MascotsPlugin,
            new::NewPlugin,
            git::BedGitPlugin,
            bedtime_http_client::HttpClientPlugin,
            search::BedSearchPlugin,
            preview::BedPreviewPlugin,
            automation,
        ),
    ))
    .init_resource::<bedtime_ui::fonts::InstalledFonts>()
    .add_status_item(bedtime_workbench::workspaces::status_item)
    .add_status_item(bedtime_code::lsp::status::status_item)
    .add_dock_menu_item::<menus::NewWindow>()
    .add_systems(Startup, setup);

    app
}

/// `$BED_CONFIG_DIR`, else `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`.
pub fn config_directory() -> Option<PathBuf> {
    bedtime_persistence::config_directory("bed", Some("BED_CONFIG_DIR"))
}

/// A transparent window whose content runs under the titlebar.
fn window(workspace: &Workspace, focused: bool) -> Window {
    Window {
        title: if workspace.default {
            "bEd".into()
        } else {
            format!("{} — bEd", workspace.name())
        },
        resolution: (960, 640).into(),
        focused,
        titlebar_show_title: false,
        titlebar_transparent: true,
        fullsize_content_view: true,
        transparent: true,
        composite_alpha_mode: if cfg!(target_os = "macos") {
            bevy::window::CompositeAlphaMode::PostMultiplied
        } else {
            bevy::window::CompositeAlphaMode::Auto
        },
        ..default()
    }
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        // A layout of one panel is just that panel, drawn onto the window:
        // no tab bar, no background (a terminal looks native this way).
        LayoutSurface {
            tab_bar: TabBarVisibility::MultipleTabsOrAreas,
            area_background: AreaBackground::MultipleAreas,
            ..default()
        },
        // The panels, with the status bar under them.
        SurfaceBody(bedtime_workbench::status::body),
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: menus::draw_titlebar,
        },
    ));
    commands.spawn((Camera2d, LayoutCamera));
}

// ═════════════════════════════════════════════════════════════════════════════
// Settings
//
// bEd's defaults for its plugins' settings, and the Settings panel's layout.
// ═════════════════════════════════════════════════════════════════════════════

mod settings {
    //! bEd's settings: its own defaults for plugins' settings, its themes, and
    //! how the Settings panel lays them out. The settings
    //! themselves belong to the crates whose behaviour they change (appearance
    //! and autosave to workbench, the editor's to bedtime-code, the file tree's
    //! to bedtime-filetree, the screen effects to `effects`).
    use bedtime_workbench::{
        appearance::{
            ColorTheme, FontFamily, FontSize, PanelGap, PanelOpacity, PanelRadius,
            ThemeEditorPlugin, WindowBlurEnabled, WindowOpacity,
        },
        settings::{
            SettingsAppExt, SettingsPanelPlugin,
            ui::{SettingsLayout, SettingsUiAppExt},
        },
        settings_panel,
    };
    use bevy::prelude::*;

    pub struct BedSettingsPlugin;

    impl Plugin for BedSettingsPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins((SettingsPanelPlugin, ThemeEditorPlugin))
                // bEd saves as you go.
                .set_setting_default::<bedtime_workbench::Autosave>(true)
                .set_settings_layout(layout());
        }
    }

    /// The Settings panel: every setting bEd shows, in its order.
    fn layout() -> SettingsLayout {
        use crate::effects::*;
        use bedtime_code::{LineNumbers, WrapLines};
        use bedtime_filetree::{ShowHiddenFiles, ShowIgnoredFiles};
        use bedtime_workbench::{Autosave, workspaces::ReopenLastWorkspace};
        settings_panel! {
            "Appearance" {
                ColorTheme, FontFamily, FontSize,
                EffectsEnabled, Scanlines, Vignette, Bloom, Noise, Curvature, PixelGrid, BurnIn, Pulse,
            }
            "Window" { WindowOpacity, WindowBlurEnabled }
            "Panels" { PanelOpacity, PanelRadius, PanelGap }
            "Editor" { WrapLines, Autosave, LineNumbers }
            "Files" { ShowHiddenFiles, ShowIgnoredFiles }
            "Startup" { ReopenLastWorkspace }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use bedtime_workbench::appearance::AppearanceSettings;
        use bedtime_workbench::{
            KeyChord, WorkbenchCorePlugins,
            settings::{SettingsPanel, ui::SettingsUi},
        };

        fn app() -> App {
            let mut app = App::new();
            app.add_plugins((
                WorkbenchCorePlugins,
                bedtime_workbench::appearance::AppearancePlugin,
                bedtime_workbench::DocumentSettingsPlugin,
                BedSettingsPlugin,
                crate::layouts::LayoutSlotsPlugin,
                // bEd's themes, without writing a folder.
                bedtime_workbench::appearance::ThemesPlugin {
                    folder: None,
                    ..crate::themes::plugin()
                },
            ));
            app
        }

        #[test]
        fn the_theme_and_autosave_have_beds_defaults() {
            let mut app = app();
            app.update();
            let world = app.world();
            assert_eq!(
                world.resource::<AppearanceSettings>().theme,
                crate::themes::DEFAULT
            );
            assert!(
                world
                    .resource::<bedtime_workbench::DocumentAutosave>()
                    .enabled()
            );
        }

        #[test]
        fn the_panel_shows_beds_sections_in_order() {
            let mut app = app();
            app.update();
            let sections: Vec<_> = app
                .world()
                .resource::<SettingsUi>()
                .sections()
                .into_iter()
                .map(|(section, _)| section)
                .collect();
            // Editor, Files and Startup come from plugins this test doesn't add.
            assert_eq!(sections, ["Appearance", "Window", "Panels", "Editor"]);
        }

        #[test]
        fn the_open_settings_keys_open_one_settings_panel() {
            use bedtime_workbench::{BindingInput, BindingPress, InvocationContext};
            let mut app = app();
            app.update();
            let chord = KeyChord::primary(KeyCode::Comma);
            for _ in 0..2 {
                app.world_mut()
                    .resource_mut::<BindingInput>()
                    .0
                    .push(BindingPress {
                        chord,
                        repeat: false,
                        context: InvocationContext::Focused,
                    });
                app.update();
            }
            let world = app.world_mut();
            let panels = world.query::<&SettingsPanel>().iter(world).count();
            assert_eq!(panels, 1);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Themes
//
// The themes bEd ships, and its default.
// ═════════════════════════════════════════════════════════════════════════════

mod themes {
    //! bEd's themes: the ones it ships, which `ThemesPlugin` (workbench) writes
    //! into the config folder's `themes` when missing, and its default.
    use bedtime_workbench::appearance::ThemesPlugin;
    use std::path::PathBuf;

    /// The shipped themes: each id and file.
    pub const BUILT_IN: &[(&str, &str)] = &[
        ("carbon", include_str!("themes/carbon.ron")),
        (
            "catppuccin-frappe",
            include_str!("themes/catppuccin-frappe.ron"),
        ),
        (
            "catppuccin-latte",
            include_str!("themes/catppuccin-latte.ron"),
        ),
        (
            "catppuccin-macchiato",
            include_str!("themes/catppuccin-macchiato.ron"),
        ),
        (
            "catppuccin-mocha",
            include_str!("themes/catppuccin-mocha.ron"),
        ),
        (
            "everforest-dark-hard",
            include_str!("themes/everforest-dark-hard.ron"),
        ),
        (
            "everforest-dark-medium",
            include_str!("themes/everforest-dark-medium.ron"),
        ),
        (
            "everforest-dark-soft",
            include_str!("themes/everforest-dark-soft.ron"),
        ),
        (
            "everforest-light-hard",
            include_str!("themes/everforest-light-hard.ron"),
        ),
        (
            "everforest-light-medium",
            include_str!("themes/everforest-light-medium.ron"),
        ),
        (
            "everforest-light-soft",
            include_str!("themes/everforest-light-soft.ron"),
        ),
        ("oxocarbon-dark", include_str!("themes/oxocarbon-dark.ron")),
        (
            "oxocarbon-light",
            include_str!("themes/oxocarbon-light.ron"),
        ),
        ("rose-pine", include_str!("themes/rose-pine.ron")),
        ("rose-pine-dawn", include_str!("themes/rose-pine-dawn.ron")),
        ("rose-pine-moon", include_str!("themes/rose-pine-moon.ron")),
        ("slash", include_str!("themes/slash.ron")),
        (
            "solarized-light",
            include_str!("themes/solarized-light.ron"),
        ),
        ("synthwave-84", include_str!("themes/synthwave-84.ron")),
        ("tokyo", include_str!("themes/tokyo.ron")),
    ];

    /// The default theme's id.
    pub const DEFAULT: &str = "everforest-dark-medium";

    /// Where themes are read from.
    pub fn folder() -> Option<PathBuf> {
        crate::config_directory().map(|config| config.join("themes"))
    }

    /// The themes folder, with bEd's shipped themes and default.
    pub fn plugin() -> ThemesPlugin {
        ThemesPlugin {
            folder: folder(),
            shipped: BUILT_IN,
            default: DEFAULT,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use bedtime_ui::{Theme, theme::ThemeFolder};
        use bedtime_workbench::WorkbenchCorePlugins;
        use bevy::prelude::*;

        #[test]
        fn bed_ships_twenty_valid_themes_and_installs_only_missing_ones() {
            assert_eq!(BUILT_IN.len(), 20);
            for (id, text) in BUILT_IN {
                Theme::from_ron(text).unwrap_or_else(|error| panic!("{id}: {error}"));
            }
            let folder = std::env::temp_dir().join(format!("bed-install-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            // An edited theme is kept.
            std::fs::write(folder.join("tokyo.ron"), "edited").unwrap();
            let mut app = App::new();
            app.add_plugins((
                WorkbenchCorePlugins,
                ThemesPlugin {
                    folder: Some(folder.clone()),
                    shipped: BUILT_IN,
                    default: DEFAULT,
                },
            ));
            assert_eq!(
                std::fs::read_to_string(folder.join("tokyo.ron")).unwrap(),
                "edited"
            );
            let themes = ThemeFolder::new(&folder);
            assert_eq!(themes.themes().count(), 19, "tokyo.ron no longer parses");
            assert!(themes.get(DEFAULT).is_some());
            std::fs::remove_dir_all(folder).unwrap();
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Menus and the titlebar
//
// Quit, folders and windows; bEd's own menu items; the titlebar with its menu bar and tool buttons.
// ═════════════════════════════════════════════════════════════════════════════

mod menus {
    use crate::workspace::{self, RecentWorkspaces, Start, Workspace};
    use bedtime_code::ShowStructure;
    use bedtime_filetree::{FileTree, ShowFiles};
    use bedtime_git::ShowGit;
    use bedtime_terminal::NewTerminal;
    use bedtime_ui::{BuiltinIcon, UiContext, egui};
    use bedtime_workbench::ErrorDialogWorldExt;
    use bedtime_workbench::WorkbenchWorldExt as _;
    use bedtime_workbench::settings::{OpenSettings, SettingsPanel};
    use bedtime_workbench::{
        Action, ActionAppExt, ActionId, ActionInvocation, ActionRequests, ActionResult,
        ActionsWorldExt, CloseRequested, ErrorDialog, Handler, InMenu, InvocationContext, Invokes,
        KeyChord, LayoutSurface, Menu, MenuEntries, MenuItem, MenusAppExt, PanelFactory,
        StandardMenu, StandardMenus, draw_menu_bar, panel_drag_source,
    };
    use bevy::prelude::*;
    use std::{future::Future, path::PathBuf, pin::Pin};

    pub struct Quit;
    impl Action for Quit {
        const KEY: &'static str = "app.quit";
        const LABEL: &'static str = "Quit bEd";
        const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyQ)];
    }

    pub struct OpenFolder;
    impl Action for OpenFolder {
        const KEY: &'static str = "app.open_folder";
        const LABEL: &'static str = "Open Folder…";
        const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyO)];
    }

    /// From an Open Recent item, opens that item's folder; from a key, the most
    /// recent folder other than this one.
    pub struct OpenRecent;
    impl Action for OpenRecent {
        const KEY: &'static str = "app.open_recent";
        const LABEL: &'static str = "Open Recent";
    }

    /// Back to the default workspace, in this window.
    pub struct CloseFolder;
    impl Action for CloseFolder {
        const KEY: &'static str = "app.close_folder";
        const LABEL: &'static str = "Close Folder";
    }

    /// Another window, in its own process, starting as bed does from the command line.
    pub struct NewWindow;
    impl Action for NewWindow {
        const KEY: &'static str = "app.new_window";
        const LABEL: &'static str = "New Window";
        const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
            bedtime_workbench::Modifiers::PRIMARY.union(bedtime_workbench::Modifiers::SHIFT),
            KeyCode::KeyN,
        )];
    }

    /// On an Open Recent item: the folder it opens.
    #[derive(Component)]
    struct RecentPath(PathBuf);

    /// The Open Recent submenu, refilled when the list changes.
    #[derive(Component)]
    struct RecentMenu;

    pub struct MenusPlugin;

    impl Plugin for MenusPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<RecentWorkspaces>()
                .init_non_send::<FolderDialog>()
                .add_action_handler::<Quit>(Handler::new(quit))
                .add_action_handler::<NewWindow>(Handler::new(new_window))
                .add_action_handler::<CloseFolder>(
                    Handler::new(close_folder)
                        .available_when(|world, _| !world.resource::<Workspace>().default),
                )
                .add_action_handler::<OpenFolder>(
                    Handler::new(open_folder)
                        .available_when(|world, _| world.non_send::<FolderDialog>().0.is_none()),
                )
                .add_action_handler::<OpenRecent>(
                    Handler::new(open_recent)
                        .available_when(|world, _| other_recent(world).is_some()),
                )
                .add_menu_item::<Quit>(StandardMenu::App, "quit")
                .add_menu_item::<NewWindow>(StandardMenu::File, "new")
                .add_menu_item::<OpenFolder>(StandardMenu::File, "open")
                .add_menu_entry(StandardMenu::File, "open", recent_menu)
                .add_menu_item::<CloseFolder>(StandardMenu::File, "open")
                .add_systems(Update, (poll_folder_dialog, fill_recent_menu));
        }
    }

    /// The Open Recent submenu: filled from the recent list.
    fn recent_menu(world: &mut World) -> Entity {
        world
            .spawn((
                Menu {
                    label: "Open Recent".into(),
                },
                RecentMenu,
            ))
            .id()
    }

    /// Recent folders other than this one, in Open Recent.
    fn fill_recent_menu(
        recent: Res<RecentWorkspaces>,
        workspace: Option<Res<Workspace>>,
        menus: Query<(Entity, Option<&MenuEntries>), With<RecentMenu>>,
        mut commands: Commands,
    ) {
        let Ok((menu, entries)) = menus.single() else {
            return;
        };
        if !recent.is_changed() && entries.is_some() {
            return;
        }
        for &entry in entries.map_or(&[][..], |entries| entries.entities()) {
            commands.entity(entry).despawn();
        }
        let current = workspace
            .filter(|workspace| !workspace.default)
            .map(|workspace| workspace.root.clone());
        let others: Vec<_> = recent
            .0
            .iter()
            .filter(|path| Some(*path) != current.as_ref())
            .collect();
        if others.is_empty() {
            // A menu needs an entry; this one is never enabled.
            commands.spawn((MenuItem::new("No Recent Folders"), InMenu(menu)));
        }
        for path in others {
            commands.spawn((
                MenuItem::new(path.display().to_string()),
                Invokes::of::<OpenRecent>(),
                RecentPath(path.clone()),
                InMenu(menu),
            ));
        }
    }

    fn quit(world: &mut World, _: ActionInvocation) -> ActionResult {
        world.write_message(CloseRequested { window: None });
        Ok(())
    }

    /// The folder picker, while it's open. AppKit owns the dialog, so its future is
    /// polled on the main thread; nothing blocks while it's open.
    #[derive(Default)]
    struct FolderDialog(Option<Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>>);

    fn open_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Folder")
            .pick_folder();
        world.non_send_mut::<FolderDialog>().0 = Some(Box::pin(dialog));
        Ok(())
    }

    fn poll_folder_dialog(world: &mut World) {
        let mut dialog = world.non_send_mut::<FolderDialog>();
        let Some(future) = dialog.0.as_mut() else {
            return;
        };
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(chosen) = future.as_mut().poll(&mut context) else {
            return;
        };
        dialog.0 = None;
        if let Some(folder) = chosen {
            switch(world, Start::Folder(folder.path().to_owned()));
        }
    }

    fn open_recent(world: &mut World, invocation: ActionInvocation) -> ActionResult {
        let chosen = invocation
            .source
            .and_then(|source| world.get::<RecentPath>(source))
            .map(|path| path.0.clone());
        let folder = chosen
            .or_else(|| other_recent(world))
            .ok_or("No recent folder")?;
        switch(world, Start::Folder(folder));
        Ok(())
    }

    fn close_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
        switch(world, Start::Default);
        Ok(())
    }

    /// This window's folder, if it isn't the default workspace.
    fn current_folder(world: &World) -> Option<&PathBuf> {
        let workspace = world.get_resource::<Workspace>()?;
        (!workspace.default).then_some(&workspace.root)
    }

    /// The most recent folder other than this window's.
    fn other_recent(world: &World) -> Option<PathBuf> {
        let current = current_folder(world);
        let recent = world.resource::<RecentWorkspaces>();
        recent.0.iter().find(|path| Some(*path) != current).cloned()
    }

    /// Switch this window to `start`'s workspace, or explain why not.
    pub fn switch(world: &mut World, start: Start) {
        if let Err(error) = workspace::switch(world, start) {
            world.request_error_dialog(ErrorDialog {
                title: "Open Folder".into(),
                message: error,
                details: None,
            });
        }
    }

    fn new_window(_: &mut World, _: ActionInvocation) -> ActionResult {
        spawn_window()
    }

    /// Start another bEd, as from the command line.
    pub fn spawn_window() -> Result<(), String> {
        std::env::current_exe()
            .and_then(|bed| std::process::Command::new(bed).spawn())
            .map(|_| ())
            .map_err(|error| format!("Couldn't start another bEd: {error}"))
    }

    /// The menu bar, where the platform has no native one, then the tools.
    pub fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
        if let Some(root) = world.get_resource::<StandardMenus>().map(|menus| menus.bar) {
            // Inset like the panels below it.
            let inset = world
                .get::<LayoutSurface>(context.surface)
                .map_or(0.0, |surface| surface.panel_gap);
            let result = draw_menu_bar(&mut context.frame(ui), world, root, inset);
            world.workbench_mut().notify_failure("Menu", result);
        }
        // Right to left: terminal, search, files, git, structure, settings.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().button_padding = egui::vec2(5.0, 4.0);
            tool::<OpenSettings>(
                ui,
                world,
                context,
                BuiltinIcon::Settings,
                PanelFactory::of::<SettingsPanel>(),
            );
            tool::<ShowStructure>(
                ui,
                world,
                context,
                BuiltinIcon::Structure,
                PanelFactory::of::<bedtime_code::StructurePanel>(),
            );
            tool::<ShowGit>(
                ui,
                world,
                context,
                BuiltinIcon::GitBranch,
                PanelFactory::of::<bedtime_git::GitPanel>(),
            );
            tool::<ShowFiles>(
                ui,
                world,
                context,
                BuiltinIcon::Folder,
                PanelFactory::of::<FileTree>(),
            );
            tool::<bedtime_search::SearchWorkspace>(
                ui,
                world,
                context,
                BuiltinIcon::Search,
                PanelFactory::of::<bedtime_search::SearchPanel>(),
            );
            tool::<NewTerminal>(
                ui,
                world,
                context,
                BuiltinIcon::Terminal,
                PanelFactory::of::<bedtime_terminal::TerminalPanel>(),
            );
        });
    }

    /// A titlebar button running command `C`. Dragging it out creates a new panel
    /// from `factory` where it's dropped.
    fn tool<C: Action>(
        ui: &mut egui::Ui,
        world: &mut World,
        context: &UiContext<'_>,
        icon: BuiltinIcon,
        factory: PanelFactory,
    ) {
        let command = ActionId::of::<C>();
        let enabled = world
            .actions()
            .available(&command, InvocationContext::Focused);
        let tooltip = match world
            .actions()
            .shortcut_for(&command, InvocationContext::Focused)
        {
            Some(chord) => format!("{} ({})", C::LABEL, chord.label()),
            None => C::LABEL.to_owned(),
        };
        ui.add_enabled_ui(enabled, |ui| {
            let response = context
                .frame(ui)
                .icon_ink_button(icon, 16.0)
                .interact(egui::Sense::click_and_drag());
            context.frame(ui).tooltip(&response, tooltip);
            panel_drag_source(&response, factory);
            if response.clicked() {
                world.resource_mut::<ActionRequests>().invoke::<C>();
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use bedtime_workbench::{MenuNode, MenusWorldExt, WorkbenchCorePlugins};

        fn labels(nodes: &[MenuNode]) -> Vec<(String, bool)> {
            nodes
                .iter()
                .filter_map(|node| match node {
                    MenuNode::Menu { label, .. } => Some((label.clone(), true)),
                    MenuNode::Item { label, enabled, .. } => Some((label.clone(), *enabled)),
                    MenuNode::Separator(_) => None,
                })
                .collect()
        }

        fn menu<'a>(nodes: &'a [MenuNode], name: &str) -> &'a [MenuNode] {
            nodes
                .iter()
                .find_map(|node| match node {
                    MenuNode::Menu { label, entries, .. } if label == name => Some(&entries[..]),
                    _ => None,
                })
                .unwrap()
        }

        #[test]
        fn open_recent_lists_other_folders_and_items_follow_their_commands() {
            let mut app = App::new();
            app.add_plugins((
                WorkbenchCorePlugins,
                bedtime_workbench::StandardMenusPlugin { app_name: "bEd" },
                crate::settings::BedSettingsPlugin,
                crate::layouts::LayoutSlotsPlugin,
                MenusPlugin,
            ))
            .insert_resource(RecentWorkspaces(vec!["/a".into(), "/b".into()]))
            .insert_resource(Workspace {
                root: "/a".into(),
                default: false,
            });
            app.update();
            let world = app.world_mut();
            let root = world.resource::<StandardMenus>().bar;
            let bar = world
                .menus()
                .snapshot(root, InvocationContext::Focused)
                .unwrap();
            assert_eq!(
                labels(&bar.entries),
                [
                    ("bEd".into(), true),
                    ("File".into(), true),
                    ("Edit".into(), true),
                    ("View".into(), true),
                    ("Layout".into(), true)
                ]
            );
            let file = menu(&bar.entries, "File");
            // Save is greyed out: no document is focused (a panel is).
            assert_eq!(
                labels(file),
                [
                    // The test app has no New handler.
                    ("New".into(), false),
                    ("New Window".into(), true),
                    ("Open Folder…".into(), true),
                    ("Open Recent".into(), true),
                    ("Close Folder".into(), true),
                    ("Save".into(), false),
                    ("Close Panel".into(), true)
                ]
            );
            assert_eq!(labels(menu(file, "Open Recent")), [("/b".into(), true)]);

            // Only this folder is recent: nothing else to open.
            world.resource_mut::<RecentWorkspaces>().0 = vec!["/a".into()];
            app.update();
            let world = app.world_mut();
            let bar = world
                .menus()
                .snapshot(root, InvocationContext::Focused)
                .unwrap();
            let recent = menu(menu(&bar.entries, "File"), "Open Recent");
            assert_eq!(labels(recent), [("No Recent Folders".into(), false)]);

            let open_recent = ActionId::of::<OpenRecent>();
            assert!(
                !world
                    .actions()
                    .available(&open_recent, InvocationContext::Focused)
            );

            // In the default workspace there's no folder to close, and every
            // recent folder can be opened.
            *world.resource_mut::<Workspace>() = Workspace {
                root: "/a".into(),
                default: true,
            };
            // An App's workspace never changes; refill as a new App would.
            world.resource_mut::<RecentWorkspaces>().set_changed();
            app.update();
            let world = app.world_mut();
            let bar = world
                .menus()
                .snapshot(root, InvocationContext::Focused)
                .unwrap();
            let file = menu(&bar.entries, "File");
            assert!(labels(file).contains(&("Close Folder".into(), false)));
            assert_eq!(labels(menu(file, "Open Recent")), [("/a".into(), true)]);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Layouts
//
// Five numbered layouts per workspace, each with a default every workspace shares.
// ═════════════════════════════════════════════════════════════════════════════

mod layouts {
    use crate::workspace::{GlobalStore, Workspace, WorkspacesPanel};
    use bedtime_filetree::FileTree;
    use bedtime_workbench::WorkbenchWorldExt as _;
    use bedtime_workbench::{
        Action, ActionAppExt, ActionInvocation, ActionResult, Handler, KeyChord, Layout,
        LayoutFallback, MenusAppExt, PanelFactory, StandardMenu,
        persistence::{LayoutTemplate, Persisted, PersistenceAppExt, ReflectPersist},
    };
    use bedtime_workbench::{LayoutsWorldExt, SplitAxis};
    use bevy::prelude::*;
    use serde::{Deserialize, Serialize};

    pub const SLOTS: usize = 5;

    /// bEd's Layout menu: the slots, then saving and resetting defaults.
    const LAYOUT: StandardMenu = StandardMenu::Custom("Layout");

    /// Which numbered layout this is, from 0.
    #[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
    #[reflect(Component, Persist, Default)]
    pub struct LayoutSlot(pub usize);

    impl LayoutSlot {
        fn name(self) -> String {
            format!("Layout {}", self.0 + 1)
        }
    }

    /// Each slot's default arrangement, shared by every workspace. `None` is a
    /// single empty area.
    #[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Persisted)]
    #[persisted(key = "layout_defaults")]
    pub struct LayoutDefaults(pub [Option<LayoutTemplate>; SLOTS]);

    /// Switch to numbered layout `N` (from 0).
    pub struct UseLayout<const N: usize>;
    impl<const N: usize> Action for UseLayout<N> {
        const KEY: &'static str = ["layout.1", "layout.2", "layout.3", "layout.4", "layout.5"][N];
        const LABEL: &'static str = ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][N];
        const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(
            [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
            ][N],
        )];
    }

    /// Make the current layout its slot's default, for every workspace.
    pub struct SaveLayoutAsDefault;
    impl Action for SaveLayoutAsDefault {
        const KEY: &'static str = "layout.save_default";
        const LABEL: &'static str = "Save Layout as Default";
    }

    /// Replace the current layout with its slot's default.
    pub struct ResetLayout;
    impl Action for ResetLayout {
        const KEY: &'static str = "layout.reset";
        const LABEL: &'static str = "Reset Layout";
    }

    pub struct LayoutSlotsPlugin;

    impl Plugin for LayoutSlotsPlugin {
        fn build(&self, app: &mut App) {
            app.register_type::<LayoutSlot>()
                .init_resource::<LayoutDefaults>()
                .persist::<LayoutDefaults, GlobalStore>()
                // Placing a panel with no layout open opens layout 1.
                .insert_resource(LayoutFallback(|world| switch(world, LayoutSlot(0))))
                .add_action_handler::<UseLayout<0>>(Handler::new(use_layout::<0>))
                .add_action_handler::<UseLayout<1>>(Handler::new(use_layout::<1>))
                .add_action_handler::<UseLayout<2>>(Handler::new(use_layout::<2>))
                .add_action_handler::<UseLayout<3>>(Handler::new(use_layout::<3>))
                .add_action_handler::<UseLayout<4>>(Handler::new(use_layout::<4>))
                .add_action_handler::<SaveLayoutAsDefault>(
                    Handler::new(save_as_default)
                        .available_when(|world, _| current(world).is_some()),
                )
                .add_action_handler::<ResetLayout>(
                    Handler::new(reset).available_when(|world, _| current(world).is_some()),
                )
                .add_menu_item::<UseLayout<0>>(LAYOUT, "slots")
                .add_menu_item::<UseLayout<1>>(LAYOUT, "slots")
                .add_menu_item::<UseLayout<2>>(LAYOUT, "slots")
                .add_menu_item::<UseLayout<3>>(LAYOUT, "slots")
                .add_menu_item::<UseLayout<4>>(LAYOUT, "slots")
                .add_menu_item::<SaveLayoutAsDefault>(LAYOUT, "defaults")
                .add_menu_item::<ResetLayout>(LAYOUT, "defaults")
                .add_systems(Startup, open_first);
        }
    }

    /// The active layout's slot.
    pub fn current(world: &World) -> Option<LayoutSlot> {
        world
            .layouts()
            .active()
            .and_then(|layout| world.get::<LayoutSlot>(layout).copied())
    }

    fn layout_in(world: &World, slot: LayoutSlot) -> Option<Entity> {
        world
            .layouts()
            .all()
            .iter()
            .copied()
            .find(|&layout| world.get::<LayoutSlot>(layout) == Some(&slot))
    }

    /// Make `slot`'s layout active, creating it from its default if needed.
    pub fn switch(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
        let layout = match layout_in(world, slot) {
            Some(layout) => layout,
            None => create(world, slot)?,
        };
        world.layouts_mut().activate(layout)?;
        Ok(layout)
    }

    /// A new layout for `slot` from its default; not made active.
    fn create(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
        let default = world.resource::<LayoutDefaults>().0[slot.0].clone();
        let layout = match default {
            Some(template) => world
                .layouts_mut()
                .instantiate_layout(&template)
                .map_err(|error| format!("{}'s default can't be used: {error}", slot.name()))?,
            None => {
                let layout = world.layouts_mut().create(slot.name())?;
                let default_workspace =
                    world.get_resource::<Workspace>().is_some_and(|w| w.default);
                if slot == LayoutSlot(0) {
                    let area = world.layouts_mut().destination_area(layout)?;
                    if default_workspace {
                        (PanelFactory::of::<WorkspacesPanel>().create)(world, area);
                    } else {
                        // Files in the left quarter; files open on the right.
                        world.layouts_mut().split_area(
                            area,
                            SplitAxis::Horizontal,
                            2500,
                            [100, 100],
                        )?;
                        (PanelFactory::of::<FileTree>().create)(world, area);
                    }
                }
                layout
            }
        };
        world.entity_mut(layout).insert(slot);
        world.get_mut::<Layout>(layout).unwrap().name = slot.name();
        Ok(layout)
    }

    /// At startup: the workspace's saved active layout, or layout 1. A workbench
    /// saved before slots existed gets its active layout as layout 1.
    fn open_first(world: &mut World) {
        if world
            .layouts()
            .all()
            .iter()
            .all(|&layout| world.get::<LayoutSlot>(layout).is_none())
            && let Some(layout) = world.layouts().active()
        {
            world.entity_mut(layout).insert(LayoutSlot(0));
        }
        if world.layouts().active().is_none() {
            let result = switch(world, LayoutSlot(0));
            world
                .workbench_mut()
                .notify_failure("Couldn't open a layout", result);
        }
    }

    fn use_layout<const N: usize>(world: &mut World, _: ActionInvocation) -> ActionResult {
        switch(world, LayoutSlot(N)).map(|_| ())
    }

    fn save_as_default(world: &mut World, _: ActionInvocation) -> ActionResult {
        let slot = current(world).ok_or("This layout has no number")?;
        let layout = world.layouts().active().ok_or("No layout is open")?;
        let template = world.layouts_mut().capture_layout(layout)?;
        world.resource_mut::<LayoutDefaults>().0[slot.0] = Some(template);
        Ok(())
    }

    /// Build the replacement first, so a default that can't be used leaves the
    /// current layout alone.
    fn reset(world: &mut World, _: ActionInvocation) -> ActionResult {
        let slot = current(world).ok_or("This layout has no number")?;
        let old = world.layouts().active().ok_or("No layout is open")?;
        let new = create(world, slot)?;
        let workbench = world.layouts_mut();
        workbench.activate(new)?;
        if let Err(error) = workbench.delete(old) {
            workbench.activate(old)?;
            workbench.world_mut().despawn(new);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use bedtime_workbench::settings::SettingsPanel;
        use bedtime_workbench::{
            ActionId, ActionRequest, ActionRequests, InvocationContext, PanelsWorldExt,
            WorkbenchCorePlugins,
        };

        fn app() -> App {
            let mut app = App::new();
            app.add_plugins((WorkbenchCorePlugins, LayoutSlotsPlugin))
                .register_type::<SettingsPanel>();
            app.update();
            app
        }

        fn run<C: Action>(app: &mut App) {
            app.world_mut()
                .resource_mut::<ActionRequests>()
                .submit(ActionRequest {
                    action: ActionId::of::<C>(),
                    context: InvocationContext::Focused,
                    source: None,
                });
            app.update();
        }

        fn settings_panels(world: &mut World) -> usize {
            let layout = world.layouts().active().unwrap();
            world
                .layouts_mut()
                .panels(layout)
                .into_iter()
                .filter(|&panel| world.get::<SettingsPanel>(panel).is_some())
                .count()
        }

        fn open_settings(world: &mut World) {
            world.focus_or_open::<SettingsPanel>().unwrap();
        }

        #[test]
        fn slots_are_made_when_first_opened_and_keep_their_panels() {
            let mut app = app();
            assert_eq!(current(app.world()), Some(LayoutSlot(0)));
            open_settings(app.world_mut());

            run::<UseLayout<2>>(&mut app);
            let world = app.world_mut();
            assert_eq!(current(world), Some(LayoutSlot(2)));
            assert_eq!(world.layouts().all().len(), 2);
            assert_eq!(settings_panels(world), 0);
            let name = &world
                .get::<Layout>(world.layouts().active().unwrap())
                .unwrap()
                .name;
            assert_eq!(name, "Layout 3");

            run::<UseLayout<0>>(&mut app);
            assert_eq!(settings_panels(app.world_mut()), 1);
            assert_eq!(app.world().layouts().all().len(), 2);
        }

        #[test]
        fn a_saved_default_builds_new_slots_and_resets_the_layout() {
            let mut app = app();
            open_settings(app.world_mut());
            run::<SaveLayoutAsDefault>(&mut app);
            let defaults = app.world().resource::<LayoutDefaults>().clone();
            assert!(defaults.0[0].is_some() && defaults.0[1].is_none());

            // Another workspace (another App) shares the defaults.
            let mut other = App::new();
            other
                .add_plugins((WorkbenchCorePlugins, LayoutSlotsPlugin))
                .register_type::<SettingsPanel>()
                .insert_resource(defaults);
            other.update();
            assert_eq!(settings_panels(other.world_mut()), 1);

            // Reset puts back the default: Settings returns once closed.
            let world = other.world_mut();
            let layout = world.layouts().active().unwrap();
            for panel in world.layouts_mut().panels(layout) {
                world.layouts_mut().close_panel(panel).unwrap();
            }
            assert_eq!(settings_panels(world), 0);
            run::<ResetLayout>(&mut other);
            let world = other.world_mut();
            assert_eq!(settings_panels(world), 1);
            assert_eq!(current(world), Some(LayoutSlot(0)));
            assert_eq!(world.layouts().all().len(), 1);
        }

        #[test]
        fn a_workbench_saved_before_slots_keeps_its_layout_as_layout_1() {
            let mut app = App::new();
            app.add_plugins((WorkbenchCorePlugins, LayoutSlotsPlugin));
            let world = app.world_mut();
            let layout = world.layouts_mut().create("Old").unwrap();
            world.layouts_mut().activate(layout).unwrap();
            app.update();
            assert_eq!(app.world().get::<LayoutSlot>(layout), Some(&LayoutSlot(0)));
            assert_eq!(app.world().layouts().all().len(), 1);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Workspaces
//
// The Workspaces panel: the folders bEd has opened, to switch to or forget.
// ═════════════════════════════════════════════════════════════════════════════

mod workspace {
    use bedtime_ui::{BuiltinIcon, UiContext, egui};
    use bedtime_workbench::ErrorDialogWorldExt;
    pub use bedtime_workbench::workspaces::*;
    use bedtime_workbench::{
        Action, ActionAppExt, ActionInvocation, ActionResult, ErrorDialog, Handler, MenusAppExt,
        Panel, PanelsWorldExt, StandardMenu, persistence::ReflectPersist,
    };
    use bevy::prelude::*;
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    pub struct OpenWorkspaces;
    impl Action for OpenWorkspaces {
        const KEY: &'static str = "app.open_workspaces";
        const LABEL: &'static str = "Workspaces";
    }

    #[bedtime_ui::panel]
    #[derive(Reflect, Default)]
    #[reflect(Component, Persist)]
    pub struct WorkspacesPanel;

    pub struct WorkspacesPanelPlugin;

    impl Plugin for WorkspacesPanelPlugin {
        fn build(&self, app: &mut App) {
            app.register_type::<WorkspacesPanel>()
                .init_resource::<KnownWorkspaces>()
                .add_action_handler::<OpenWorkspaces>(Handler::new(open_workspaces))
                .add_menu_item::<OpenWorkspaces>(StandardMenu::View, "tools");
        }
    }

    fn open_workspaces(world: &mut World, _: ActionInvocation) -> ActionResult {
        world.focus_or_open::<WorkspacesPanel>().map(|_| ())
    }

    /// One row of the list.
    #[derive(Clone, Debug, PartialEq)]
    struct Known {
        folder: PathBuf,
        open_elsewhere: bool,
        missing: bool,
    }

    /// The list, looked up again about once a second while the panel is drawn:
    /// other windows open and close workspaces.
    #[derive(Resource, Default)]
    struct KnownWorkspaces {
        list: Vec<Known>,
        checked: Option<Instant>,
    }

    /// Recent folders first, then any other folder with a saved workspace (its
    /// store's `path.txt`).
    fn look_up(recent: &RecentWorkspaces, config: Option<&Path>) -> Vec<Known> {
        let mut folders = recent.0.clone();
        if let Some(config) = config
            && let Ok(stores) = std::fs::read_dir(config.join("workspaces"))
        {
            let mut others: Vec<PathBuf> = stores
                .flatten()
                .filter(|store| store.file_name() != "default")
                .filter_map(|store| std::fs::read(store.path().join("path.txt")).ok())
                .map(|bytes| PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
                .filter(|folder| !folders.contains(folder))
                .collect();
            others.sort();
            folders.extend(others);
        }
        folders
            .into_iter()
            .map(|folder| {
                let workspace = folder_workspace(&folder);
                Known {
                    open_elsewhere: config
                        .is_some_and(|config| !available(&workspace.store_directory(config))),
                    missing: !folder.is_dir(),
                    folder,
                }
            })
            .collect()
    }

    fn folder_workspace(folder: &Path) -> Workspace {
        Workspace {
            root: folder.to_owned(),
            default: false,
        }
    }

    /// What a row's buttons asked for.
    enum Choice {
        Open(PathBuf),
        Delete(PathBuf),
    }

    impl Panel for WorkspacesPanel {
        const LABEL: &'static str = "Workspaces";

        fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
            let config = crate::config_directory();
            let stale = world
                .resource::<KnownWorkspaces>()
                .checked
                .is_none_or(|checked| checked.elapsed() > Duration::from_secs(1));
            if stale {
                let list = look_up(world.resource::<RecentWorkspaces>(), config.as_deref());
                let mut known = world.resource_mut::<KnownWorkspaces>();
                known.list = list;
                known.checked = Some(Instant::now());
            }
            let list = world.resource::<KnownWorkspaces>().list.clone();
            let current = world.resource::<Workspace>().clone();
            let confirming_id = ui.id().with("confirm-delete");
            let mut confirming: Option<PathBuf> = ui.data(|data| data.get_temp(confirming_id));
            let mut action = None;
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Workspaces").strong());
                    ui.label(
                        egui::RichText::new(if list.is_empty() {
                            "None yet. Open a folder (File → Open Folder…) to make one."
                        } else {
                            "Click one to open it in this window."
                        })
                        .small()
                        .weak(),
                    );
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for known in &list {
                                let row = Row {
                                    known,
                                    here: !current.default && known.folder == current.root,
                                    confirming: confirming.as_ref() == Some(&known.folder),
                                };
                                match row.draw(ui, context) {
                                    Some(RowAction::Open) => {
                                        action = Some(Choice::Open(known.folder.clone()));
                                    }
                                    Some(RowAction::AskToDelete) => {
                                        confirming = Some(known.folder.clone());
                                    }
                                    Some(RowAction::Delete) => {
                                        action = Some(Choice::Delete(known.folder.clone()));
                                        confirming = None;
                                    }
                                    Some(RowAction::Cancel) => confirming = None,
                                    None => {}
                                }
                            }
                        });
                });
            ui.data_mut(|data| match &confirming {
                Some(folder) => {
                    data.insert_temp(confirming_id, folder.clone());
                }
                None => data.remove::<PathBuf>(confirming_id),
            });
            match action {
                Some(Choice::Open(folder)) => crate::menus::switch(world, Start::Folder(folder)),
                Some(Choice::Delete(folder)) => forget(world, &folder, config.as_deref()),
                None => {}
            }
        }
    }

    /// One workspace in the list.
    struct Row<'a> {
        known: &'a Known,
        /// This window's workspace.
        here: bool,
        /// Asking whether to delete it.
        confirming: bool,
    }

    enum RowAction {
        Open,
        AskToDelete,
        Delete,
        Cancel,
    }

    impl Row<'_> {
        /// A full-width line: folder icon, name, dimmed path; on the right, the
        /// status and a delete button. Clicking the line opens the workspace.
        fn draw(&self, ui: &mut egui::Ui, context: &UiContext<'_>) -> Option<RowAction> {
            let known = self.known;
            let name = known.folder.file_name().map_or_else(
                || known.folder.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let status = if self.here {
                Some("This window")
            } else if known.open_elsewhere {
                Some("Open in another window")
            } else if known.missing {
                Some("Folder missing")
            } else {
                None
            };
            let openable = status.is_none() && !self.confirming;
            let height = bedtime_ui::theme::control_height(context.metrics) + 4.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            // The line first, so the buttons drawn on it take their own clicks.
            let line = ui.interact(
                rect,
                ui.id().with(("workspace", &known.folder)),
                if openable {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            );
            line.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, openable, &name)
            });
            // The labels drawn over the line take egui's hover, so ask about the
            // pointer instead.
            let hovered = ui.rect_contains_pointer(rect);
            let palette = &context.theme.palette;
            // Tints of the text colour show on any background, including the
            // window's when the panel has none.
            let tint = |color: bedtime_ui::theme::Rgba, alpha: f32| {
                let mut color = color;
                color[3] = alpha;
                color
            };
            let fill = if self.confirming {
                Some(tint(palette.error, 0.14))
            } else if openable && hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                Some(tint(palette.text, 0.08))
            } else if self.here {
                Some(tint(palette.text, 0.04))
            } else {
                None
            };
            if let Some(fill) = fill {
                ui.painter().rect_filled(
                    rect,
                    context.metrics.rounding,
                    bedtime_ui::theme::color(fill),
                );
            }

            let mut result = line.clicked().then_some(RowAction::Open);
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            // Right to left: actions and status, then the name and path take
            // what's left.
            if self.confirming {
                if row.button("Cancel").clicked() {
                    result = Some(RowAction::Cancel);
                }
                let delete =
                    egui::RichText::new("Delete").color(bedtime_ui::theme::color(palette.error));
                if row.button(delete).clicked() {
                    result = Some(RowAction::Delete);
                }
                row.weak("Delete its saved layout and settings?");
            } else {
                let deletable = !self.here && !known.open_elsewhere;
                row.add_enabled_ui(deletable, |ui| {
                    let mut frame = context.frame(ui);
                    let button = frame.icon_button_sized(BuiltinIcon::Trash, 14.0);
                    let label = format!("Delete {name}");
                    button.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, deletable, &label)
                    });
                    frame.tooltip(&button, "Delete this workspace's saved layout and settings");
                    if button.clicked() {
                        result = Some(RowAction::AskToDelete);
                    }
                });
                if let Some(status) = status {
                    row.weak(status);
                }
            }
            row.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let side = 16.0;
                let (icon, _) =
                    ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                context
                    .frame(ui)
                    .icon_at(BuiltinIcon::Folder, [icon.min.x, icon.min.y], side);
                ui.add_space(4.0);
                ui.add(egui::Label::new(egui::RichText::new(&name).strong()).selectable(false));
                // Where it is; the name is already shown.
                let parent = known.folder.parent().unwrap_or(&known.folder);
                let path = egui::RichText::new(abbreviate(parent)).small().weak();
                ui.add(egui::Label::new(path).truncate().selectable(false));
            });
            result
        }
    }

    /// Delete `folder`'s workspace and take it off the recent list.
    fn forget(world: &mut World, folder: &Path, config: Option<&Path>) {
        let result = (|| {
            let config = config.ok_or("There's no configuration directory")?;
            delete(&folder_workspace(folder), config)?;
            world.resource_mut::<RecentWorkspaces>().forget(folder);
            Ok::<_, String>(())
        })();
        world.resource_mut::<KnownWorkspaces>().checked = None;
        if let Err(error) = result {
            world.request_error_dialog(ErrorDialog {
                title: "Delete Workspace".into(),
                message: format!("Couldn't delete the workspace for {}.", folder.display()),
                details: Some(error),
            });
        }
    }

    /// `~/…` for folders in the home folder.
    fn abbreviate(folder: &Path) -> String {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        match home
            .as_deref()
            .and_then(|home| folder.strip_prefix(home).ok())
        {
            Some(relative) => format!("~/{}", relative.display()),
            None => folder.display().to_string(),
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Files
//
// The file tree and the viewers, and what dropped files and folders do.
// ═════════════════════════════════════════════════════════════════════════════

mod files {
    use bedtime_audio::AudioPlugin;
    use bedtime_code::CodePlugin;
    use bedtime_filetree::FileTreePlugin;
    use bedtime_font::FontPlugin;
    use bedtime_hex::HexPlugin;
    use bedtime_image::ImagePlugin;
    use bedtime_model::ModelViewerPlugin;
    use bedtime_table::TablePlugin;
    use bedtime_video::VideoPlugin;
    use bedtime_workbench::{PanelFactory, WorkbenchWorldExt};
    use bevy::prelude::*;

    pub struct FilesPlugin;

    impl Plugin for FilesPlugin {
        fn build(&self, app: &mut App) {
            app.insert_resource(bedtime_workbench::FileDrops(dropped))
                .init_resource::<DroppedFolder>()
                .add_systems(Update, open_dropped_folder)
                .add_plugins((
                    FileTreePlugin,
                    CodePlugin,
                    HexPlugin,
                    ImagePlugin,
                    ModelViewerPlugin,
                    FontPlugin,
                    AudioPlugin,
                    VideoPlugin,
                    TablePlugin,
                ));
            // Save, Undo and Redo act on the focused panel's document.
            app.world_mut().workbench_mut().register_document_handlers();
        }
    }

    /// A folder dropped on the window, to open as the workspace (in `Update`,
    /// not while the layout draws).
    #[derive(Resource, Default)]
    struct DroppedFolder(Option<std::path::PathBuf>);

    /// Files dropped on a layout open where they're dropped; a folder opens as
    /// the workspace.
    fn dropped(world: &mut World, paths: Vec<std::path::PathBuf>) -> Option<PanelFactory> {
        if let [folder] = paths.as_slice()
            && folder.is_dir()
        {
            world.resource_mut::<DroppedFolder>().0 = Some(folder.clone());
            return None;
        }
        let files: Vec<_> = paths.into_iter().filter(|path| !path.is_dir()).collect();
        (!files.is_empty()).then(|| bedtime_filetree::open_files_factory(files))
    }

    fn open_dropped_folder(world: &mut World) {
        if let Some(folder) = world.resource_mut::<DroppedFolder>().0.take() {
            crate::menus::switch(world, crate::workspace::Start::Folder(folder));
        }
    }

    #[cfg(test)]
    mod tests {
        use bedtime_workbench::{WorkbenchCorePlugins, WorkbenchWorldExt};
        use bevy::prelude::*;

        /// Which viewers bed's plugins offer for files, the one each opens with
        /// first.
        #[test]
        fn files_open_with_the_viewer_their_format_suits() {
            let mut app = App::new();
            app.add_plugins((
                WorkbenchCorePlugins,
                bedtime_code::CodePlugin,
                bedtime_hex::HexPlugin,
                bedtime_image::ImagePlugin,
                bedtime_table::TablePlugin,
                bedtime_preview::PreviewPlugin,
            ));
            let directory =
                std::env::temp_dir().join(format!("bed-formats-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let offered = |name: &str, contents: &[u8]| {
                let path = directory.join(name);
                std::fs::write(&path, contents).unwrap();
                app.world()
                    .workbench()
                    .viewers_for(&path)
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect::<Vec<_>>()
            };
            assert_eq!(offered("a.csv", b"a,b"), ["Table", "Text Editor", "Hex"]);
            assert_eq!(
                offered("README.md", b"# Hi"),
                ["Text Editor", "Markdown Preview", "Hex"]
            );
            assert_eq!(
                offered("Cargo.lock", b"version = 4"),
                ["Text Editor", "Tree View", "Hex"]
            );
            assert_eq!(
                offered("logo.svg", b"<svg/>"),
                ["Image", "Text Editor", "Hex"]
            );
            assert_eq!(
                offered("photo.png", &[137, 80, 78, 71, 0]),
                ["Image", "Hex"]
            );
            assert_eq!(offered("main.rs", b"fn main() {}"), ["Text Editor", "Hex"]);
            assert_eq!(offered("notes.poop", b"plain"), ["Text Editor", "Hex"]);
            assert_eq!(offered("blob", &[0, 1, 2]), ["Hex"]);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// New and Save As
//
// New makes an untitled text document; saving one asks where.
// ═════════════════════════════════════════════════════════════════════════════

mod new {
    use bedtime_document::TextBuffer;
    use bedtime_workbench::{
        DocumentId, NewAppExt, Notification, WorkbenchWorldExt, workspaces::Workspace,
    };
    use bevy::prelude::*;
    use std::{future::Future, pin::Pin};

    /// New makes an untitled text document (workbench's `New`; a terminal makes
    /// another terminal itself). Saving one asks where, with the system picker.
    pub struct NewPlugin;

    impl Plugin for NewPlugin {
        fn build(&self, app: &mut App) {
            app.init_non_send::<SaveDialog>()
                .set_new_document::<TextBuffer>()
                .add_systems(Update, poll_save_dialog);
            app.world_mut().workbench_mut().set_save_as(ask_where);
        }
    }

    /// The save picker, while it's open: for which document. AppKit owns the
    /// dialog, so its future is polled on the main thread.
    #[derive(Default)]
    struct SaveDialog(Option<(DocumentId, Picking)>);

    /// The picker's answer: where to save, or nothing (dismissed).
    type Picking = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>;

    fn ask_where(world: &mut World, document: DocumentId) {
        let name = world
            .get::<bedtime_workbench::Untitled>(document.0)
            .map(|untitled| untitled.name.clone())
            .unwrap_or_default();
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Save")
            .set_file_name(format!("{name}.txt"));
        if let Some(workspace) = world.get_resource::<Workspace>() {
            dialog = dialog.set_directory(&workspace.root);
        }
        let previous = world
            .non_send_mut::<SaveDialog>()
            .0
            .replace((document, Box::pin(dialog.save_file())));
        // One picker at a time: the one it replaced is given up.
        if let Some((other, _)) = previous {
            world.workbench_mut().cancel_save_as(other);
        }
    }

    fn poll_save_dialog(world: &mut World) {
        let mut dialog = world.non_send_mut::<SaveDialog>();
        let Some((document, future)) = dialog.0.as_mut() else {
            return;
        };
        let document = *document;
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(chosen) = future.as_mut().poll(&mut context) else {
            return;
        };
        dialog.0 = None;
        match chosen {
            Some(file) => {
                if let Err(error) = world
                    .workbench_mut()
                    .save_untitled_to(document, file.path())
                {
                    world
                        .workbench_mut()
                        .notify(Notification::error(format!("Couldn't save: {error}")));
                }
            }
            None => world.workbench_mut().cancel_save_as(document),
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Preview
//
// An editor's Markdown or data file, previewed beside it.
// ═════════════════════════════════════════════════════════════════════════════

mod preview {
    use bedtime_code::EditorPanel;
    use bedtime_preview::{PreviewPlugin, preview_viewer};
    use bedtime_workbench::{
        Action, ActionAppExt, ActionInvocation, ActionResult, Handler, KeyChord, MenusAppExt,
        Modifiers, PathOptions, Placement, StandardMenu, ViewerFile, WorkbenchWorldExt,
    };
    use bevy::prelude::*;

    pub struct OpenPreview;
    /// Not Cmd/Ctrl+Shift+V: every Cmd/Ctrl+V chord is the editor's paste.
    impl Action for OpenPreview {
        const KEY: &'static str = "app.open_preview";
        const LABEL: &'static str = "Open Preview to the Side";
        const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
            Modifiers::PRIMARY.union(Modifiers::SHIFT),
            KeyCode::KeyM,
        )];
    }

    pub struct BedPreviewPlugin;

    impl Plugin for BedPreviewPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins(PreviewPlugin)
                .add_menu_item::<OpenPreview>(StandardMenu::View, "editor")
                .add_action_handler::<OpenPreview>(
                    Handler::new(open_preview).available_when(|world, panel| {
                        panel.is_some_and(|p| previewable(world, p))
                    }),
                );
        }
    }

    /// An editor showing a file that has a preview.
    fn previewable(world: &World, panel: Entity) -> bool {
        world.get::<EditorPanel>(panel).is_some()
            && world
                .get::<ViewerFile>(panel)
                .and_then(|file| file.0.file_name())
                .is_some_and(|name| preview_viewer(&name.to_string_lossy()).is_some())
    }

    /// The editor's document in its preview: the one already in the layout,
    /// else a new one beside the editor (`Placement::Beside`: with previews of
    /// its kind, else in the editor's area split in two, else as a tab in it).
    fn open_preview(world: &mut World, invocation: ActionInvocation) -> ActionResult {
        let editor = invocation.entity.ok_or("No editor is focused")?;
        if !previewable(world, editor) {
            return Err("This file has no preview".into());
        }
        let path = world.get::<ViewerFile>(editor).unwrap().0.clone();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // The preview already showing it, else a new one beside the editor; and
        // keep typing where you were (unless the preview had to go in the
        // editor's own area, where it's the tab shown).
        let options = PathOptions {
            viewer: preview_viewer(&name),
            place: Some(Placement::Beside(editor)),
            keep_focus: true,
            ..default()
        };
        world.workbench_mut().open_path(&path, options).map(|_| ())
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Terminal
//
// Terminals, and `+open` from their shells.
// ═════════════════════════════════════════════════════════════════════════════

mod terminal {
    use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation, TerminalPlugin};
    use bedtime_workbench::{PathOptions, WorkbenchWorldExt};
    use bevy::prelude::*;
    use std::path::{Path, PathBuf};

    pub struct TerminalsPlugin;

    impl Plugin for TerminalsPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins(TerminalPlugin)
                .add_shell_command(["+open", "+o"], open_from_shell)
                .expect("bed's shell commands are unique");
        }
    }

    /// `+open a.rs b.rs:12 c.rs:3:5`, relative to the shell's directory;
    /// `+open` or `+open folder` opens the folder as the workspace.
    fn open_from_shell(world: &mut World, invocation: ShellInvocation) {
        let folder = match invocation.arguments.as_slice() {
            [] => Some(invocation.directory.clone()),
            [only] => Some(invocation.directory.join(only)).filter(|path| path.is_dir()),
            _ => None,
        };
        if let Some(folder) = folder {
            invocation.reply.complete(Ok(()));
            // After this frame: switching relaunches the app.
            world.commands().queue(move |world: &mut World| {
                crate::menus::switch(world, crate::workspace::Start::Folder(folder));
            });
            return;
        }
        let mut result = Ok(());
        for argument in &invocation.arguments {
            if invocation.directory.join(argument).is_dir() {
                result = Err(format!(
                    "{}: open a folder on its own",
                    argument.to_string_lossy()
                ));
                continue;
            }
            let (path, line) = location(&invocation.directory, &argument.to_string_lossy());
            let options = PathOptions { line, ..default() };
            if let Err(error) = world.workbench_mut().open_path(&path, options) {
                result = Err(format!("{}: {error}", path.display()));
            }
        }
        invocation.reply.complete(result);
    }

    /// A path with an optional `:line` or `:line:column` (from 1), resolved
    /// against `directory`. A path that exists as written wins (a name with a
    /// colon in it).
    fn location(directory: &Path, argument: &str) -> (PathBuf, Option<(u32, u32)>) {
        let resolve = |path: &str| directory.join(path);
        if resolve(argument).exists() {
            return (resolve(argument), None);
        }
        let mut parts = argument.rsplitn(3, ':');
        let numbers: Vec<&str> = parts.by_ref().take(2).collect();
        let rest = parts.next();
        match (rest, numbers.as_slice()) {
            (Some(path), [column, line]) => match (line.parse::<u32>(), column.parse::<u32>()) {
                (Ok(line), Ok(column)) => (
                    resolve(path),
                    Some((line.saturating_sub(1), column.saturating_sub(1))),
                ),
                _ => location_line(directory, argument),
            },
            _ => location_line(directory, argument),
        }
    }

    fn location_line(directory: &Path, argument: &str) -> (PathBuf, Option<(u32, u32)>) {
        match argument.rsplit_once(':') {
            Some((path, line)) if line.parse::<u32>().is_ok() => (
                directory.join(path),
                Some((line.parse::<u32>().unwrap().saturating_sub(1), 0)),
            ),
            _ => (directory.join(argument), None),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn shell_locations_take_a_line_and_column() {
            let directory = Path::new("/project");
            assert_eq!(location(directory, "a.rs"), (directory.join("a.rs"), None));
            assert_eq!(
                location(directory, "src/a.rs:12"),
                (directory.join("src/a.rs"), Some((11, 0)))
            );
            assert_eq!(
                location(directory, "a.rs:3:5"),
                (directory.join("a.rs"), Some((2, 4)))
            );
            assert_eq!(
                location(directory, "/abs/b.rs:1"),
                (PathBuf::from("/abs/b.rs"), Some((0, 0)))
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Git
//
// The Git panel, with bEd's answers to opening a folder or a terminal from it.
// ═════════════════════════════════════════════════════════════════════════════

mod git {
    use bedtime_git::{GitHost, GitPlugin};
    use bedtime_terminal::{TerminalOptions, TerminalPanel};
    use bedtime_workbench::Place;
    use bevy::prelude::*;
    use std::path::PathBuf;

    pub struct BedGitPlugin;

    impl Plugin for BedGitPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins(GitPlugin).insert_resource(GitHost {
                open_folder: Some(open_folder),
                open_terminal: Some(open_terminal),
            });
        }
    }

    /// The repository's folder as the workspace (after this frame's drawing).
    fn open_folder(world: &mut World, folder: PathBuf) {
        world.commands().queue(move |world: &mut World| {
            crate::menus::switch(world, crate::workspace::Start::Folder(folder));
        });
    }

    fn open_terminal(world: &mut World, folder: PathBuf, area: Entity) {
        let options = TerminalOptions {
            working_directory: Some(folder),
            ..default()
        };
        world.spawn((TerminalPanel::new(options), Place::in_area(area)));
        world.flush();
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Search
//
// Find in Files, seeded with the editor's selection.
// ═════════════════════════════════════════════════════════════════════════════

mod search {
    use bedtime_code::EditorPanel;
    use bedtime_document::TextBuffer;
    use bedtime_search::{SearchPanel, SearchPlugin, SearchWorkspace};
    use bedtime_workbench::{
        ActionAppExt, ActionInvocation, ActionResult, DocumentReference, Handler, PanelsWorldExt,
        WorkbenchWorldExt,
    };
    use bevy::prelude::*;

    pub struct BedSearchPlugin;

    impl Plugin for BedSearchPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins(SearchPlugin)
                .add_action_handler::<SearchWorkspace>(Handler::new(search_workspace));
        }
    }

    fn search_workspace(world: &mut World, invocation: ActionInvocation) -> ActionResult {
        let selected = invocation.entity.and_then(|panel| selection(world, panel));
        let panel = world.focus_or_open::<SearchPanel>()?;
        bedtime_search::seed(world, panel, selected);
        Ok(())
    }

    /// The text selected in `panel`'s editor, if it has one.
    fn selection(world: &World, panel: Entity) -> Option<String> {
        let range = world.get::<EditorPanel>(panel)?.view.selection().range();
        if range.is_empty() {
            return None;
        }
        let document = world.get::<DocumentReference>(panel)?.document();
        let workbench = world.workbench();
        let text = workbench
            .read_document(workbench.cast_document::<TextBuffer>(document).ok()?)
            .ok()?;
        Some(text.range_text(range))
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Mascots
//
// Bed and Ducky (`+ducky` in a terminal).
// ═════════════════════════════════════════════════════════════════════════════

mod mascots {
    use bedtime_mascots::{DuckPanel, MascotPlugin};
    use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation};
    use bedtime_workbench::PanelsWorldExt;
    use bevy::prelude::*;

    pub struct MascotsPlugin;

    impl Plugin for MascotsPlugin {
        fn build(&self, app: &mut App) {
            app.add_plugins(MascotPlugin)
                .add_shell_command(["+ducky"], ducky)
                .expect("bed's shell commands are unique");
        }
    }

    fn ducky(world: &mut World, invocation: ShellInvocation) {
        let result = if invocation.arguments.is_empty() {
            world.focus_or_open::<DuckPanel>().map(|_| ())
        } else {
            Err(format!("usage: {}", invocation.alias))
        };
        invocation.reply.complete(result);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Screen effects
//
// CRT-style effects drawn over the window, as settings.
// ═════════════════════════════════════════════════════════════════════════════

mod effects {
    use bedtime_ui::{UiSystems, egui};
    use bedtime_workbench::{
        ActiveLayout, LayoutCamera,
        persistence::StoreId,
        settings::{SettingsAppExt, SettingsWorldExt, settings},
    };
    use bevy::{
        asset::{AssetPath, embedded_asset, embedded_path},
        core_pipeline::{Core2d, FullscreenShader, upscaling::upscaling},
        prelude::*,
        render::{
            Render, RenderApp, RenderStartup, RenderSystems,
            extract_component::{
                ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
                UniformComponentPlugin,
            },
            render_resource::{
                binding_types::{sampler, texture_2d, uniform_buffer},
                *,
            },
            renderer::{RenderContext, RenderDevice, ViewQuery},
            view::ViewTarget,
        },
    };
    use bevy_egui::{EguiInput, EguiPreUpdateSet, render::egui_pass};
    use std::ops::RangeInclusive;

    const INTENSITY: RangeInclusive<f32> = 0.0..=1.0;

    /// Settings write into `Effects`; `prepare_input` reads it each frame. The
    /// intensities show in Settings only while effects are on; they keep their
    /// values while off.
    macro_rules! effect_settings {
    ($($name:ident: $default:expr, $key:literal, $field:ident $(, $label:literal)?;)*) => {
        $(settings! {
            "Appearance" {
                pub $name: f32 {
                    default: $default,
                    key: $key,
                    $(label: $label,)?
                    ui: slider(INTENSITY),
                    visible: effects_on,
                    apply: |world, value| {
                        world.resource_mut::<Effects>().$field =
                            bedtime_workbench::settings::limit(*value, INTENSITY, $default)
                    },
                }
            }
        })*
        fn register_intensities(app: &mut App) {
            $(app.register_setting::<$name>();)*
        }
    };
}

    settings! {
        "Appearance" {
            pub EffectsEnabled: bool {
                default: false,
                key: "effects.enabled",
                label: "Screen effects",
                apply: |world, enabled| world.resource_mut::<Effects>().enabled = *enabled,
            }
        }
    }

    fn effects_on(world: &World, layer: StoreId) -> bool {
        world.settings().get_at::<EffectsEnabled>(layer)
    }

    effect_settings! {
        Scanlines: 0.06, "effects.scanlines", scanlines;
        Vignette: 0.04, "effects.vignette", vignette;
        Bloom: 0.025, "effects.bloom", bloom;
        Noise: 0.0, "effects.noise", noise;
        Curvature: 0.0, "effects.curvature", curvature;
        PixelGrid: 0.0, "effects.pixel_grid", pixel_grid;
        BurnIn: 0.0, "effects.burn_in", burn_in, "Burn-in";
        Pulse: 0.0, "effects.pulse", pulse;
    }

    /// The effects as the settings resolve them.
    #[derive(Resource, Clone, Copy, Debug, PartialEq)]
    pub struct Effects {
        pub enabled: bool,
        pub scanlines: f32,
        pub vignette: f32,
        pub bloom: f32,
        pub noise: f32,
        pub curvature: f32,
        pub pixel_grid: f32,
        pub burn_in: f32,
        pub pulse: f32,
    }

    impl Default for Effects {
        fn default() -> Self {
            use bedtime_workbench::settings::Setting;
            Self {
                enabled: EffectsEnabled::default(),
                scanlines: Scanlines::default(),
                vignette: Vignette::default(),
                bloom: Bloom::default(),
                noise: Noise::default(),
                curvature: Curvature::default(),
                pixel_grid: PixelGrid::default(),
                burn_in: BurnIn::default(),
                pulse: Pulse::default(),
            }
        }
    }

    pub struct EffectsPlugin;
    impl Plugin for EffectsPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<Effects>()
                .register_setting::<EffectsEnabled>();
            register_intensities(app);
            app.add_systems(
                PreUpdate,
                prepare_input
                    .after(EguiPreUpdateSet::ProcessInput)
                    .before(UiSystems::Draw),
            );
            // Headless apps (tests) have no renderer.
            if app.get_sub_app(RenderApp).is_none() {
                return;
            }
            // bed's sources sit at the crate root, not under `src`.
            embedded_asset!(app, "bed", "effects.wesl");
            app.add_plugins((
                ExtractComponentPlugin::<EffectsFrame>::default(),
                ExtractComponentPlugin::<EffectUniform>::default(),
                UniformComponentPlugin::<EffectUniform>::default(),
            ));
            app.sub_app_mut(RenderApp)
                .add_systems(RenderStartup, prepare_pipeline)
                .add_systems(
                    Render,
                    prepare_targets.in_set(RenderSystems::PrepareBindGroups),
                )
                .add_systems(
                    Core2d,
                    render
                        .after(egui_pass)
                        .before(UiSystems::PrepareOutput)
                        .before(upscaling),
                );
        }
    }

    #[derive(Component, Clone, Copy, ExtractComponent)]
    #[extract_app(RenderApp)]
    struct EffectsFrame {
        enabled: bool,
        /// The active layout: when it changes, burn-in history starts over.
        scene: Option<Entity>,
    }

    #[derive(Component, Clone, Copy, ShaderType, ExtractComponent)]
    #[extract_app(RenderApp)]
    struct EffectUniform {
        // Scanlines, vignette, bloom, noise.
        light: Vec4,
        // Curvature, pixel grid, persistence, pulse.
        geometry: Vec4,
        // Real elapsed seconds, frame-rate-independent persistence decay.
        clock: Vec4,
    }

    impl EffectUniform {
        fn new(effects: &Effects, time: &Time<Real>) -> Self {
            Self {
                light: Vec4::new(
                    effects.scanlines,
                    effects.vignette,
                    effects.bloom,
                    effects.noise,
                ),
                geometry: Vec4::new(
                    effects.curvature,
                    effects.pixel_grid,
                    effects.burn_in,
                    effects.pulse,
                ),
                clock: Vec4::new(
                    time.elapsed_secs(),
                    effects.burn_in.powf((time.delta_secs() * 240.0).max(0.001)),
                    0.0,
                    0.0,
                ),
            }
        }
    }

    #[derive(Default)]
    struct PointerState {
        camera: Option<Entity>,
        raw: Option<egui::Pos2>,
        mapped: Option<egui::Pos2>,
    }

    fn prepare_input(
        mut commands: Commands,
        effects: Res<Effects>,
        layouts: Query<Entity, With<ActiveLayout>>,
        mut cameras: Query<(Entity, &mut EguiInput), With<LayoutCamera>>,
        time: Res<Time<Real>>,
        mut pointer: Local<PointerState>,
    ) {
        let scene = layouts.iter().next();
        let curvature = if effects.enabled {
            effects.curvature
        } else {
            0.0
        };
        for (camera, mut input) in &mut cameras {
            if pointer.camera != Some(camera) {
                *pointer = PointerState {
                    camera: Some(camera),
                    ..default()
                };
            }
            if let Some(rect) = input.screen_rect {
                map_events(&mut input, rect, curvature, &mut pointer);
            }
            // Captured before the UI draws: a change made in this frame's UI
            // reaches input and rendering together, next frame.
            commands.entity(camera).insert((
                EffectUniform::new(&effects, &time),
                EffectsFrame {
                    enabled: effects.enabled,
                    scene,
                },
            ));
        }
    }

    /// The shader maps display UV to source UV, so input uses the same function,
    /// not its inverse. Radial symmetry makes this identical in top-origin egui UV.
    fn source_position(position: egui::Pos2, rect: egui::Rect, curvature: f32) -> egui::Pos2 {
        if curvature == 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
            return position;
        }
        let uv = (position - rect.min) / rect.size();
        let center = uv - egui::vec2(0.5, 0.5);
        let warped = egui::vec2(0.5, 0.5) + center * (1.0 + curvature * 0.25 * center.length_sq());
        let uv = warped.clamp(egui::vec2(0.001, 0.001), egui::vec2(0.999, 0.999));
        rect.min + uv * rect.size()
    }

    fn map_events(
        input: &mut egui::RawInput,
        rect: egui::Rect,
        curvature: f32,
        pointer: &mut PointerState,
    ) {
        let mut moved = false;
        for event in &mut input.events {
            match event {
                egui::Event::PointerMoved(position) => {
                    pointer.raw = Some(*position);
                    *position = source_position(*position, rect, curvature);
                    pointer.mapped = Some(*position);
                    moved = true;
                }
                egui::Event::PointerButton { pos, .. } => {
                    pointer.raw = Some(*pos);
                    *pos = source_position(*pos, rect, curvature);
                }
                egui::Event::Touch { pos, .. } => *pos = source_position(*pos, rect, curvature),
                egui::Event::PointerGone => {
                    pointer.raw = None;
                    pointer.mapped = None;
                }
                _ => {}
            }
        }
        // egui retains its last pointer coordinate. Reproject a stationary pointer
        // when curvature or viewport size changes, including when disabled.
        if !moved && let Some(raw) = pointer.raw {
            let mapped = source_position(raw, rect, curvature);
            if pointer.mapped != Some(mapped) {
                input.events.insert(0, egui::Event::PointerMoved(mapped));
                pointer.mapped = Some(mapped);
            }
        }
    }

    #[derive(Resource)]
    struct EffectPipeline {
        layout: BindGroupLayoutDescriptor,
        sampler: Sampler,
        crt: CachedRenderPipelineId,
        burn: CachedRenderPipelineId,
        bloom_downsample: CachedRenderPipelineId,
        bloom_horizontal: CachedRenderPipelineId,
        bloom_vertical: CachedRenderPipelineId,
    }

    const BLOOM_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

    fn prepare_pipeline(
        mut commands: Commands,
        device: Res<RenderDevice>,
        assets: Res<AssetServer>,
        fullscreen: Res<FullscreenShader>,
        cache: Res<PipelineCache>,
    ) {
        let layout = BindGroupLayoutDescriptor::new(
            "bEd effects",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer::<EffectUniform>(true),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
            ),
        );
        let shader = assets.load(
            AssetPath::from_path_buf(embedded_path!("bed", "effects.wesl")).with_source("embedded"),
        );
        let pipeline = |entry: &'static str, format| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(format!("bEd {entry}").into()),
                layout: vec![layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    entry_point: Some(entry.into()),
                    targets: vec![Some(ColorTargetState {
                        format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            })
        };
        commands.insert_resource(EffectPipeline {
            crt: pipeline("crt", TextureFormat::Rgba8UnormSrgb),
            burn: pipeline("burn", TextureFormat::Rgba8UnormSrgb),
            bloom_downsample: pipeline("bloom_downsample", BLOOM_FORMAT),
            bloom_horizontal: pipeline("bloom_horizontal", BLOOM_FORMAT),
            bloom_vertical: pipeline("bloom_vertical", BLOOM_FORMAT),
            layout,
            sampler: device.create_sampler(&SamplerDescriptor {
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..default()
            }),
        });
    }

    #[derive(Component)]
    struct History {
        textures: [Texture; 2],
        views: [TextureView; 2],
        previous: usize,
        valid: bool,
        // Only effect values and visible scene identity invalidate history; unrelated
        // editor/appearance settings and the continuously advancing clock do not.
        values: (Vec4, Vec4),
        scene: Option<Entity>,
    }

    #[derive(Component)]
    struct BloomTextures {
        textures: [Texture; 2],
        views: [TextureView; 2],
    }

    fn bloom_extent(size: Extent3d) -> Extent3d {
        Extent3d {
            width: size.width.div_ceil(2).max(1),
            height: size.height.div_ceil(2).max(1),
            depth_or_array_layers: 1,
        }
    }

    /// A camera's effect state in the render world.
    type EffectCameras<'w, 's> = Query<
        'w,
        's,
        (
            Entity,
            &'static ViewTarget,
            &'static EffectsFrame,
            &'static EffectUniform,
            Option<&'static mut History>,
            Option<&'static BloomTextures>,
        ),
    >;

    fn prepare_targets(
        mut commands: Commands,
        device: Res<RenderDevice>,
        mut cameras: EffectCameras,
    ) {
        for (entity, target, frame, uniform, history, bloom) in &mut cameras {
            let size = target.main_texture().size();
            if !frame.enabled || uniform.light.z == 0.0 {
                if bloom.is_some() {
                    commands.entity(entity).remove::<BloomTextures>();
                }
            } else {
                let size = bloom_extent(size);
                if bloom.is_none_or(|bloom| bloom.textures[0].size() != size) {
                    let textures = std::array::from_fn(|_| {
                        device.create_texture(&TextureDescriptor {
                            label: Some("bEd reduced bloom"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: BLOOM_FORMAT,
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        })
                    });
                    let views = textures
                        .each_ref()
                        .map(|texture| texture.create_view(&TextureViewDescriptor::default()));
                    commands
                        .entity(entity)
                        .insert(BloomTextures { textures, views });
                }
            }
            if !frame.enabled || uniform.geometry.z == 0.0 {
                if history.is_some() {
                    commands.entity(entity).remove::<History>();
                }
                continue;
            }
            let values = (uniform.light, uniform.geometry);
            if let Some(mut history) = history
                && history.textures[0].size() == size
            {
                if history.values != values || history.scene != frame.scene {
                    history.valid = false;
                    history.values = values;
                    history.scene = frame.scene;
                }
                continue;
            }
            let textures = std::array::from_fn(|_| {
                device.create_texture(&TextureDescriptor {
                    label: Some("bEd phosphor history"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba8UnormSrgb,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            });
            let views = textures
                .each_ref()
                .map(|texture| texture.create_view(&TextureViewDescriptor::default()));
            commands.entity(entity).insert(History {
                textures,
                views,
                previous: 0,
                valid: false,
                values,
                scene: frame.scene,
            });
        }
    }

    /// The view being rendered, with its effect state.
    type EffectView<'w, 's> = ViewQuery<
        'w,
        's,
        (
            &'static ViewTarget,
            &'static EffectsFrame,
            &'static DynamicUniformIndex<EffectUniform>,
            Option<&'static mut History>,
            Option<&'static BloomTextures>,
        ),
    >;

    fn render(
        view: EffectView,
        effects: Res<EffectPipeline>,
        cache: Res<PipelineCache>,
        uniforms: Res<ComponentUniforms<EffectUniform>>,
        mut ctx: RenderContext,
    ) {
        let (target, frame, index, history, bloom) = view.into_inner();
        if !frame.enabled || target.main_texture_format() != TextureFormat::Rgba8UnormSrgb {
            return;
        }
        let Some(crt) = cache.get_render_pipeline(effects.crt) else {
            return;
        };
        let Some(binding) = uniforms.uniforms().binding() else {
            return;
        };
        let burn = if history.is_some() {
            let Some(burn) = cache.get_render_pipeline(effects.burn) else {
                return;
            };
            Some(burn)
        } else {
            None
        };
        let bloom_pipelines = if bloom.is_some() {
            let (Some(downsample), Some(horizontal), Some(vertical)) = (
                cache.get_render_pipeline(effects.bloom_downsample),
                cache.get_render_pipeline(effects.bloom_horizontal),
                cache.get_render_pipeline(effects.bloom_vertical),
            ) else {
                return;
            };
            Some((downsample, horizontal, vertical))
        } else {
            None
        };
        // All fallible prerequisites must be resolved before this flips the target.
        let post = target.post_process_write();
        let mut pass = |source: &TextureView,
                        previous: &TextureView,
                        bloom: &TextureView,
                        output: &TextureView,
                        pipeline: &RenderPipeline,
                        label: &'static str| {
            let group = ctx.render_device().create_bind_group(
                label,
                &cache.get_bind_group_layout(&effects.layout),
                &BindGroupEntries::sequential((
                    source,
                    &effects.sampler,
                    binding.clone(),
                    previous,
                    bloom,
                )),
            );
            let mut pass = ctx
                .command_encoder()
                .begin_render_pass(&RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: output,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations::default(),
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[index.index()]);
            pass.draw(0..3, 0..1);
        };
        let source = if let (Some(history), Some(burn)) = (history, burn) {
            let history = history.into_inner();
            let next = 1 - history.previous;
            let previous = if history.valid {
                &history.views[history.previous]
            } else {
                post.source
            };
            pass(
                post.source,
                previous,
                post.source,
                &history.views[next],
                burn,
                "bEd phosphor persistence",
            );
            history.previous = next;
            history.valid = true;
            &history.views[next]
        } else {
            post.source
        };
        let bloom_source = if let (Some(bloom), Some((downsample, horizontal, vertical))) =
            (bloom, bloom_pipelines)
        {
            pass(
                source,
                source,
                source,
                &bloom.views[0],
                downsample,
                "bEd bloom downsample",
            );
            pass(
                &bloom.views[0],
                source,
                source,
                &bloom.views[1],
                horizontal,
                "bEd bloom horizontal",
            );
            pass(
                &bloom.views[1],
                source,
                source,
                &bloom.views[0],
                vertical,
                "bEd bloom vertical",
            );
            &bloom.views[0]
        } else {
            source
        };
        pass(
            source,
            source,
            bloom_source,
            post.destination,
            crt,
            "bEd CRT",
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rect() -> egui::Rect {
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
        }

        #[test]
        fn curvature_bends_pointer_positions_like_the_shader() {
            let rect = rect();
            let corner = egui::pos2(40.0, 30.0);
            // No curvature, or the centre: unchanged.
            assert_eq!(source_position(corner, rect, 0.0), corner);
            let centre = rect.center();
            assert_eq!(source_position(centre, rect, 0.8), centre);
            // Towards the edges the display shows content from further out, as the
            // shader's display→source map does.
            let bent = source_position(corner, rect, 0.8);
            assert!(bent.x < corner.x && bent.y < corner.y, "{bent:?}");
            // Never outside the window.
            let edge = source_position(egui::pos2(0.0, 0.0), rect, 1.0);
            assert!(rect.contains(edge));
        }

        #[test]
        fn a_still_pointer_is_moved_when_curvature_changes() {
            let rect = rect();
            let mut pointer = PointerState::default();
            let mut input = egui::RawInput {
                events: vec![egui::Event::PointerMoved(egui::pos2(40.0, 30.0))],
                ..default()
            };
            map_events(&mut input, rect, 0.0, &mut pointer);
            assert_eq!(
                input.events,
                [egui::Event::PointerMoved(egui::pos2(40.0, 30.0))]
            );

            // No new pointer events, but curvature changed: egui is told where the
            // pointer now lands.
            let mut input = egui::RawInput::default();
            map_events(&mut input, rect, 0.8, &mut pointer);
            let [egui::Event::PointerMoved(moved)] = input.events[..] else {
                panic!("{:?}", input.events);
            };
            assert_eq!(moved, source_position(egui::pos2(40.0, 30.0), rect, 0.8));
            // Unchanged next frame: nothing more to send.
            let mut input = egui::RawInput::default();
            map_events(&mut input, rect, 0.8, &mut pointer);
            assert!(input.events.is_empty());
        }

        #[test]
        fn effects_follow_their_settings_and_stay_in_range() {
            use bedtime_workbench::{WorkbenchCorePlugins, settings::SettingsAppExt};
            let mut app = App::new();
            app.add_plugins((WorkbenchCorePlugins, EffectsPlugin))
                .init_resource::<Time<Real>>()
                .add_settings_layer::<crate::workspace::GlobalStore>();
            app.update();
            let world = app.world_mut();
            assert!(!world.resource::<Effects>().enabled);
            let global = StoreId::of::<crate::workspace::GlobalStore>();
            world
                .settings_mut()
                .set::<EffectsEnabled>(global, true)
                .unwrap();
            world.settings_mut().set::<Curvature>(global, 3.0).unwrap();
            let effects = world.resource::<Effects>();
            assert!(effects.enabled);
            assert_eq!(effects.curvature, 1.0);
            assert_eq!(effects.scanlines, 0.06);
        }
    }
}
