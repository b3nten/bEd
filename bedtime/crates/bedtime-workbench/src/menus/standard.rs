//! The standard menu bar: App, File, Edit, View, the app's own menus,
//! Window, Help. Plugins add their actions to it (`add_menu_item`); the app
//! adds its own the same way, and hides plugins' it doesn't want
//! (`hide_menu_item`). `StandardMenusPlugin` builds the bar at startup.
//!
//! Each menu has the usual groups, in the usual order (File: new, open, save,
//! close); an item goes in a group, and groups are separated by a line.
//! Within a group, items come in the order they were added (plugin order).
//! Groups a menu doesn't know come after its own, in the order first used.
//! Menus with nothing in them aren't shown.
use super::*;
use crate::actions::ActionId;

/// A menu of the standard bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StandardMenu {
    /// The application's menu (macOS puts it first, under the app's name).
    App,
    File,
    Edit,
    View,
    /// One of the app's own, after View (bed's Layout).
    Custom(&'static str),
    Window,
    Help,
}

impl StandardMenu {
    /// Its groups, in order.
    fn groups(self) -> &'static [&'static str] {
        match self {
            StandardMenu::App => &["about", "settings", "quit"],
            StandardMenu::File => &["new", "open", "save", "close"],
            StandardMenu::Edit => &["history", "clipboard", "find", "code"],
            StandardMenu::View => &["panels", "editor", "zoom", "tools"],
            StandardMenu::Custom(_) | StandardMenu::Window | StandardMenu::Help => &[],
        }
    }

    fn label(self, app_name: &str) -> String {
        match self {
            StandardMenu::App => app_name.to_owned(),
            StandardMenu::File => "File".into(),
            StandardMenu::Edit => "Edit".into(),
            StandardMenu::View => "View".into(),
            StandardMenu::Custom(label) => label.into(),
            StandardMenu::Window => "Window".into(),
            StandardMenu::Help => "Help".into(),
        }
    }
}

enum Entry {
    Action {
        action: ActionId,
        label: String,
    },
    /// Something else (a submenu): spawned by the app, then put in place.
    Spawn(fn(&mut World) -> Entity),
}

struct Contribution {
    menu: StandardMenu,
    group: &'static str,
    entry: Entry,
}

/// What plugins and the app put in the standard menus, until the bar is
/// built.
#[derive(Resource, Default)]
pub struct MenuContributions {
    entries: Vec<Contribution>,
    hidden: HashSet<ActionId>,
}

/// The standard menu bar, once built: its root and each menu's entity.
#[derive(Resource, Debug)]
pub struct StandardMenus {
    pub bar: Entity,
    menus: Vec<(StandardMenu, Entity)>,
}

impl StandardMenus {
    pub fn menu(&self, menu: StandardMenu) -> Option<Entity> {
        self.menus
            .iter()
            .find(|(known, _)| *known == menu)
            .map(|(_, entity)| *entity)
    }
}

pub trait MenusAppExt {
    /// Put action `A` in `menu`'s `group`, labelled as the action is.
    fn add_menu_item<A: Action>(&mut self, menu: StandardMenu, group: &'static str) -> &mut Self;
    /// As `add_menu_item`, with its own label ("Settings…").
    fn add_menu_item_labelled<A: Action>(
        &mut self,
        menu: StandardMenu,
        group: &'static str,
        label: impl Into<String>,
    ) -> &mut Self;
    /// Put an entry `spawn` makes (a submenu, say) in `menu`'s `group`.
    fn add_menu_entry(
        &mut self,
        menu: StandardMenu,
        group: &'static str,
        spawn: fn(&mut World) -> Entity,
    ) -> &mut Self;
    /// Leave action `A` out of the standard menus, whoever added it.
    fn hide_menu_item<A: Action>(&mut self) -> &mut Self;
}

impl MenusAppExt for App {
    fn add_menu_item<A: Action>(&mut self, menu: StandardMenu, group: &'static str) -> &mut Self {
        self.add_menu_item_labelled::<A>(menu, group, A::LABEL)
    }

    fn add_menu_item_labelled<A: Action>(
        &mut self,
        menu: StandardMenu,
        group: &'static str,
        label: impl Into<String>,
    ) -> &mut Self {
        let entry = Entry::Action {
            action: ActionId::of::<A>(),
            label: label.into(),
        };
        self.world_mut()
            .get_resource_or_init::<MenuContributions>()
            .entries
            .push(Contribution { menu, group, entry });
        self
    }

    fn add_menu_entry(
        &mut self,
        menu: StandardMenu,
        group: &'static str,
        spawn: fn(&mut World) -> Entity,
    ) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<MenuContributions>()
            .entries
            .push(Contribution {
                menu,
                group,
                entry: Entry::Spawn(spawn),
            });
        self
    }

    fn hide_menu_item<A: Action>(&mut self) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<MenuContributions>()
            .hidden
            .insert(ActionId::of::<A>());
        self
    }
}

/// Builds the standard menu bar (in `PreStartup`, so the app's `Startup`
/// systems find it in `StandardMenus`), shown natively with the `native`
/// feature. `app_name` labels the App menu.
pub struct StandardMenusPlugin {
    pub app_name: &'static str,
}

impl Default for StandardMenusPlugin {
    fn default() -> Self {
        Self { app_name: "App" }
    }
}

#[derive(Resource)]
struct AppName(&'static str);

impl Plugin for StandardMenusPlugin {
    fn build(&self, app: &mut App) {
        use crate::*;
        app.init_resource::<MenuContributions>()
            .insert_resource(AppName(self.app_name))
            .add_systems(PreStartup, compose)
            // Workbench's own.
            .add_menu_item::<New>(StandardMenu::File, "new")
            .add_menu_item::<SaveDocument>(StandardMenu::File, "save")
            .add_menu_item::<ClosePanel>(StandardMenu::File, "close")
            .add_menu_item::<UndoDocument>(StandardMenu::Edit, "history")
            .add_menu_item::<RedoDocument>(StandardMenu::Edit, "history")
            .add_menu_item::<ZoomIn>(StandardMenu::View, "zoom")
            .add_menu_item::<ZoomOut>(StandardMenu::View, "zoom")
            .add_menu_item::<ResetZoom>(StandardMenu::View, "zoom");
    }
}

fn compose(world: &mut World) {
    let app_name = world.resource::<AppName>().0;
    let contributions = world
        .remove_resource::<MenuContributions>()
        .unwrap_or_default();
    let mut bar = world.spawn(MenuBar);
    #[cfg(feature = "native")]
    bar.insert(NativeMenuBar);
    let bar = bar.id();
    // The menus in order: standard ones, the app's own after View.
    let mut order = vec![
        StandardMenu::App,
        StandardMenu::File,
        StandardMenu::Edit,
        StandardMenu::View,
    ];
    for contribution in &contributions.entries {
        if let StandardMenu::Custom(_) = contribution.menu
            && !order.contains(&contribution.menu)
        {
            order.push(contribution.menu);
        }
    }
    order.extend([StandardMenu::Window, StandardMenu::Help]);
    let mut menus = Vec::new();
    for menu in order {
        let entity = world
            .spawn((
                Menu {
                    label: menu.label(app_name),
                },
                InMenu(bar),
            ))
            .id();
        menus.push((menu, entity));
        // Its groups: its own, then others in the order first used.
        let mut groups: Vec<&'static str> = menu.groups().to_vec();
        for contribution in contributions.entries.iter().filter(|c| c.menu == menu) {
            if !groups.contains(&contribution.group) {
                groups.push(contribution.group);
            }
        }
        let mut first = true;
        for group in groups {
            let entries: Vec<&Contribution> = contributions
                .entries
                .iter()
                .filter(|c| c.menu == menu && c.group == group)
                .filter(|c| match &c.entry {
                    Entry::Action { action, .. } => !contributions.hidden.contains(action),
                    Entry::Spawn(_) => true,
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            if !first {
                world.spawn((MenuSeparator, InMenu(entity)));
            }
            first = false;
            for contribution in entries {
                match &contribution.entry {
                    Entry::Action { action, label } => {
                        world.spawn((
                            MenuItem::new(label.clone()),
                            Invokes(action.clone()),
                            InMenu(entity),
                        ));
                    }
                    Entry::Spawn(spawn) => {
                        let spawned = spawn(world);
                        world.entity_mut(spawned).insert(InMenu(entity));
                    }
                }
            }
        }
    }
    world.insert_resource(StandardMenus { bar, menus });
}
