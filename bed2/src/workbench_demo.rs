//! Application policy and chrome. All layouts and panels use public workbench APIs.
use bedtime_ui2::{
    UiFrame, gallery,
    imgui::{Condition, WindowFlags},
    register_view,
};
use bedtime_workbench2::*;
use bevy::prelude::*;

#[derive(Resource)]
pub(crate) struct DemoWorkbench {
    pub drag_source: Rect,
    pub light: Entity,
    pub dark: Entity,
    pub home: Entity,
    pub split: Entity,
    pub reading: Entity,
    pub scratch: Entity,
}
#[derive(Component)]
pub(crate) struct Notes {
    pub text: String,
}

#[derive(Resource, Default)]
struct DemoRequests(Vec<DemoRequest>);
enum DemoRequest {
    Gallery,
    Cube,
    NewLayout,
    DeleteLayout,
    NewWorkspace,
    DeleteWorkspace,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<DemoRequests>().add_systems(
        PostUpdate,
        apply
            .after(CommandSystems::Dispatch)
            .before(WorkbenchSystems::Apply),
    );
}

pub(crate) fn setup(world: &mut World, surface: Entity, light: Entity, dark: Entity) -> Entity {
    let (home, split, reading, scratch, scratch_layout) = {
        let mut w = Workbench::new(world);
        let home = w.active_workspace();
        let split = w.active_layout();
        let reading = w.create_layout(home, "Reading").unwrap();
        let scratch = w.create_workspace("Scratch", false);
        w.activate_workspace(scratch).unwrap();
        let scratch_layout = w.active_layout();
        w.activate_workspace(home).unwrap();
        (home, split, reading, scratch, scratch_layout)
    };
    world.get_mut::<Workspace>(home).unwrap().name = "Playground".into();
    world.get_mut::<Layout>(split).unwrap().name = "Split".into();
    world.insert_resource(DemoWorkbench {
        drag_source: Rect::default(),
        light,
        dark,
        home,
        split,
        reading,
        scratch,
    });
    // Chrome draws before the host, and remains available in full-window mode.
    register_view(world, surface, chrome);
    let host = attach_surface(world, surface);
    world.get_mut::<WorkbenchHost>(host).unwrap().top_inset = 100.0;
    let gallery = add_gallery(world, split);
    let cube = crate::viewport_demo::spawn(world, split);
    Workbench::new(world)
        .move_panel(cube, gallery, Some(SplitAxis::Horizontal))
        .unwrap();
    Workbench::new(world)
        .resize_border(
            split,
            SplitAxis::Horizontal,
            5000,
            [0, LAYOUT_EXTENT],
            5900,
            1,
        )
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<GalleryPanel, _>(gallery_factory)
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<NotesPanel, _>(notes_factory)
        .unwrap();
    add_gallery(world, reading);
    let notes = register_panel(world, scratch_layout, "Scratch notes", draw_notes).unwrap();
    world.entity_mut(notes).insert(Notes { text: "This is panel-local state. Switch workspaces and come back.\n\nNo files or documents are involved yet.".into() });
    gallery
}

fn add_gallery(world: &mut World, layout: Entity) -> Entity {
    let context = world.resource::<DemoWorkbench>();
    let (light, dark) = (context.light, context.dark);
    let mut state = gallery::create_state(world, light, dark);
    state.embedded = true;
    let panel = register_panel(world, layout, "Widget gallery", gallery::draw_gallery).unwrap();
    world.entity_mut(panel).insert((
        state,
        bedtime_ui2::IconSource::from(bedtime_ui2::BuiltinIcon::Folder),
    ));
    panel
}

fn draw_notes(frame: UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    frame.controls().heading("A workspace with its own panels");
    frame
        .controls()
        .muted("Layout switches retain instances; closing a panel retires it.");
    frame
        .ui
        .input_text_multiline("##notes", &mut notes.text, frame.ui.content_region_avail())
        .build();
}

fn chrome(frame: UiFrame, world: &mut World) {
    let ui = frame.ui;
    let c = frame.controls();
    let active_workspace = Workbench::new(world).active_workspace();
    let active_layout = Workbench::new(world).active_layout();
    let workspaces: Vec<_> = world
        .query::<(Entity, &Workspace)>()
        .iter(world)
        .map(|(e, w)| (e, w.name.clone()))
        .collect();
    let layouts: Vec<_> = world
        .query::<(Entity, &Layout, &InWorkspace)>()
        .iter(world)
        .filter(|(_, _, w)| w.0 == active_workspace)
        .map(|(e, l, _)| (e, l.name.clone()))
        .collect();
    let overlay = world
        .query::<(&crate::viewport_demo::DemoState, &InArea)>()
        .iter(world)
        .any(|(s, a)| s.overlay && world.get::<InLayout>(a.0).unwrap().0 == active_layout);
    if overlay {
        crate::viewport_demo::draw_overlay(&frame);
    } else {
        ui.get_background_draw_list()
            .add_rect(
                [0.0, 0.0],
                [ui.io().display_size()[0], 100.0],
                ui.get_color_u32_from_rgba(frame.theme.palette.background),
            )
            .filled(true)
            .build();
    }
    ui.window("##bed2-workbench-toolbar")
        .position([0.0, 0.0], Condition::Always)
        .size([ui.io().display_size()[0], 100.0], Condition::Always)
        .flags(
            WindowFlags::NO_DECORATION
                | WindowFlags::NO_MOVE
                | WindowFlags::NO_BACKGROUND
                | WindowFlags::NO_SAVED_SETTINGS
                | WindowFlags::NO_SCROLLBAR,
        )
        .build(|| {
            if overlay {
                c.text("Bevy scene + Bedtime overlay");
                if c.primary_button("Return to workbench") {
                    for mut state in world
                        .query::<&mut crate::viewport_demo::DemoState>()
                        .iter_mut(world)
                    {
                        state.overlay = false;
                    }
                }
                return;
            }
            c.text("bEd 2 / Workbench");
            c.same_line();
            let workspace_name = &world.get::<Workspace>(active_workspace).unwrap().name;
            ui.set_next_item_width(160.0);
            if let Some(_combo) = ui.begin_combo("Workspace", workspace_name) {
                for (entity, name) in &workspaces {
                    if ui
                        .selectable_config(name)
                        .selected(*entity == active_workspace)
                        .build()
                    {
                        world
                            .resource_mut::<WorkbenchRequests>()
                            .submit(WorkbenchRequest::ActivateWorkspace(*entity));
                    }
                }
            }
            c.same_line();
            let layout_name = &world.get::<Layout>(active_layout).unwrap().name;
            ui.set_next_item_width(130.0);
            if let Some(_combo) = ui.begin_combo("Layout", layout_name) {
                for (entity, name) in &layouts {
                    if ui
                        .selectable_config(name)
                        .selected(*entity == active_layout)
                        .build()
                    {
                        world
                            .resource_mut::<WorkbenchRequests>()
                            .submit(WorkbenchRequest::ActivateLayout(*entity));
                    }
                }
            }
            c.same_line();
            if c.button("New layout") {
                world
                    .resource_mut::<DemoRequests>()
                    .0
                    .push(DemoRequest::NewLayout);
            }
            c.same_line();
            c.disabled(layouts.len() == 1, || {
                if c.button("Delete layout") {
                    world
                        .resource_mut::<DemoRequests>()
                        .0
                        .push(DemoRequest::DeleteLayout);
                }
            });
            if c.button("Open gallery") {
                world
                    .resource_mut::<DemoRequests>()
                    .0
                    .push(DemoRequest::Gallery);
            }
            c.same_line();
            if c.button("Open 3D viewport") {
                world
                    .resource_mut::<DemoRequests>()
                    .0
                    .push(DemoRequest::Cube);
            }
            c.same_line();
            if c.button("New workspace") {
                world
                    .resource_mut::<DemoRequests>()
                    .0
                    .push(DemoRequest::NewWorkspace);
            }
            c.same_line();
            c.disabled(workspaces.len() == 1, || {
                if c.button("Delete workspace") {
                    world
                        .resource_mut::<DemoRequests>()
                        .0
                        .push(DemoRequest::DeleteWorkspace);
                }
            });
            c.same_line();
            c.button("Drag notes");
            world.resource_mut::<DemoWorkbench>().drag_source = Rect::from_corners(
                Vec2::from_array(ui.item_rect_min()),
                Vec2::from_array(ui.item_rect_max()),
            );
            if let Some(kind) = world
                .query::<(Entity, &PanelType)>()
                .iter(world)
                .find_map(|(e, p)| (p.key == NotesPanel::KEY).then_some(e))
            {
                panel_drag_source(ui, kind, "Notes");
            }
            c.same_line();
            c.muted("Drag corners to split/join. Drag tabs to move or split.");
        });
}

fn apply(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<DemoRequests>().0);
    for request in requests {
        let workspace = Workbench::new(world).active_workspace();
        let layout = Workbench::new(world).active_layout();
        match request {
            DemoRequest::Gallery => {
                let panel = world
                    .query_filtered::<(Entity, &InArea), With<gallery::GalleryState>>()
                    .iter(world)
                    .find_map(|(e, l)| {
                        (world.get::<InLayout>(l.0).unwrap().0 == layout).then_some(e)
                    });
                if let Some(panel) = panel {
                    Workbench::new(world).focus_panel(panel).unwrap();
                } else {
                    add_gallery(world, layout);
                }
            }
            DemoRequest::Cube => crate::viewport_demo::show(world),
            DemoRequest::NewLayout => {
                let n = world.query::<&Layout>().iter(world).count();
                let layout = Workbench::new(world)
                    .create_layout(workspace, format!("Layout {}", n + 1))
                    .unwrap();
                Workbench::new(world).activate_layout(layout).unwrap();
            }
            DemoRequest::DeleteLayout => {
                Workbench::new(world).delete_layout(layout).unwrap();
            }
            DemoRequest::NewWorkspace => {
                let n = world.query::<&Workspace>().iter(world).count();
                let workspace =
                    Workbench::new(world).create_workspace(format!("Workspace {}", n + 1), false);
                Workbench::new(world).activate_workspace(workspace).unwrap();
            }
            DemoRequest::DeleteWorkspace => {
                let replacement = world
                    .query_filtered::<Entity, With<Workspace>>()
                    .iter(world)
                    .find(|&e| e != workspace)
                    .unwrap();
                let mut w = Workbench::new(world);
                w.activate_workspace(replacement).unwrap();
                w.delete_workspace(workspace).unwrap();
            }
        }
    }
}

struct GalleryPanel;
impl PanelDefinition for GalleryPanel {
    const KEY: &'static str = "bed2.gallery";
    const LABEL: &'static str = "Widget gallery";
}
struct NotesPanel;
impl PanelDefinition for NotesPanel {
    const KEY: &'static str = "bed2.notes";
    const LABEL: &'static str = "Notes";
}
fn gallery_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let context = world.resource::<DemoWorkbench>();
    let (light, dark) = (context.light, context.dark);
    let mut state = gallery::create_state(world, light, dark);
    state.embedded = true;
    let panel = register_panel_in_area(world, area, "Widget gallery", gallery::draw_gallery)?;
    world.entity_mut(panel).insert((
        state,
        bedtime_ui2::IconSource::from(bedtime_ui2::BuiltinIcon::Folder),
    ));
    Ok(panel)
}
fn notes_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, "Notes", draw_notes)?;
    world.entity_mut(panel).insert(Notes {
        text: "New panel. Drag its tab or split its area.".into(),
    });
    Ok(panel)
}
