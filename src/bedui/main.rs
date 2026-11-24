//! The playground for bedtime-ui: every component on its own, with the
//! controls that change it. Three panels: the stories, the one that's
//! chosen, and its controls.
//!
//! ```sh
//! cargo run -p bedui
//! ```
//!
//! A story is a function in bedtime-ui (`bedtime::ui::stories`), next to the
//! component it shows; this app only lists and draws them.
use bedtime::BedtimeWorldExt as _;
use bedtime::{
    AppPlugins,
    automation::AutomationPlugin,
    persistence::ReflectPersist,
    ui::{
        UiContext,
        layouts::{InArea, LAYOUT_EXTENT, Panel, Place, SplitAxis},
        prelude::*,
        stories::{self, Knobs, Story},
    },
    workbench::{OnWorkspaceOpen, PanelsAppExt, ShowPanel},
};
use bevy::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins((
            AppPlugins::new("Bedui"),
            AutomationPlugin::from_env(),
            PlaygroundPlugin,
        ))
        .run()
}

struct PlaygroundPlugin;

impl Plugin for PlaygroundPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Playground::new(stories::builtin()))
            .add_panel::<Stories>()
            .add_panel::<Canvas>()
            .add_panel::<Controls>()
            .add_systems(OnWorkspaceOpen, first_layout);
    }
}

/// The stories, which one is shown, and each one's knobs.
#[derive(Resource)]
struct Playground {
    stories: Vec<Story>,
    selected: usize,
    knobs: Vec<Knobs>,
}

impl Playground {
    fn new(stories: Vec<Story>) -> Self {
        Self {
            knobs: vec![Knobs::default(); stories.len()],
            stories,
            selected: 0,
        }
    }
}

/// On the first run: the stories on the left, the canvas in the middle, the
/// controls on the right.
fn first_layout(world: &mut World) {
    if world.bedtime().layouts().active().is_some() {
        return;
    }
    let stories = world.bedtime_mut().open_panel(Stories);
    let Some(left) = world.get::<InArea>(stories).map(|area| area.0) else {
        return;
    };
    let bedtime = world.bedtime_mut();
    let layouts = bedtime.layouts_mut();
    let minimum = [100, 100];
    let Ok(middle) = layouts.split_area(left, SplitAxis::Horizontal, LAYOUT_EXTENT / 5, minimum)
    else {
        return;
    };
    let right = layouts.split_area(
        middle,
        SplitAxis::Horizontal,
        LAYOUT_EXTENT * 3 / 4,
        minimum,
    );
    if let Ok(right) = right {
        world.spawn((Controls, Place::in_area(right).focus(false)));
    }
    world.spawn((Canvas, Place::in_area(middle)));
}

/// The list of stories.
#[bedtime::ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
struct Stories;

impl ShowPanel for Stories {
    const SHOW: &'static str = "stories.show";
}

impl Panel for Stories {
    const LABEL: &'static str = "Stories";

    fn draw(ui: &mut Ui, world: &mut World, _: &UiContext<'_>, _: Entity) {
        let mut playground = world.resource_mut::<Playground>();
        let mut selected = playground.selected;
        egui::ScrollArea::vertical().show(ui, |ui| {
            Column::new()
                .gap(Space::Xs)
                .padding(Space::Sm)
                .show(ui, |ui| {
                    let mut group = "";
                    for (index, story) in playground.stories.iter().enumerate() {
                        if story.group != group {
                            group = story.group;
                            if index > 0 {
                                ui.add_space(Space::Sm.px(ui));
                            }
                            ui.add(Text::caption(group).muted());
                        }
                        let row = Button::new(story.name)
                            .ghost()
                            .fill_width()
                            .align(egui::Align::Min)
                            .selected(index == selected);
                        if ui.add(row).clicked() {
                            selected = index;
                        }
                    }
                });
        });
        playground.selected = selected;
    }
}

/// The chosen story.
#[bedtime::ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
struct Canvas;

impl ShowPanel for Canvas {
    const SHOW: &'static str = "canvas.show";
}

impl Panel for Canvas {
    const LABEL: &'static str = "Canvas";

    fn draw(ui: &mut Ui, world: &mut World, _: &UiContext<'_>, _: Entity) {
        let mut playground = world.resource_mut::<Playground>();
        let selected = playground.selected;
        let Some(story) = playground.stories.get(selected).copied() else {
            return;
        };
        let knobs = &mut playground.knobs[selected];
        knobs.begin();
        egui::ScrollArea::both().show(ui, |ui| {
            Column::new()
                .gap(Space::Xl)
                .padding(Space::Xl)
                .show(ui, |ui| {
                    ui.add(Text::heading(story.name));
                    (story.draw)(ui, knobs);
                });
        });
    }
}

/// The chosen story's controls.
#[bedtime::ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
struct Controls;

impl ShowPanel for Controls {
    const SHOW: &'static str = "controls.show";
}

impl Panel for Controls {
    const LABEL: &'static str = "Controls";

    fn draw(ui: &mut Ui, world: &mut World, _: &UiContext<'_>, _: Entity) {
        let mut playground = world.resource_mut::<Playground>();
        let selected = playground.selected;
        let Some(knobs) = playground.knobs.get_mut(selected) else {
            return;
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            Column::new()
                .padding(Space::Lg)
                .show(ui, |ui| knobs.controls(ui));
        });
    }
}
