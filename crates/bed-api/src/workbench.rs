//! Panels, commands, registration and presentation for desktop workbenches.
pub use bed_keybindings::{Scope as ShortcutScope, ShortcutAction, ShortcutInput};
use bevy::{
    ecs::system::{BoxedSystem, SystemId},
    prelude::*,
};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Current command facts, published by the owning feature's preparation systems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPresentation {
    pub label: Option<String>,
    pub enabled: bool,
    pub visible: bool,
}

impl Default for CommandPresentation {
    fn default() -> Self {
        Self {
            label: None,
            enabled: true,
            visible: true,
        }
    }
}

#[derive(Resource, Default)]
pub struct CommandPresentations(pub HashMap<String, CommandPresentation>);

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
        requests: &mut Vec<WorkbenchAction>,
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

/// Bytes supplied by a local shell. Command owners validate argument semantics;
/// terminal transport preserves the calling directory and originating pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellInvocation {
    pub arguments: Vec<Vec<u8>>,
    pub directory: Vec<u8>,
    pub pane: Option<String>,
}
impl ShellInvocation {
    pub fn to_arguments(&self) -> Value {
        serde_json::json!({"shell":{"arguments":self.arguments,"directory":self.directory,"pane":self.pane}})
    }
    pub fn from_arguments(arguments: &Value) -> Result<Self, String> {
        let shell = arguments.get("shell").ok_or("Missing shell invocation")?;
        let arguments = serde_json::from_value(shell["arguments"].clone())
            .map_err(|_| "Invalid shell argument bytes")?;
        let directory = serde_json::from_value(shell["directory"].clone())
            .map_err(|_| "Invalid shell directory bytes")?;
        let pane =
            serde_json::from_value(shell["pane"].clone()).map_err(|_| "Invalid shell pane")?;
        Ok(Self {
            arguments,
            directory,
            pane,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellAlias {
    pub alias: &'static str,
    pub command: &'static str,
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
pub enum WorkbenchAction {
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

pub type CommandResult = Result<Vec<WorkbenchAction>, String>;

pub type PanelResult = Result<Box<dyn Panel>, String>;

pub type CommandSystem = SystemId<In<CommandInvocation>, CommandResult>;

pub type PanelSystem = SystemId<In<PanelCreation>, PanelResult>;

#[derive(Clone)]
pub struct CommandDefinition {
    pub id: &'static str,
    pub owner: &'static str,
    pub label: &'static str,
    pub icon: Option<&'static str>,
    pub handler: CommandSystem,
}

/// Resolve static contribution metadata and the feature's current command facts.
pub struct PresentedCommand<'a> {
    pub id: &'static str,
    pub label: &'a str,
    pub icon: Option<&'static str>,
    pub enabled: bool,
    pub visible: bool,
}

impl CommandDefinition {
    pub fn presentation<'a>(
        &self,
        facts: Option<&'a CommandPresentations>,
    ) -> PresentedCommand<'a> {
        let fact = facts.and_then(|facts| facts.0.get(self.id));
        PresentedCommand {
            id: self.id,
            label: fact
                .and_then(|fact| fact.label.as_deref())
                .unwrap_or(self.label),
            icon: self.icon,
            enabled: fact.is_none_or(|fact| fact.enabled),
            visible: fact.is_none_or(|fact| fact.visible),
        }
    }
}

#[derive(Clone)]
pub struct PanelDefinition {
    pub id: &'static str,
    pub owner: &'static str,
    pub label: &'static str,
    pub icon: Option<&'static str>,
    pub factory: PanelSystem,
}

/// Ordered definitions, published only by a validated registration transaction.
#[derive(Resource)]
pub struct WorkbenchRegistry {
    owners: HashSet<&'static str>,
    commands: Vec<CommandDefinition>,
    panels: Vec<PanelDefinition>,
    menus: Vec<&'static str>,
    toolbar: Vec<&'static str>,
    shortcuts: Vec<ShortcutAction>,
    shell_aliases: Vec<ShellAlias>,
}

impl Default for WorkbenchRegistry {
    fn default() -> Self {
        Self {
            owners: HashSet::new(),
            commands: Vec::new(),
            panels: Vec::new(),
            menus: Vec::new(),
            toolbar: Vec::new(),
            shortcuts: bed_keybindings::common_actions(),
            shell_aliases: Vec::new(),
        }
    }
}

impl WorkbenchRegistry {
    pub fn shell_aliases(&self) -> &[ShellAlias] {
        &self.shell_aliases
    }
    pub fn shell_command(&self, alias: &str) -> Option<&CommandDefinition> {
        self.shell_aliases
            .iter()
            .find(|entry| entry.alias == alias)
            .and_then(|entry| self.command(entry.command))
    }

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

    pub fn toolbar(&self) -> &[&'static str] {
        &self.toolbar
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
    icon: Option<&'static str>,
    system: BoxedSystem<In<CommandInvocation>, CommandResult>,
}

struct PendingPanel {
    id: &'static str,
    label: &'static str,
    icon: Option<&'static str>,
    system: BoxedSystem<In<PanelCreation>, PanelResult>,
}

#[derive(Default)]
pub struct WorkbenchRegistrar {
    commands: Vec<PendingCommand>,
    panels: Vec<PendingPanel>,
    menus: Vec<&'static str>,
    toolbar: Vec<&'static str>,
    command_icons: Vec<(&'static str, &'static str)>,
    panel_icons: Vec<(&'static str, &'static str)>,
    shortcuts: Vec<ShortcutAction>,
    shell_aliases: Vec<ShellAlias>,
}

impl WorkbenchRegistrar {
    /// Expose an owned workbench command through the local shell bridge.
    pub fn shell_alias(&mut self, alias: &'static str, command: &'static str) {
        self.shell_aliases.push(ShellAlias { alias, command });
    }

    pub fn command<M>(
        &mut self,
        id: &'static str,
        label: &'static str,
        system: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) {
        self.commands.push(PendingCommand {
            id,
            label,
            icon: None,
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
            icon: None,
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

    pub fn toolbar(&mut self, command: &'static str) {
        self.toolbar.push(command);
    }

    pub fn command_icon(&mut self, command: &'static str, icon: &'static str) {
        self.command_icons.push((command, icon));
    }

    pub fn panel_icon(&mut self, panel: &'static str, icon: &'static str) {
        self.panel_icons.push((panel, icon));
    }
}

/// Validate before allocating registered systems or publishing any definition.
pub fn register(
    world: &mut World,
    owner: &'static str,
    build: impl FnOnce(&mut WorkbenchRegistrar),
) -> Result<(), String> {
    let mut pending = WorkbenchRegistrar::default();
    build(&mut pending);
    let registry = world.get_resource::<WorkbenchRegistry>();
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
        registry.into_iter().flat_map(WorkbenchRegistry::ids),
    )?;
    for command in pending.menus.iter().chain(&pending.toolbar) {
        if !pending.commands.iter().any(|entry| entry.id == *command) {
            return Err(format!("Missing contributed command: {command}"));
        }
    }
    let mut aliases = HashSet::new();
    for entry in &pending.shell_aliases {
        if !valid_shell_alias(entry.alias)
            || !aliases.insert(entry.alias)
            || registry.is_some_and(|registry| {
                registry
                    .shell_aliases
                    .iter()
                    .any(|existing| existing.alias == entry.alias)
            })
        {
            return Err(format!("Invalid or duplicate shell alias: {}", entry.alias));
        }
        if !pending
            .commands
            .iter()
            .any(|command| command.id == entry.command)
        {
            return Err(format!(
                "Missing contributed shell command: {}",
                entry.command
            ));
        }
    }
    let mut toolbar_ids = HashSet::new();
    for command in &pending.toolbar {
        if !toolbar_ids.insert(command) {
            return Err(format!("Duplicate toolbar command: {command}"));
        }
    }
    let mut icon_ids = HashSet::new();
    for &(id, icon) in &pending.command_icons {
        if !valid_id(icon) || !icon_ids.insert(id) {
            return Err(format!("Invalid or duplicate command icon: {id} ({icon})"));
        }
        let command = pending
            .commands
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("Missing contributed command: {id}"))?;
        command.icon = Some(icon);
    }
    icon_ids.clear();
    for &(id, icon) in &pending.panel_icons {
        if !valid_id(icon) || !icon_ids.insert(id) {
            return Err(format!("Invalid or duplicate panel icon: {id} ({icon})"));
        }
        let panel = pending
            .panels
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("Missing contributed panel: {id}"))?;
        panel.icon = Some(icon);
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
            icon: entry.icon,
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
            icon: entry.icon,
            factory: world.register_boxed_system(entry.system),
        })
        .collect::<Vec<_>>();
    world.init_resource::<WorkbenchRegistry>();
    let mut registry = world.resource_mut::<WorkbenchRegistry>();
    registry.commands.extend(commands);
    registry.panels.extend(panels);
    registry.menus.extend(pending.menus);
    registry.toolbar.extend(pending.toolbar);
    registry.shortcuts.extend(pending.shortcuts);
    registry.shell_aliases.extend(pending.shell_aliases);
    registry.owners.insert(owner);
    Ok(())
}

pub fn valid_shell_alias(alias: &str) -> bool {
    alias.strip_prefix('+').is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 63
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

/// Shared by workbench and feature contribution validation.
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
    fn shell_registration_is_owned_unique_and_atomic() {
        let mut world = World::new();
        for invalid in ["+../outside", "name", "+", "+name/other"] {
            assert!(
                register(&mut world, "example", |r| {
                    r.command("example.open", "Open", command);
                    r.shell_alias(invalid, "example.open");
                })
                .is_err()
            );
            assert!(!world.contains_resource::<WorkbenchRegistry>());
        }
        assert!(
            register(&mut world, "example", |r| r
                .shell_alias("+name", "example.missing"))
            .is_err()
        );
        register(&mut world, "example", |r| {
            r.command("example.open", "Open", command);
            r.shell_alias("+name", "example.open");
            r.shell_alias("+n", "example.open");
        })
        .unwrap();
        assert_eq!(
            world
                .resource::<WorkbenchRegistry>()
                .shell_command("+name")
                .unwrap()
                .owner,
            "example"
        );
        assert!(
            register(&mut world, "other", |r| {
                r.command("other.open", "Other", command);
                r.shell_alias("+name", "other.open");
            })
            .is_err()
        );
        assert!(
            world
                .resource::<WorkbenchRegistry>()
                .command("other.open")
                .is_none()
        );
        let invocation = ShellInvocation {
            arguments: vec![b"non-utf8\xff".to_vec()],
            directory: b"/tmp/root\xff".to_vec(),
            pane: Some("pane".into()),
        };
        assert_eq!(
            ShellInvocation::from_arguments(&invocation.to_arguments()).unwrap(),
            invocation
        );
    }

    #[test]
    fn shared_icons_toolbar_and_availability_belong_to_one_contribution() {
        let mut world = World::new();
        register(&mut world, "example", |r| {
            r.panel("example.panel", "Example", panel);
            r.command("example.open", "Open Example", command);
            r.panel_icon("example.panel", "example.symbol");
            r.command_icon("example.open", "example.symbol");
            r.toolbar("example.open");
            r.menu("example.open");
        })
        .unwrap();
        let registry = world.resource::<WorkbenchRegistry>();
        assert_eq!(
            registry.panel("example.panel").unwrap().icon,
            Some("example.symbol")
        );
        let open = registry.command("example.open").unwrap();
        assert_eq!(open.label, "Open Example");
        assert_eq!(open.icon, Some("example.symbol"));
        assert_eq!(registry.toolbar(), &["example.open"]);
        assert_eq!(registry.menus(), &["example.open"]);
        world.init_resource::<CommandPresentations>();
        world.resource_mut::<CommandPresentations>().0.insert(
            "example.open".into(),
            CommandPresentation {
                label: Some("Example is busy".into()),
                enabled: false,
                visible: true,
            },
        );
        let presented = world
            .resource::<WorkbenchRegistry>()
            .command("example.open")
            .unwrap()
            .presentation(world.get_resource::<CommandPresentations>());
        assert_eq!(presented.label, "Example is busy");
        assert_eq!(presented.icon, Some("example.symbol"));
        assert!(!presented.enabled);
        assert!(presented.visible);
    }

    #[test]
    fn invalid_icons_and_toolbar_references_leave_registration_unchanged() {
        let mut world = World::new();
        for invalid in 0..5 {
            let result = register(&mut world, "example", |r| {
                r.command("example.open", "Open Example", command);
                match invalid {
                    0 => r.command_icon("example.missing", "example.symbol"),
                    1 => r.panel_icon("example.missing", "example.symbol"),
                    2 => r.command_icon("example.open", "../image.svg"),
                    3 => r.toolbar("example.missing"),
                    _ => {
                        r.toolbar("example.open");
                        r.toolbar("example.open");
                    }
                }
            });
            assert!(result.is_err());
            assert!(!world.contains_resource::<WorkbenchRegistry>());
        }
        register(&mut world, "example", |r| {
            r.command("example.open", "Open Example", command);
            r.toolbar("example.open");
        })
        .unwrap();
        assert_eq!(world.resource::<WorkbenchRegistry>().commands().len(), 1);
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
        let registry = world.resource::<WorkbenchRegistry>();
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
                    .resource::<WorkbenchRegistry>()
                    .command("invalid.show")
                    .is_none()
            );
            assert_eq!(
                world.resource::<WorkbenchRegistry>().shortcuts().len(),
                shortcut_count
            );
        }
    }
}

/// Actual feature work state for this presentation; absence means synchronous Ready.
#[derive(bevy::prelude::Component, Clone, Debug, PartialEq, Eq)]
pub enum PanelReadiness {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum ClipboardAction {
    Copy,
    Cut,
    Paste(String),
    SelectAll,
}

#[derive(Clone, Debug)]
pub struct ClipboardInvocation {
    pub entity: bevy::prelude::Entity,
    pub action: ClipboardAction,
}

pub type ClipboardResult = Result<Option<String>, String>;

pub type PanelClipboardSystem =
    bevy::ecs::system::SystemId<bevy::prelude::In<ClipboardInvocation>, ClipboardResult>;

#[derive(bevy::prelude::Component, Clone, Copy)]
pub struct PanelClipboard(pub PanelClipboardSystem);

pub type PanelLifecycleSystem =
    bevy::ecs::system::SystemId<bevy::prelude::In<bevy::prelude::Entity>, Result<(), String>>;

#[derive(bevy::prelude::Component, Clone, Copy)]
pub struct PanelFocus(pub PanelLifecycleSystem);

#[derive(bevy::prelude::Component, Clone, Copy)]
pub struct PanelClose(pub PanelLifecycleSystem);

/// Native file picking stays with the application host, not feature panels.
pub trait FileDialogService {
    fn pick_file(&mut self, directory: &Path, extensions: &[&str]) -> Option<PathBuf>;
}

/// Native file-list clipboard access supplied by the desktop host.
/// Clipboard imports always copy. Local cuts publish file references too;
/// move intent is retained only by the workbench.
pub trait FileClipboardService {
    fn read_files(&mut self) -> std::io::Result<Vec<PathBuf>>;

    fn write_files(&mut self, paths: &[PathBuf]) -> std::io::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalFileDragPhase {
    Hover,
    Drop,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct ExternalFileDrag {
    pub paths: Vec<PathBuf>,
    /// Desktop logical coordinates, matching panel rectangles.
    pub position: [f32; 2],
    pub viewport: u32,
    pub phase: ExternalFileDragPhase,
    pub modifiers: ExternalFileModifiers,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalFileModifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExternalFileDropResponse {
    #[default]
    Ignored,
    Accepted,
}

#[derive(Clone, Copy, Debug)]
pub struct PanelActionInvocation {
    pub entity: bevy::prelude::Entity,
    pub action: PanelAction,
}

pub type PanelActionResult = Result<bool, String>;

pub type PanelActionSystem =
    bevy::ecs::system::SystemId<bevy::prelude::In<PanelActionInvocation>, PanelActionResult>;

/// Optional document commands are registered systems, separate from presentation.
#[derive(Clone, Copy, bevy::prelude::Component)]
pub struct PanelActions(pub PanelActionSystem);

#[derive(Clone, Debug)]
pub struct PanelExternalFilesInvocation {
    pub entity: bevy::prelude::Entity,
    pub event: ExternalFileDrag,
}

pub type PanelExternalFilesSystem = bevy::ecs::system::SystemId<
    bevy::prelude::In<PanelExternalFilesInvocation>,
    Result<ExternalFileDropResponse, String>,
>;

/// Native file drops resolve through the owning presentation's registered system.
#[derive(Clone, Copy, bevy::prelude::Component)]
pub struct PanelExternalFiles(pub PanelExternalFilesSystem);

/// Destination for newly created content. New windows default to the largest
/// area; callers can explicitly choose the focused area. Existing content is
/// focused in its current area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTarget {
    LargestArea,
    LastFocused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelPlacement {
    #[default]
    Center,
    Sidebar,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct PanelType {
    /// The factory supports opening without a document or local file.
    pub empty_input: bool,
    pub singleton: bool,
    /// Placement when constructing the initial workspace layout.
    pub placement: PanelPlacement,
    /// Runtime placement hint. The tiled workbench chooses destinations
    /// from the action's origin and any explicit area target.
    pub open_placement: PanelPlacement,
    pub id: &'static str,
    pub plugin: &'static str,
}

/// Actions sent to the originating panel by native menus and document lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Find,
    SelectAll,
    GoToLine,
    Undo,
    Redo,
    /// Validate and finish any local edit before saving or closing. Implementations
    /// may queue only edit requests; the host applies them before continuing.
    /// An error must retain the draft and prevents the save or close.
    CommitEdit,
}
