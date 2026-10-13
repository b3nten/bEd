//! Editor-free panel and command contributions for desktop workbenches.
pub mod status;
pub use bed_keybindings::{Action as ShortcutAction, Scope as ShortcutScope, ShortcutInput};
use bevy::{
    ecs::system::{BoxedSystem, SystemId},
    prelude::*,
};
use dear_imgui_rs::Ui;
use serde_json::Value;
pub use status::{
    StatusCommand, StatusContext, StatusDrawContext, StatusDrawMode, StatusItem, StatusItems,
    StatusText,
};
use std::collections::HashSet;

/// Native pinch facts for the current UI frame. Presentations capture the
/// gesture beneath its starting position; the compositor clears these after
/// every panel and overlay has read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, Debug)]
pub struct PinchEvent {
    pub position: [f32; 2],
    pub delta: f64,
    pub phase: PinchPhase,
}

#[derive(Resource, Default)]
pub struct PinchEvents(pub Vec<PinchEvent>);

/// Saved panel numbers are independent of the entity's live identity.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct PanelPresentation {
    pub id: u64,
    pub focus: bool,
    pub dock_next: bool,
    pub viewport: u32,
    pub dock_id: u32,
}

impl PanelPresentation {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            focus: true,
            dock_next: true,
            viewport: 0,
            dock_id: 0,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn focus_requested(&self) -> bool {
        self.focus
    }

    pub fn viewport_id(&self) -> u32 {
        self.viewport
    }
}

/// Omit transient panels, such as a startup launcher, from saved layouts.
/// Panels without this component are persisted.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelPersistence(pub bool);

/// Retain a panel and its services while excluding it from the active layout.
/// The host owns suspension; feature plugins keep their ordinary lifecycle.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelSuspended;

pub fn panel_active(world: &World, entity: Entity) -> bool {
    panel_owner(world, entity).is_some_and(|owner| world.get::<PanelSuspended>(owner).is_none())
}

/// Content padding requested by this presentation. The compositor owns its scope.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PanelPadding(pub [f32; 2]);

/// Usage notes published on a panel's frontend. Shortcut labels and assignments
/// come from the live keymap, rather than being repeated in this prose.
#[derive(Component, Clone, Debug, Default)]
pub struct PanelDocumentation {
    /// Last selected symbol, including its LSP markup and syntax palette.
    pub symbol: Option<bed_ui::documentation::Documentation>,
    pub sections: Vec<DocumentationSection>,
    /// Additional widget scopes handled by this frontend (such as a text editor).
    pub shortcut_scopes: Vec<String>,
    /// Individual action IDs supported by this frontend, including shared focused actions.
    pub shortcut_actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DocumentationSection {
    pub title: String,
    pub body: String,
}

impl PanelDocumentation {
    pub fn new(sections: &[(&str, &str)]) -> Self {
        Self {
            sections: sections
                .iter()
                .map(|(title, body)| DocumentationSection {
                    title: (*title).into(),
                    body: (*body).into(),
                })
                .collect(),
            ..Default::default()
        }
    }

    pub fn shortcut_scope(mut self, scope: &str) -> Self {
        self.shortcut_scopes.push(scope.into());
        self
    }

    pub fn shortcut_actions(mut self, actions: &[&str]) -> Self {
        self.shortcut_actions = actions.iter().map(|id| (*id).into()).collect();
        self
    }
}

/// Observed content bounds for the current UI frame, in logical screen pixels.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelGeometry {
    pub content_min: [f32; 2],
    pub content_max: [f32; 2],
    pub clip_min: [f32; 2],
    pub clip_max: [f32; 2],
    pub visible: bool,
}

/// The active frontend of a layout panel. Retained, inactive viewers remain
/// owned by PanelOwner; only this relationship chooses what is drawn.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelFrontend(pub Entity);

/// A retained frontend presentation belongs to one live layout panel. The
/// presentation keeps feature components while an inactive viewer is hidden.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelOwner(pub Entity);

/// Resolve a frontend source to its layout panel without changing the source
/// passed to a feature's registered system.
pub fn panel_owner(world: &World, entity: Entity) -> Option<Entity> {
    if world.get::<PanelPresentation>(entity).is_some() {
        Some(entity)
    } else {
        world
            .get::<PanelOwner>(entity)
            .map(|owner| owner.0)
            .filter(|owner| world.get::<PanelPresentation>(*owner).is_some())
    }
}

/// An application-wide popup contribution drawn inside the frontend phase.
/// Plugins install one component; consequential effects still use request queues.
#[derive(Component, Clone, Copy)]
pub struct WorkbenchOverlay(pub fn(&mut World, &Ui));

#[derive(Component, Clone, Debug)]
pub struct OverlayError(pub String);

/// An ImGui payload placed through the workspace's tab and split targets.
/// Features resolve the payload into intentions after all content has drawn.
#[derive(Message, Clone, Debug)]
pub struct PanelDrop {
    pub area: u32,
    pub panel: Option<Entity>,
    /// Tab insertion position; `None` appends to the destination area.
    pub tab_index: Option<usize>,
    pub payload_type: String,
    pub data: Vec<u8>,
}

/// Floating label shared by workspace tabs and sources of new panel content.
pub fn draw_panel_drag_label(ui: &Ui, title: &str) {
    let mouse = ui.io().mouse_pos();
    let min = [mouse[0] + 14.0, mouse[1] + 14.0];
    let max = [
        min[0] + ui.calc_text_size(title)[0] + 16.0,
        min[1] + ui.text_line_height() + 10.0,
    ];
    let draw = ui.get_foreground_draw_list();
    draw.add_rect(min, max, ui.style_color(dear_imgui_rs::StyleColor::PopupBg))
        .rounding(3.0)
        .filled(true)
        .build();
    draw.add_rect(min, max, ui.style_color(dear_imgui_rs::StyleColor::Border))
        .rounding(3.0)
        .build();
    draw.add_text(
        [min[0] + 8.0, min[1] + 5.0],
        ui.style_color(dear_imgui_rs::StyleColor::Text),
        title,
    );
}

/// A frontend object is main-thread state. Its entity and feature resources
/// live in the world; drawing may update widget state and returns explicit
/// intentions. Commands and panel lifecycle run after the frontend returns.
pub trait Panel: 'static {
    fn title(&self, world: &World) -> String;

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<Action>,
        shortcuts: &ShortcutInput,
    );

    fn save(&self, _world: &World) -> Value {
        Value::Null
    }
}

#[derive(Clone, Debug)]
pub struct PanelCreation {
    pub entity: Entity,
    pub state: Value,
}

/// Captured at the originating surface. Dispatch never substitutes later focus.
#[derive(Clone, Debug)]
pub struct CommandInvocation {
    pub command: String,
    pub source: Option<Entity>,
    pub area: u32,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub enum Action {
    /// Focus the most recently used matching panel, or create one.
    ShowPanel {
        panel_type: String,
        state: Value,
        area: Option<u32>,
    },
    OpenPanel {
        panel_type: String,
        state: Value,
        area: Option<u32>,
    },
    ClosePanel(Entity),
    /// Retain the owning tab if it was opened as a replaceable preview.
    KeepPanel(Entity),
    FocusPanel(Entity),
    ObserveFocus(Entity),
    FocusArea(u32),
    /// Replace area geometry/membership after the listed captured panels close.
    SetLayout {
        tiling: Value,
        close: Vec<Entity>,
    },
    SplitPanel {
        panel: Entity,
        axis: usize,
        high: bool,
    },
    Invoke(CommandInvocation),
}

pub type CommandResult = Result<Vec<Action>, String>;

pub type PanelResult = Result<Box<dyn Panel>, String>;

pub type CommandSystem = SystemId<In<CommandInvocation>, CommandResult>;

pub type PanelSystem = SystemId<In<PanelCreation>, PanelResult>;

#[derive(Clone)]
pub struct CommandDefinition {
    pub id: &'static str,
    pub owner: &'static str,
    pub label: &'static str,
    pub handler: CommandSystem,
}

#[derive(Clone)]
pub struct PanelDefinition {
    pub id: &'static str,
    pub owner: &'static str,
    pub label: &'static str,
    pub factory: PanelSystem,
}

/// Ordered definitions, published only by a validated registration transaction.
#[derive(Resource)]
pub struct Registry {
    owners: HashSet<&'static str>,
    commands: Vec<CommandDefinition>,
    panels: Vec<PanelDefinition>,
    menus: Vec<&'static str>,
    shortcuts: Vec<ShortcutAction>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            owners: HashSet::new(),
            commands: Vec::new(),
            panels: Vec::new(),
            menus: Vec::new(),
            shortcuts: bed_keybindings::common_actions(),
        }
    }
}

impl Registry {
    pub fn shortcuts(&self) -> &[ShortcutAction] {
        &self.shortcuts
    }

    pub fn commands(&self) -> &[CommandDefinition] {
        &self.commands
    }

    pub fn panels(&self) -> &[PanelDefinition] {
        &self.panels
    }

    pub fn menus(&self) -> &[&'static str] {
        &self.menus
    }

    pub fn command(&self, id: &str) -> Option<&CommandDefinition> {
        self.commands.iter().find(|entry| entry.id == id)
    }

    pub fn panel(&self, id: &str) -> Option<&PanelDefinition> {
        self.panels.iter().find(|entry| entry.id == id)
    }

    fn ids(&self) -> impl Iterator<Item = &str> {
        self.commands
            .iter()
            .map(|entry| entry.id)
            .chain(self.panels.iter().map(|entry| entry.id))
    }
}

struct PendingCommand {
    id: &'static str,
    label: &'static str,
    system: BoxedSystem<In<CommandInvocation>, CommandResult>,
}

struct PendingPanel {
    id: &'static str,
    label: &'static str,
    system: BoxedSystem<In<PanelCreation>, PanelResult>,
}

#[derive(Default)]
pub struct Registrar {
    commands: Vec<PendingCommand>,
    panels: Vec<PendingPanel>,
    menus: Vec<&'static str>,
    shortcuts: Vec<ShortcutAction>,
}

impl Registrar {
    pub fn command<M>(
        &mut self,
        id: &'static str,
        label: &'static str,
        system: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) {
        self.commands.push(PendingCommand {
            id,
            label,
            system: Box::new(IntoSystem::into_system(system)),
        });
    }

    pub fn panel<M>(
        &mut self,
        id: &'static str,
        label: &'static str,
        system: impl IntoSystem<In<PanelCreation>, PanelResult, M> + 'static,
    ) {
        self.panels.push(PendingPanel {
            id,
            label,
            system: Box::new(IntoSystem::into_system(system)),
        });
    }

    /// Attach defaults to an application-wide command. Every registered command
    /// is bindable even when this method is not called.
    pub fn global_shortcut(&mut self, command: &'static str, defaults: &[&str]) {
        self.shortcuts.push(ShortcutAction::new(
            command,
            "",
            "",
            ShortcutScope::Global,
            defaults,
        ));
    }

    /// A frontend handles these action IDs using its scoped ShortcutInput.
    pub fn panel_shortcut(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel: &'static str,
        defaults: &[&str],
    ) {
        self.shortcuts.push(ShortcutAction::new(
            id,
            label,
            "",
            ShortcutScope::panel(panel),
            defaults,
        ));
    }

    pub fn shortcut_action(&mut self, action: ShortcutAction) {
        self.shortcuts.push(action);
    }

    pub fn menu(&mut self, command: &'static str) {
        self.menus.push(command);
    }
}

/// Validate before allocating registered systems or publishing any definition.
pub fn register(
    world: &mut World,
    owner: &'static str,
    build: impl FnOnce(&mut Registrar),
) -> Result<(), String> {
    let mut pending = Registrar::default();
    build(&mut pending);
    let registry = world.get_resource::<Registry>();
    if registry.is_some_and(|registry| registry.owners.contains(owner)) {
        return Err(format!("Module already registered: {owner}"));
    }
    validate_contribution_ids(
        owner,
        pending
            .commands
            .iter()
            .map(|entry| entry.id)
            .chain(pending.panels.iter().map(|entry| entry.id)),
        registry.into_iter().flat_map(Registry::ids),
    )?;
    for command in &pending.menus {
        if !pending.commands.iter().any(|entry| entry.id == *command) {
            return Err(format!("Missing contributed command: {command}"));
        }
    }
    let mut shortcut_ids = HashSet::new();
    for shortcut in &mut pending.shortcuts {
        if !valid_id(&shortcut.id)
            || !shortcut.id.starts_with(&format!("{owner}."))
            || !shortcut_ids.insert(shortcut.id.clone())
        {
            return Err(format!(
                "Invalid or duplicate shortcut action: {}",
                shortcut.id
            ));
        }
        if registry.is_some_and(|r| r.shortcuts.iter().any(|a| a.id == shortcut.id)) {
            return Err(format!(
                "Shortcut action already registered: {}",
                shortcut.id
            ));
        }
        if shortcut
            .defaults
            .iter()
            .any(|stroke| bed_keybindings::key_name(stroke.key).is_none())
        {
            return Err(format!("Invalid shortcut key in {}", shortcut.id));
        }
        if let ShortcutScope::Panel(panel) = &shortcut.scope {
            if !pending.panels.iter().any(|p| p.id == panel)
                && !registry.is_some_and(|r| r.panel(panel).is_some())
            {
                return Err(format!("Missing shortcut panel: {panel}"));
            }
        } else {
            let command = pending
                .commands
                .iter()
                .find(|c| c.id == shortcut.id)
                .ok_or_else(|| format!("Missing shortcut command: {}", shortcut.id))?;
            shortcut.label = command.label.into();
        }
        shortcut.owner = owner.into();
    }
    for command in &pending.commands {
        if !shortcut_ids.contains(command.id) {
            pending.shortcuts.push(ShortcutAction::new(
                command.id,
                command.label,
                owner,
                ShortcutScope::Global,
                &[],
            ));
        }
    }
    let commands = pending
        .commands
        .into_iter()
        .map(|entry| CommandDefinition {
            id: entry.id,
            owner,
            label: entry.label,
            handler: world.register_boxed_system(entry.system),
        })
        .collect::<Vec<_>>();
    let panels = pending
        .panels
        .into_iter()
        .map(|entry| PanelDefinition {
            id: entry.id,
            owner,
            label: entry.label,
            factory: world.register_boxed_system(entry.system),
        })
        .collect::<Vec<_>>();
    world.init_resource::<Registry>();
    let mut registry = world.resource_mut::<Registry>();
    registry.commands.extend(commands);
    registry.panels.extend(panels);
    registry.menus.extend(pending.menus);
    registry.shortcuts.extend(pending.shortcuts);
    registry.owners.insert(owner);
    Ok(())
}

/// Shared by generic and editor-specific contribution validation.
pub fn valid_id(id: &str) -> bool {
    id.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

pub fn validate_contribution_ids<'a>(
    owner: &str,
    pending: impl IntoIterator<Item = &'a str>,
    existing: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    if !valid_id(owner) {
        return Err(format!("Invalid module ID: {owner}"));
    }
    let namespace = format!("{owner}.");
    let existing = existing.into_iter().collect::<HashSet<_>>();
    let mut ids = HashSet::new();
    for id in pending {
        if !valid_id(id) || !id.starts_with(&namespace) || !ids.insert(id) {
            return Err(format!("Invalid or duplicate contribution ID: {id}"));
        }
        if existing.contains(id) {
            return Err(format!("Contribution already registered: {id}"));
        }
    }
    Ok(())
}

/// One application input owner; hosts publish overrides before the frontend.
#[derive(Default)]
pub struct ShortcutRuntime {
    pub overrides: bed_keybindings::Overrides,
    pub input: ShortcutInput,
    pub recording: Option<Entity>,
    pub focus: Option<(Entity, u32)>,
    pub prepared_frame: Option<usize>,
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn command(In(_): In<CommandInvocation>) -> CommandResult {
        Ok(Vec::new())
    }

    fn panel(In(_): In<PanelCreation>) -> PanelResult {
        Err("fixture".into())
    }

    #[test]
    fn plugins_expose_global_and_panel_actions_and_registration_is_atomic() {
        let mut world = World::new();
        register(&mut world, "fixture", |r| {
            r.panel("fixture.panel", "Fixture", panel);
            r.command("fixture.show", "Show Fixture", command);
            r.command("fixture.other", "Other", command);
            r.global_shortcut("fixture.show", &["Primary+P", "Ctrl+Alt+F12"]);
            r.panel_shortcut("fixture.next", "Next", "fixture.panel", &["Down"]);
        })
        .unwrap();
        let registry = world.resource::<Registry>();
        let shortcut_count = registry.shortcuts().len();
        assert_eq!(shortcut_count, bed_keybindings::common_actions().len() + 3);
        let show = registry
            .shortcuts()
            .iter()
            .find(|a| a.id == "fixture.show")
            .unwrap();
        assert_eq!(show.defaults.len(), 2);
        assert_eq!(show.label, "Show Fixture");
        assert_eq!(show.owner, "fixture");
        assert!(
            registry
                .shortcuts()
                .iter()
                .find(|a| a.id == "fixture.other")
                .unwrap()
                .defaults
                .is_empty()
        );
        assert_eq!(
            registry
                .shortcuts()
                .iter()
                .find(|a| a.id == "fixture.next")
                .unwrap()
                .scope,
            ShortcutScope::panel("fixture.panel")
        );
        for bad in [0, 1, 2] {
            assert!(
                register(&mut world, "invalid", |r| {
                    r.command("invalid.show", "Show", command);
                    match bad {
                        0 => r.global_shortcut("invalid.missing", &["F5"]),
                        1 => r.panel_shortcut("invalid.next", "Next", "missing.panel", &["F5"]),
                        _ => {
                            r.global_shortcut("invalid.show", &["F5"]);
                            r.global_shortcut("invalid.show", &["F6"]);
                        }
                    }
                })
                .is_err()
            );
            assert!(
                world
                    .resource::<Registry>()
                    .command("invalid.show")
                    .is_none()
            );
            assert_eq!(
                world.resource::<Registry>().shortcuts().len(),
                shortcut_count
            );
        }
    }
}
