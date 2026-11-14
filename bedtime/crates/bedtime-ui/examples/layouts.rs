//! Layouts without bedtime-workbench: two panels in a window, which can be
//! split, joined, resized and rearranged by dragging tabs and corners. A
//! status line under the panels shows how an app draws its own body.
//!
//! ```sh
//! cargo run -p bedtime-ui --example layouts
//! ```
use bedtime_ui::{
    UiContext, UiPlugin, egui,
    layouts::{
        InArea, LayoutCamera, LayoutSurface, LayoutsPlugin, LayoutsWorldExt, Panel, SurfaceBody,
        view,
    },
};
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bedtime UI — Layouts".into(),
                resolution: (1000, 700).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((bedtime_clipboard::ClipboardPlugin, UiPlugin, LayoutsPlugin))
        .add_systems(Startup, setup)
        .run()
}

/// A panel that shows some text.
#[bedtime_ui::panel]
#[derive(Default)]
struct Note {
    text: String,
}

impl Panel for Note {
    const LABEL: &'static str = "Note";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            if let Some(mut note) = world.get_mut::<Note>(panel) {
                ui.add(egui::TextEdit::multiline(&mut note.text).desired_width(f32::INFINITY));
            }
        });
    }
}

fn setup(world: &mut World) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world
        .entity_mut(window)
        .insert((LayoutSurface::default(), SurfaceBody(body)));
    world.spawn((Camera2d, LayoutCamera));

    let layout = world.layouts_mut().create("Main").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let area = world.layouts().destination_area(layout).unwrap();
    for text in [
        "Drag a tab onto another area's edge to split it.",
        "Drag from a corner to split or join areas.",
    ] {
        world.spawn((Note { text: text.into() }, InArea(area)));
    }
}

/// The panels, with a line of text under them.
fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let layout = world.layouts().active();
    let panels = layout.map_or(0, |layout| world.layouts().panels(layout).len());
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.label(format!("{panels} panels"));
    });
    view::show(ui, world, context);
}
