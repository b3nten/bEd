//! Application and scene contributions to the reusable UI2 status bar.
use bedtime_ui2::{
    BuiltinIcon, MotionPolicy, StatusItem, StatusSide, UiDiagnostics, UiFrame,
    register_status_item, register_status_tooltip,
};
use bedtime_workbench2::{ActiveWorkspace, Layout, Panel, Workbench, Workspace};
use bevy::prelude::*;

pub(crate) fn setup(world: &mut World, bar: Entity) {
    let mut descriptor = StatusItem::new(
        StatusSide::Left,
        BuiltinIcon::Folder,
        "Workspace and layout",
        230.0,
    );
    descriptor.priority = 100;
    let item = register_status_item(world, bar, descriptor, workspace);
    register_status_tooltip(world, item, workspace_details);

    let mut descriptor =
        StatusItem::new(StatusSide::Left, BuiltinIcon::Files, "Focused panel", 190.0);
    descriptor.priority = 50;
    let item = register_status_item(world, bar, descriptor, focused);
    register_status_tooltip(world, item, focused_details);

    let item = register_status_item(
        world,
        bar,
        StatusItem::new(StatusSide::Right, BuiltinIcon::Image, "UI rendering", 130.0),
        rendering,
    );
    register_status_tooltip(world, item, rendering_details);

    register_status_item(
        world,
        bar,
        StatusItem::new(
            StatusSide::Right,
            BuiltinIcon::Config,
            "Motion setting",
            150.0,
        ),
        motion,
    );
}

fn workspace(
    frame: UiFrame,
    active: Res<ActiveWorkspace>,
    workspaces: Query<&Workspace>,
    layouts: Query<&Layout>,
) {
    let workspace = workspaces.get(active.entity()).unwrap();
    let layout = layouts.get(workspace.active_layout()).unwrap();
    frame
        .ui
        .label(format!("{} / {}", workspace.name, layout.name));
}

fn workspace_details(frame: UiFrame, world: &mut World) {
    let w = Workbench::new(world);
    let workspace = w.active_workspace();
    let layout = w.active_layout();
    let count = w.panels(layout).len();
    frame
        .ui
        .heading(&world.get::<Workspace>(workspace).unwrap().name);
    frame.ui.label(format!(
        "Layout: {}",
        world.get::<Layout>(layout).unwrap().name
    ));
    frame
        .ui
        .label(format!("{count} panels retained in this layout"));
    frame
        .ui
        .weak("Use Controls to switch workspaces and layouts.");
}

fn focused(frame: UiFrame, world: &mut World) {
    let w = Workbench::new(world);
    let panel = w.focused_panel(w.active_layout());
    frame.ui.label(panel.map_or("No focused panel", |panel| {
        world.get::<Panel>(panel).unwrap().title.as_str()
    }));
}

fn focused_details(frame: UiFrame, world: &mut World) {
    let w = Workbench::new(world);
    let panel = w.focused_panel(w.active_layout());
    if let Some(panel) = panel {
        let data = world.get::<Panel>(panel).unwrap();
        frame.ui.heading(&data.title);
        frame
            .ui
            .label("ContextCommands resolve this panel's handlers before the workbench handlers.");
        if world.get::<crate::workbench_demo::Notes>(panel).is_some() {
            frame
                .ui
                .weak("The focused Notes panel also contributes its own menu.");
        }
    } else {
        frame.ui.label("The focused area has no selected panel.");
    }
}

fn rendering(frame: UiFrame, diagnostics: Query<&UiDiagnostics>) {
    let diagnostics = diagnostics.get(frame.surface).unwrap();
    frame.ui.label(format!("{:.2} ms UI", diagnostics.draw_ms));
}

fn rendering_details(frame: UiFrame, diagnostics: Query<&UiDiagnostics>) {
    let diagnostics = diagnostics.get(frame.surface).unwrap();
    frame.ui.heading("UI rendering");
    frame.ui.label(format!(
        "{} vertices / {} indices",
        diagnostics.vertices, diagnostics.indices
    ));
    frame.ui.label(format!(
        "{} icon textures / {} viewports",
        diagnostics.icon_textures, diagnostics.viewport_textures
    ));
    frame.ui.label(format!(
        "{} active animations",
        diagnostics.active_animations
    ));
}

fn motion(frame: UiFrame, mut policies: Query<&mut MotionPolicy>) {
    let mut policy = policies.get_mut(frame.surface).unwrap();
    if frame
        .ui
        .button(if policy.enabled {
            "Motion: On"
        } else {
            "Motion: Off"
        })
        .clicked()
    {
        policy.enabled = !policy.enabled;
    }
}
