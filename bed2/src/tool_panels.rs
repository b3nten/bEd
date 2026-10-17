//! Workspace-owned tool panels; the host applies changes to shared window services.
use crate::workbench_demo::DemoWorkbench;
use bedtime_clipboard2::Clipboard;
use bedtime_ui2::{
    MotionPolicy, Theme, UiDiagnostics, UiFrame, UiIconStatus, UiIcons, UsesTheme,
    gallery::{self, GalleryAction, GalleryState},
};
use bedtime_workbench2::*;
use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct Gallery;
impl PanelDefinition for Gallery {
    const KEY: &'static str = "bed2.gallery";
    const LABEL: &'static str = "Widget gallery";
}

#[derive(Component)]
pub(crate) struct ThemeEditor;
impl PanelDefinition for ThemeEditor {
    const KEY: &'static str = "bed2.theme_editor";
    const LABEL: &'static str = "Theme editor";
}

#[derive(Resource)]
pub(crate) struct PanelTools {
    // The template shares host asset handles. Each panel receives its own widget state.
    pub gallery: GalleryState,
    // Host entity: only sent back to the host, never queried in the workspace world.
    pub theme: Entity,
    diagnostics: UiDiagnostics,
    icons: UiIconStatus,
    clipboard: Clipboard,
    pub requests: Vec<ToolRequest>,
}
pub(crate) enum ToolRequest {
    Gallery(GalleryAction),
    Theme { entity: Entity, value: Theme },
    Motion(MotionPolicy),
}
impl PanelTools {
    pub fn from_host(host: &World) -> Self {
        let demo = host.resource::<DemoWorkbench>();
        Self {
            gallery: demo.gallery.clone(),
            theme: host.get::<UsesTheme>(demo.surface).unwrap().0,
            diagnostics: host.get::<UiDiagnostics>(demo.surface).unwrap().clone(),
            icons: host.get::<UiIconStatus>(demo.surface).unwrap().clone(),
            clipboard: host.resource::<Clipboard>().clone(),
            requests: Vec::new(),
        }
    }
}

pub(crate) fn install(world: &mut World, tools: PanelTools) {
    world.insert_resource(tools);
    let mut w = Workbench::new(world);
    w.register_panel_type::<Gallery, _>(gallery_factory)
        .unwrap();
    w.register_panel_type::<ThemeEditor, _>(theme_factory)
        .unwrap();
}
fn gallery_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let state = world.resource::<PanelTools>().gallery.clone();
    let panel = register_panel_in_area(world, area, Gallery::LABEL, draw_gallery)?;
    world.entity_mut(panel).insert((Gallery, state));
    Ok(panel)
}
fn theme_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, ThemeEditor::LABEL, crate::theme_editor::draw)?;
    world.entity_mut(panel).insert(ThemeEditor);
    Ok(panel)
}

/// Toolbar buttons focus an existing tab in this layout, or open one in its focused area.
pub(crate) fn open<P: PanelDefinition + Component>(host: &mut World) -> Entity {
    let mut w = Workbench::new(host);
    let layout = w.active_layout();
    let kind = w
        .world_mut()
        .query::<(Entity, &PanelType)>()
        .iter(w.world())
        .find_map(|(entity, kind)| (kind.key == P::KEY).then_some(entity))
        .expect("Tool panel type is registered");
    let existing = w
        .panels(layout)
        .into_iter()
        .find(|&panel| w.world().get::<P>(panel).is_some());
    if let Some(panel) = existing {
        w.focus_panel(panel).unwrap();
        panel
    } else {
        let area = w.world().get::<Layout>(layout).unwrap().focused_area();
        w.open_panel(kind, area).unwrap()
    }
}

fn draw_gallery(
    frame: UiFrame,
    mut states: Query<&mut GalleryState>,
    mut tools: ResMut<PanelTools>,
) {
    let state = states.get_mut(frame.view).unwrap().into_inner();
    let mut motion = frame.motion;
    let original = motion;
    let tools = &mut *tools;
    let actions = gallery::draw_contents(
        frame,
        state,
        &mut motion,
        &tools.diagnostics,
        &tools.icons,
        &mut tools.clipboard,
    );
    tools
        .requests
        .extend(actions.into_iter().map(ToolRequest::Gallery));
    if motion.enabled != original.enabled || motion.seconds != original.seconds {
        tools.requests.push(ToolRequest::Motion(motion));
    }
}

/// Called by host chrome before the active workspace is drawn.
pub(crate) fn sync(host: &mut World) {
    let surface = host.resource::<DemoWorkbench>().surface;
    let theme = host.get::<UsesTheme>(surface).unwrap().0;
    let diagnostics = host.get::<UiDiagnostics>(surface).unwrap().clone();
    let icons = host.get::<UiIconStatus>(surface).unwrap().clone();
    let mut w = Workbench::new(host);
    let mut tools = w.world_mut().resource_mut::<PanelTools>();
    tools.theme = theme;
    tools.diagnostics = diagnostics;
    tools.icons = icons;
}

/// Apply tool requests after drawing and before queued workspace switches.
pub(crate) fn apply(host: &mut World) {
    let surface = host.resource::<DemoWorkbench>().surface;
    let requests = std::mem::take(
        &mut Workbench::new(host)
            .world_mut()
            .resource_mut::<PanelTools>()
            .requests,
    );
    for request in requests {
        match request {
            ToolRequest::Gallery(GalleryAction::Theme(theme)) => {
                host.entity_mut(surface).insert(UsesTheme(theme));
            }
            ToolRequest::Gallery(GalleryAction::Icons(Some(icons))) => {
                host.entity_mut(surface).insert(UiIcons(icons));
            }
            ToolRequest::Gallery(GalleryAction::Icons(None)) => {
                host.entity_mut(surface).remove::<UiIcons>();
            }
            ToolRequest::Theme { entity, value } => {
                host.get_mut::<Theme>(entity).unwrap().set_if_neq(value);
            }
            ToolRequest::Motion(policy) => {
                host.entity_mut(surface).insert(policy);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_ui2::{Icon, IconSet, UiSurface, themes};

    fn host() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Assets<Icon>>()
            .init_resource::<Assets<IconSet>>()
            .insert_resource(Clipboard::memory());
        let world = app.world_mut();
        let light = world.spawn(themes::light()).id();
        let dark = world.spawn(themes::dark()).id();
        let surface = world.spawn((UiSurface, UsesTheme(dark))).id();
        let mut gallery = gallery::create_state(world, light, dark);
        gallery.embedded = true;
        let host = world.spawn_empty().id();
        world.insert_resource(DemoWorkbench {
            host,
            surface,
            gallery,
        });
        let id = workspace(world);
        Workspaces::new(world).activate(id).unwrap();
        app
    }
    fn workspace(host: &mut World) -> WorkspaceId {
        let mut runtime = WorkspaceRuntime::new("Tools");
        install(runtime.world_mut(), PanelTools::from_host(host));
        Workspaces::new(host).open(runtime)
    }

    #[test]
    fn buttons_open_docked_tabs_reuse_them_and_keep_state_in_their_layout_and_workspace() {
        let mut app = host();
        let host = app.world_mut();
        let a = Workspaces::new(host).active();
        let gallery = open::<Gallery>(host);
        let layout = Workbench::new(host).active_layout();
        let area = Workbench::new(host)
            .world()
            .get::<InArea>(gallery)
            .unwrap()
            .0;
        assert_eq!(
            Workbench::new(host)
                .world()
                .get::<InLayout>(area)
                .unwrap()
                .0,
            layout
        );
        assert!(
            Workbench::new(host)
                .world()
                .get::<GalleryState>(gallery)
                .unwrap()
                .embedded
        );
        Workbench::new(host)
            .world_mut()
            .get_mut::<GalleryState>(gallery)
            .unwrap()
            .text = "Keep this".into();
        let editor = open::<ThemeEditor>(host);
        assert_eq!(
            Workbench::new(host)
                .world()
                .get::<InArea>(editor)
                .unwrap()
                .0,
            area
        );
        assert_eq!(open::<Gallery>(host), gallery);
        assert_eq!(Workbench::new(host).focused_panel(layout), Some(gallery));
        assert_eq!(Workbench::new(host).panels(layout).len(), 2);
        assert_eq!(open::<ThemeEditor>(host), editor);

        let reading = Workbench::new(host).create_layout("Reading").unwrap();
        Workbench::new(host).activate_layout(reading).unwrap();
        assert_ne!(open::<Gallery>(host), gallery);
        Workbench::new(host).activate_layout(layout).unwrap();
        assert_eq!(open::<Gallery>(host), gallery);

        let b = workspace(host);
        Workspaces::new(host).activate(b).unwrap();
        let other = open::<Gallery>(host);
        assert_ne!(
            Workbench::new(host)
                .world()
                .get::<GalleryState>(other)
                .unwrap()
                .text,
            "Keep this"
        );
        Workspaces::new(host).activate(a).unwrap();
        assert_eq!(
            Workbench::new(host)
                .world()
                .get::<GalleryState>(gallery)
                .unwrap()
                .text,
            "Keep this"
        );
        Workbench::new(host).close_panel(gallery).unwrap();
        let reopened = open::<Gallery>(host);
        assert_ne!(reopened, gallery);
        assert_eq!(Workbench::new(host).focused_panel(layout), Some(reopened));
    }

    #[test]
    fn workspace_tools_apply_changes_to_shared_host_services() {
        let mut app = host();
        let host = app.world_mut();
        let surface = host.resource::<DemoWorkbench>().surface;
        let light = host.resource::<DemoWorkbench>().gallery.light;
        let mut edited = themes::light();
        edited.metrics.font_size = 20.0;
        let icons = host
            .resource::<DemoWorkbench>()
            .gallery
            .custom_icons
            .clone();
        Workbench::new(host)
            .world_mut()
            .resource_mut::<PanelTools>()
            .requests
            .extend([
                ToolRequest::Theme {
                    entity: light,
                    value: edited.clone(),
                },
                ToolRequest::Gallery(GalleryAction::Theme(light)),
                ToolRequest::Gallery(GalleryAction::Icons(Some(icons.clone()))),
                ToolRequest::Motion(MotionPolicy {
                    enabled: false,
                    seconds: 0.25,
                }),
            ]);
        apply(host);
        assert_eq!(host.get::<Theme>(light).unwrap(), &edited);
        assert_eq!(host.get::<UsesTheme>(surface).unwrap().0, light);
        assert_eq!(host.get::<UiIcons>(surface).unwrap().0, icons);
        assert!(!host.get::<MotionPolicy>(surface).unwrap().enabled);
        sync(host);
        let mut w = Workbench::new(host);
        let mut tools = w.world_mut().resource_mut::<PanelTools>();
        assert_eq!(tools.theme, light);
        assert!(tools.requests.is_empty());
        tools.clipboard.write_text("Shared clipboard").unwrap();
        assert_eq!(
            host.resource_mut::<Clipboard>().read_text().unwrap(),
            "Shared clipboard"
        );
    }
}
