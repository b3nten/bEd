//! Five application slots and reusable tool arrangements. Entities stay live.
use crate::session::{AreaRecipe, LayoutRecipe, PanelKind, Session, TabRecipe};
use bedtime_workbench2::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LayoutSlot {
    One,
    Two,
    Three,
    Four,
    Five,
}

impl LayoutSlot {
    pub const ALL: [Self; 5] = [Self::One, Self::Two, Self::Three, Self::Four, Self::Five];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap()
    }
    pub fn label(self) -> &'static str {
        ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][self.index()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    LanguageServers,
    Files,
    Structure,
    Terminal,
    Viewport,
    Tasks,
    Settings,
}

impl ToolKind {
    pub const ALL: [Self; 7] = [
        Self::LanguageServers,
        Self::Files,
        Self::Structure,
        Self::Terminal,
        Self::Viewport,
        Self::Tasks,
        Self::Settings,
    ];
    
    pub fn key(self) -> &'static str {
        match self {
            Self::LanguageServers => crate::lsp_panel::LspPanel::KEY,
            Self::Files => bedtime_filetree2::FileTreePanel::KEY,
            Self::Structure => bed_structure2::StructurePanel::KEY,
            Self::Terminal => bedtime_terminal2::TerminalPanel::KEY,
            Self::Viewport => crate::cube::CubeViewport::KEY,
            Self::Tasks => crate::tasks::TasksPanel::KEY,
            Self::Settings => crate::settings::SettingsPanel::KEY,
        }
    }
    
    fn panel_kind(self) -> PanelKind {
        match self {
            Self::LanguageServers => PanelKind::LanguageServers,
            Self::Files => PanelKind::Files,
            Self::Structure => PanelKind::Structure,
            Self::Terminal => PanelKind::Terminal,
            Self::Viewport => PanelKind::Viewport,
            Self::Tasks => PanelKind::Tasks,
            Self::Settings => PanelKind::Settings,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutTemplate {
    pub areas: Vec<TemplateArea>,
    pub focused_area: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateArea {
    pub rect: AreaRect,
    pub tools: Vec<ToolKind>,
    pub selected: Option<usize>,
}

impl LayoutTemplate {
    pub fn validate(&self) -> Result<(), String> {
        if self.areas.is_empty() {
            return Err("A layout default must contain an area".into());
        }
        crate::session::validate(&Session {
            version: 1,
            root: None,
            settings: Default::default(),
            visibility: Default::default(),
            name: String::new(),
            background_updates: true,
            active_layout: Some(0),
            layouts: vec![LayoutRecipe {
                slot: None,
                name: String::new(),
                focused_area: self.focused_area,
                areas: self
                    .areas
                    .iter()
                    .map(|area| AreaRecipe {
                        rect: area.rect,
                        selected_tab: area.selected,
                        tabs: area
                            .tools
                            .iter()
                            .map(|kind| TabRecipe {
                                kind: kind.panel_kind(),
                                title: String::new(),
                                preview: false,
                                file: None,
                                tree_root: None,
                            })
                            .collect(),
                    })
                    .collect(),
            }],
        })
    }
    
    fn empty() -> Self {
        Self {
            areas: vec![TemplateArea {
                rect: AreaRect {
                    min: [0, 0],
                    max: [LAYOUT_EXTENT; 2],
                },
                tools: vec![],
                selected: None,
            }],
            focused_area: Some(0),
        }
    }
    
    fn terminal() -> Self {
        let mut template = Self::empty();
        template.areas[0].tools.push(ToolKind::Terminal);
        template.areas[0].selected = Some(0);
        template
    }
}

#[derive(Resource, Clone)]
pub struct LayoutDefaults(pub [LayoutTemplate; 5]);
impl Default for LayoutDefaults {
    fn default() -> Self {
        Self([
            LayoutTemplate::terminal(),
            LayoutTemplate::empty(),
            LayoutTemplate::empty(),
            LayoutTemplate::empty(),
            LayoutTemplate::empty(),
        ])
    }
}

pub struct LayoutsPlugin;
impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_layouts", |world, _| {
                let entries: Vec<_> = world.query::<(Entity, &LayoutSlot, &InWorkspace)>().iter(world).map(|(entity, slot, workspace)|
                    serde_json::json!({ "layout": entity.to_bits().to_string(), "workspace": workspace.0.to_bits().to_string(),
                        "slot": slot.index()+1, "active": world.get::<ActiveLayout>(entity).is_some() })).collect();
                Ok(serde_json::json!({ "layouts": entries, "defaults": world.resource::<LayoutDefaults>().0 }))
            }).register_automation_command("app.select_layout", |world, arguments| {
                let number = arguments.get("slot").and_then(|v| v.as_u64()).filter(|v| (1..=5).contains(v)).ok_or("slot must be 1–5")?;
                let workspace = world.query_filtered::<Entity, With<ActiveWorkspace>>().iter(world).next().ok_or("Open a workspace first")?;
                switch(world, workspace, LayoutSlot::ALL[number as usize-1])?;
                Ok(serde_json::json!({}))
            });
        }
        app.init_resource::<LayoutDefaults>()
            .add_systems(Update, initialize.after(crate::files::FilesSystems::Update));
    }
}

fn initialize(world: &mut World) {
    // Only bEd-owned rooted workspaces receive application defaults. Bare
    // Workbench composition stays explicit, including its existing area IDs.
    let owners: Vec<_> = world
        .query_filtered::<Entity, (With<Workspace>, With<crate::files::WorkspaceRoot>)>()
        .iter(world)
        .collect();
    for owner in owners {
        if !crate::storage::ready(world, owner) {
            continue;
        }
        let has_slot = world.get::<WorkspaceMembers>(owner).is_some_and(|members| {
            members
                .entities()
                .iter()
                .any(|&e| world.get::<LayoutSlot>(e).is_some())
        });
        if has_slot {
            continue;
        }
        // Fresh application startup has no layout. Headless/library hosts may
        // already have a composition; adopt it rather than replace live panels.
        if let Some(layout) = active_layout(world, owner) {
            world.entity_mut(layout).insert(LayoutSlot::One);
            if Workbench::for_entity(world, layout)
                .unwrap()
                .panels(layout)
                .is_empty()
            {
                let template = world.resource::<LayoutDefaults>().0[0].clone();
                apply(world, layout, &template).expect("Validated initial layout");
                crate::session::seed_initial_layout(world, owner);
            }
        } else if let Err(error) = initialize_first(world, owner) {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Layout".into(),
                    message: "The initial layout couldn't be created.".into(),
                    details: Some(error),
                },
            );
        }
    }
}

fn initialize_first(world: &mut World, workspace: Entity) -> Result<(), String> {
    let template = world.resource::<LayoutDefaults>().0[0].clone();
    template.validate()?;
    let layout = world
        .spawn((
            Layout {
                name: LayoutSlot::One.label().into(),
                focused_area: None,
            },
            InWorkspace(workspace),
            LayoutSlot::One,
            ActiveLayout,
        ))
        .id();
    apply(world, layout, &template)?;
    crate::session::seed_initial_layout(world, workspace);
    Ok(())
}

pub fn switch(world: &mut World, workspace: Entity, slot: LayoutSlot) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world, workspace) {
        return Err("Workspace settings are loading or a close decision is pending".into());
    }
    let existing = world
        .get::<WorkspaceMembers>(workspace)
        .and_then(|members| {
            members
                .entities()
                .iter()
                .copied()
                .find(|&e| world.get::<LayoutSlot>(e) == Some(&slot))
        });
    let layout = if let Some(layout) = existing {
        layout
    } else {
        let template = world.resource::<LayoutDefaults>().0[slot.index()].clone();
        template.validate()?;
        let layout = world
            .spawn((
                Layout {
                    name: slot.label().into(),
                    focused_area: None,
                },
                InWorkspace(workspace),
                slot,
            ))
            .id();
        apply(world, layout, &template)?;
        layout
    };
    Workbench::for_entity(world, workspace)?.activate_layout(layout)
}

pub fn save_current(world: &mut World, workspace: Entity) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world, workspace) {
        return Err("Finish loading or cancel quit before saving a layout default".into());
    }
    let layout = active_layout(world, workspace).ok_or("Select a layout first")?;
    let slot = *world
        .get::<LayoutSlot>(layout)
        .ok_or("Current layout has no numbered slot")?;
    let template = capture(world, layout)?;
    world.resource_mut::<LayoutDefaults>().0[slot.index()] = template;
    Ok(())
}

pub fn reset_current(world: &mut World, workspace: Entity) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world, workspace) {
        return Err("Finish loading or cancel quit before resetting a layout".into());
    }
    let layout = active_layout(world, workspace).ok_or("Select a layout first")?;
    let slot = *world
        .get::<LayoutSlot>(layout)
        .ok_or("Current layout has no numbered slot")?;
    let template = world.resource::<LayoutDefaults>().0[slot.index()].clone();
    apply(world, layout, &template)
}

fn capture(world: &World, layout: Entity) -> Result<LayoutTemplate, String> {
    let areas = world
        .get::<LayoutAreas>(layout)
        .ok_or("Layout unavailable")?
        .entities();
    let mut recipes = Vec::new();
    for &area in areas {
        let data = world.get::<Area>(area).unwrap();
        let mut tools = Vec::new();
        let mut selected = None;
        for &panel in panels_in_area(world, area) {
            let key = &world.get::<PanelInstance>(panel).unwrap().key;
            if let Some(kind) = ToolKind::ALL.into_iter().find(|kind| kind.key() == key) {
                if data.selected == Some(panel) {
                    selected = Some(tools.len());
                }
                tools.push(kind);
            }
        }
        recipes.push(TemplateArea {
            rect: data.rect,
            tools,
            selected,
        });
    }
    let template = LayoutTemplate {
        areas: recipes,
        focused_area: world
            .get::<Layout>(layout)
            .unwrap()
            .focused_area
            .and_then(|a| areas.iter().position(|&e| e == a)),
    };
    template.validate()?;
    Ok(template)
}

/// Validate before changing entities; move every retained panel before retiring
/// empty old areas. Document bindings and worker owners never change on reset.
fn apply(world: &mut World, layout: Entity, template: &LayoutTemplate) -> Result<(), String> {
    template.validate()?;
    let old_areas = world
        .get::<LayoutAreas>(layout)
        .map(|a| a.entities().to_vec())
        .unwrap_or_default();
    let mut remaining: Vec<_> = old_areas
        .iter()
        .flat_map(|&area| panels_in_area(world, area).iter().copied())
        .collect();
    let mut areas = Vec::new();
    let mut selections = Vec::new();
    for recipe in &template.areas {
        let area = world
            .spawn((
                Area {
                    rect: recipe.rect,
                    selected: None,
                },
                InLayout(layout),
            ))
            .id();
        areas.push(area);
        let mut panels = Vec::new();
        for kind in &recipe.tools {
            let panel = if let Some(index) = remaining.iter().position(|&panel| {
                world
                    .get::<PanelInstance>(panel)
                    .is_some_and(|data| data.key == kind.key())
            }) {
                let panel = remaining.remove(index);
                world.entity_mut(panel).insert(InArea(area));
                panel
            } else {
                spawn_tool(world, area, *kind)
            };
            panels.push(panel);
        }
        selections.push(recipe.selected.map(|index| panels[index]));
    }
    let largest = template
        .areas
        .iter()
        .enumerate()
        .max_by_key(|(index, area)| {
            (
                i64::from(area.rect.max[0] - area.rect.min[0])
                    * i64::from(area.rect.max[1] - area.rect.min[1]),
                std::cmp::Reverse(*index),
            )
        })
        .unwrap()
        .0;
    let first_extra = remaining.first().copied();
    for panel in remaining {
        world.entity_mut(panel).insert(InArea(areas[largest]));
    }
    for area in old_areas {
        world.despawn(area);
    }
    world.flush();
    for (index, &area) in areas.iter().enumerate() {
        world.get_mut::<Area>(area).unwrap().selected =
            selections[index].or_else(|| if index == largest { first_extra } else { None });
    }
    world.get_mut::<Layout>(layout).unwrap().focused_area =
        template.focused_area.map(|index| areas[index]);
    reconcile_workbench(world)
}

pub fn spawn_tool(world: &mut World, area: Entity, kind: ToolKind) -> Entity {
    match kind {
        ToolKind::LanguageServers => world.spawn((crate::lsp_panel::LspPanel, InArea(area))).id(),
        ToolKind::Files => {
            let workspace = world.get::<InWorkspace>(area).unwrap().0;
            let root = world
                .get::<crate::files::WorkspaceRoot>(workspace)
                .map(|root| root.0.clone());
            let panel = world
                .spawn((bedtime_filetree2::FileTreePanel::default(), InArea(area)))
                .id();
            if let Some(root) = root {
                world
                    .entity_mut(panel)
                    .insert(bedtime_filetree2::FileTreeRoot(root));
            }
            panel
        }
        ToolKind::Structure => world
            .spawn((bed_structure2::StructurePanel::default(), InArea(area)))
            .id(),
        ToolKind::Terminal => {
            let workspace = world.get::<InWorkspace>(area).unwrap().0;
            let options = bedtime_terminal2::TerminalOptions {
                working_directory: crate::files::workspace_directory(world, workspace),
                ..Default::default()
            };
            world
                .spawn((bedtime_terminal2::TerminalPanel::new(options), InArea(area)))
                .id()
        }
        ToolKind::Viewport => world.spawn((crate::cube::CubeViewport, InArea(area))).id(),
        ToolKind::Tasks => world
            .spawn((crate::tasks::TasksPanel::default(), InArea(area)))
            .id(),
        ToolKind::Settings => world
            .spawn((crate::settings::SettingsPanel, InArea(area)))
            .id(),
    }
}

pub fn open_or_focus(world: &mut World, layout: Entity, kind: ToolKind) -> Result<Entity, String> {
    let panels = Workbench::for_entity(world, layout)?.panels(layout);
    let panel = if let Some(panel) = panels.into_iter().find(|&panel| {
        world
            .get::<PanelInstance>(panel)
            .is_some_and(|data| data.key == kind.key())
    }) {
        panel
    } else {
        crate::menus::open_panel(world, layout, kind.key())?
    };
    Workbench::for_entity(world, layout)?.focus_panel(panel)?;
    Ok(panel)
}

pub struct UseLayout<const N: usize>;
impl<const N: usize> ContextCommand for UseLayout<N> {
    const KEY: &'static str = [
        "bed.layout.1",
        "bed.layout.2",
        "bed.layout.3",
        "bed.layout.4",
        "bed.layout.5",
    ][N];
    const LABEL: &'static str = ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][N];
}

pub struct SaveCurrentLayout;
impl ContextCommand for SaveCurrentLayout {
    const KEY: &'static str = "bed.layout.save_default";
    const LABEL: &'static str = "Save Current Layout";
}

pub struct ResetCurrentLayout;
impl ContextCommand for ResetCurrentLayout {
    const KEY: &'static str = "bed.layout.reset";
    const LABEL: &'static str = "Reset Current Layout";
}

pub fn install_menu(world: &mut World, view: Entity) {
    register_context_command::<SaveCurrentLayout>(world).unwrap();
    register_context_command::<ResetCurrentLayout>(world).unwrap();
    install_global_handler::<SaveCurrentLayout, _>(
        world,
        |invocation: In<ContextCommandInvocation>, world: &mut World| {
            save_current(
                world,
                invocation
                    .workspace
                    .ok_or("Open a workspace first")?
                    .entity(),
            )
        },
    )
    .unwrap();
    install_global_handler::<ResetCurrentLayout, _>(
        world,
        |invocation: In<ContextCommandInvocation>, world: &mut World| {
            reset_current(
                world,
                invocation
                    .workspace
                    .ok_or("Open a workspace first")?
                    .entity(),
            )
        },
    )
    .unwrap();
    let menu = world
        .spawn((
            Menu {
                label: "Layout".into(),
            },
            InMenu(view),
        ))
        .id();
    install_slot::<0>(world, menu, KeyCode::Digit1);
    install_slot::<1>(world, menu, KeyCode::Digit2);
    install_slot::<2>(world, menu, KeyCode::Digit3);
    install_slot::<3>(world, menu, KeyCode::Digit4);
    install_slot::<4>(world, menu, KeyCode::Digit5);
    world.spawn((MenuSeparator, InMenu(menu)));
    world.spawn((
        MenuItem::new(SaveCurrentLayout::LABEL),
        InMenu(menu),
        Invokes::of::<SaveCurrentLayout>(),
    ));
    world.spawn((
        MenuItem::new(ResetCurrentLayout::LABEL),
        InMenu(menu),
        Invokes::of::<ResetCurrentLayout>(),
    ));
}

fn install_slot<const N: usize>(world: &mut World, menu: Entity, key: KeyCode) {
    register_context_command::<UseLayout<N>>(world).unwrap();
    install_global_handler::<UseLayout<N>, _>(
        world,
        |invocation: In<ContextCommandInvocation>, world: &mut World| {
            switch(
                world,
                invocation
                    .workspace
                    .ok_or("Open a workspace first")?
                    .entity(),
                LayoutSlot::ALL[N],
            )
        },
    )
    .unwrap();
    world.spawn((
        MenuItem::new(LayoutSlot::ALL[N].label()),
        InMenu(menu),
        Invokes::of::<UseLayout<N>>(),
    ));
    world.spawn(Binding::new::<UseLayout<N>>(KeyChord::new(
        Modifiers::PRIMARY,
        key,
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<LayoutDefaults>();
        let workspace = Workspaces::new(app.world_mut()).open("Project").entity();
        let layout = active_layout(app.world(), workspace).unwrap();
        app.world_mut().entity_mut(layout).insert(LayoutSlot::One);
        (app, workspace, layout)
    }
    #[test]
    fn bare_workbench_workspaces_keep_their_explicit_composition() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, LayoutsPlugin));
        let workspace = Workspaces::new(app.world_mut())
            .open("Explicit composition")
            .entity();
        let layout = active_layout(app.world(), workspace).unwrap();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        app.update();
        assert!(app.world().get::<Area>(area).is_some());
        assert!(app.world().get::<LayoutSlot>(layout).is_none());
        assert!(
            Workbench::for_entity(app.world_mut(), layout)
                .unwrap()
                .panels(layout)
                .is_empty()
        );
        assert!(
            app.world()
                .get::<WorkspaceWork>(workspace)
                .unwrap()
                .list()
                .is_empty()
        );
    }
    #[test]
    fn switching_slots_retains_canonical_documents_and_panel_instances() {
        let (mut app, workspace, first) = host();
        let area = Workbench::for_entity(app.world_mut(), first)
            .unwrap()
            .destination_area(first)
            .unwrap();
        let panel = app
            .world_mut()
            .spawn((crate::files::FilePreviewPanel::default(), InArea(area)))
            .id();
        let document = open_document(
            app.world_mut(),
            workspace,
            "retained".to_owned(),
            DocumentOptions::loaded("Document"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), panel, document).unwrap();
        let terminal = spawn_tool(app.world_mut(), area, ToolKind::Terminal);
        switch(app.world_mut(), workspace, LayoutSlot::Two).unwrap();
        assert_ne!(first, active_layout(app.world(), workspace).unwrap());
        assert!(app.world().get::<DocumentView>(panel).is_some());
        assert!(
            app.world()
                .get::<bedtime_terminal2::TerminalPanel>(terminal)
                .is_some()
        );
        switch(app.world_mut(), workspace, LayoutSlot::One).unwrap();
        assert_eq!(active_layout(app.world(), workspace), Some(first));
        assert_eq!(read_document(app.world(), document).unwrap(), "retained");
        assert_eq!(
            app.world().get::<DocumentView>(panel).unwrap().document(),
            document.id()
        );
        assert_eq!(app.world().get::<InArea>(terminal).unwrap().0, area);
    }
    #[test]
    fn reset_reuses_tools_and_gathers_extra_tabs_without_losing_documents() {
        let (mut app, workspace, layout) = host();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let terminal = spawn_tool(app.world_mut(), area, ToolKind::Terminal);
        let extra_terminal = spawn_tool(app.world_mut(), area, ToolKind::Terminal);
        let editor = app
            .world_mut()
            .spawn((crate::files::FilePreviewPanel::default(), InArea(area)))
            .id();
        let document = open_document(
            app.world_mut(),
            workspace,
            "dirty".to_owned(),
            DocumentOptions::unsaved("Draft"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), editor, document).unwrap();
        reset_current(app.world_mut(), workspace).unwrap();
        let areas = app
            .world()
            .get::<LayoutAreas>(layout)
            .unwrap()
            .entities()
            .to_vec();
        assert_eq!(areas.len(), 1);
        assert_eq!(app.world().get::<InArea>(terminal).unwrap().0, areas[0]);
        assert_eq!(
            app.world().get::<InArea>(extra_terminal).unwrap().0,
            areas[0]
        );
        assert_eq!(app.world().get::<InArea>(editor).unwrap().0, areas[0]);
        assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
        let before = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .panels(layout);
        reset_current(app.world_mut(), workspace).unwrap();
        let after = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .panels(layout);
        assert_eq!(
            before
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            after.into_iter().collect()
        );
        assert_eq!(read_document(app.world(), document).unwrap(), "dirty");
    }
    #[test]
    fn saved_defaults_exclude_paths_and_apply_only_on_creation_or_explicit_reset() {
        let (mut app, workspace, layout) = host();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let files = spawn_tool(app.world_mut(), area, ToolKind::Files);
        app.world_mut()
            .entity_mut(files)
            .insert(bedtime_filetree2::FileTreeRoot(std::path::PathBuf::from(
                "/private-project",
            )));
        let editor = app
            .world_mut()
            .spawn((crate::files::FilePreviewPanel::default(), InArea(area)))
            .id();
        save_current(app.world_mut(), workspace).unwrap();
        let encoded =
            crate::storage::encode(&app.world().resource::<LayoutDefaults>().0[0]).unwrap();
        assert!(
            !String::from_utf8(encoded)
                .unwrap()
                .contains("private-project")
        );
        let defaults = app.world().resource::<LayoutDefaults>().0[0].clone();
        assert_eq!(defaults.areas[0].tools, vec![ToolKind::Files]);
        let other = Workspaces::new(app.world_mut()).open("Other").entity();
        let other_layout = active_layout(app.world(), other).unwrap();
        app.world_mut()
            .entity_mut(other_layout)
            .insert(LayoutSlot::One);
        let other_area = Workbench::for_entity(app.world_mut(), other)
            .unwrap()
            .destination_area(other_layout)
            .unwrap();
        let settings = spawn_tool(app.world_mut(), other_area, ToolKind::Settings);
        assert_eq!(app.world().get::<InArea>(settings).unwrap().0, other_area);
        reset_current(app.world_mut(), other).unwrap();
        assert!(app.world().get::<InArea>(settings).is_some());
        assert!(app.world().get::<InArea>(editor).is_some());
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<bedtime_filetree2::FileTreePanel>>()
                .iter(app.world())
                .count(),
            2
        );
    }
    #[test]
    fn invalid_reset_is_rejected_before_geometry_or_panels_change() {
        let (mut app, workspace, layout) = host();
        let before = capture(app.world(), layout).unwrap();
        app.world_mut().resource_mut::<LayoutDefaults>().0[0].areas[0]
            .rect
            .max[0] = -1;
        assert!(reset_current(app.world_mut(), workspace).is_err());
        assert_eq!(capture(app.world(), layout).unwrap(), before);
    }
    #[test]
    fn titlebar_click_reuses_current_layout_tools_and_drag_factory_creates_another() {
        let (mut app, _, layout) = host();
        let first = open_or_focus(app.world_mut(), layout, ToolKind::Settings).unwrap();
        assert_eq!(
            open_or_focus(app.world_mut(), layout, ToolKind::Settings).unwrap(),
            first
        );
        let area = app.world().get::<InArea>(first).unwrap().0;
        let factory = PanelFactory::of::<crate::settings::SettingsPanel>();
        let second = (factory.create)(app.world_mut(), area);
        assert_ne!(first, second);
        assert_eq!(
            Workbench::for_entity(app.world_mut(), layout)
                .unwrap()
                .panels(layout)
                .len(),
            2
        );
    }
}
