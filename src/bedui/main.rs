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
    workbench::{
        OnWorkspaceOpen, PanelsAppExt, Show, ShowCommandPalette, ShowPanel,
        appearance::RainbowInk,
        bound::{ActionButton, SettingToggle},
        settings::SettingsPanel,
    },
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
        // bedtime-ui's stories, and one of this app's own: widgets bound to
        // the world need an app's actions and settings to show.
        let mut all = stories::builtin();
        all.push(Story {
            group: "Bound to the world",
            name: "Actions and Settings",
            draw: bound,
        });
        app.insert_resource(Playground::new(all))
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

    fn draw(ui: &mut Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw::with_system(ui, world, context, panel, stories_panel);
    }
}

fn stories_panel(
    PanelUi { ui, .. }: PanelUi<Stories>,
    playground: Res<Playground>,
    mut commands: Commands,
) {
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
                    let row = ListRow::new(story.name).selected(index == playground.selected);
                    if ui.add(row).clicked() {
                        commands.queue(move |world: &mut World| {
                            world.resource_mut::<Playground>().selected = index;
                        });
                    }
                }
            });
    });
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

    fn draw(ui: &mut Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw::with_system(ui, world, context, panel, canvas);
    }
}

/// A story asks for its knobs as it draws, so it draws with a copy of them,
/// kept when the draw is done.
fn canvas(
    PanelUi { ui, .. }: PanelUi<Canvas>,
    playground: Res<Playground>,
    mut commands: Commands,
) {
    let selected = playground.selected;
    let (Some(story), Some(knobs)) = (
        playground.stories.get(selected).copied(),
        playground.knobs.get(selected),
    ) else {
        return;
    };
    let mut knobs = knobs.clone();
    knobs.begin();
    egui::ScrollArea::both().show(ui, |ui| {
        Column::new()
            .gap(Space::Xl)
            .padding(Space::Xl)
            .show(ui, |ui| {
                ui.add(Text::heading(story.name));
                (story.draw)(ui, &mut knobs);
            });
    });
    commands.queue(move |world: &mut World| {
        world.resource_mut::<Playground>().knobs[selected] = knobs;
    });
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

    fn draw(ui: &mut Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw::with_system(ui, world, context, panel, controls);
    }
}

fn controls(
    PanelUi { ui, .. }: PanelUi<Controls>,
    playground: Res<Playground>,
    mut commands: Commands,
) {
    let selected = playground.selected;
    let Some(knobs) = playground.knobs.get(selected) else {
        return;
    };
    let mut knobs = knobs.clone();
    egui::ScrollArea::vertical().show(ui, |ui| {
        Column::new()
            .padding(Space::Lg)
            .show(ui, |ui| knobs.controls(ui));
    });
    commands.queue(move |world: &mut World| {
        world.resource_mut::<Playground>().knobs[selected] = knobs;
    });
}

/// Widgets that are an action or a setting: nothing is passed to them and
/// nothing is done with what they return. They read the world the story is
/// drawn in and ask for the change themselves.
fn bound(ui: &mut Ui, _: &mut Knobs) {
    Column::new().gap(Space::Xl).show(ui, |ui| {
        Column::new().gap(Space::Sm).show(ui, |ui| {
            ui.add(Text::caption("ActionButton: the action's label, whether it can run, its keys on hover").muted());
            Row::new().show(ui, |ui| {
                ui.add(ActionButton::<Show<SettingsPanel>>::new());
                ui.add(ActionButton::<ShowCommandPalette>::new().ghost());
                ui.add(ActionButton::<Show<Controls>>::new().text("Another Controls panel").ghost());
            });
        });
        Column::new().gap(Space::Sm).show(ui, |ui| {
            ui.add(Text::caption("SettingToggle: the setting's value, set when it's flipped (see it change in Settings)").muted());
            ui.add(SettingToggle::<RainbowInk>::new());
            ui.add(SettingToggle::<RainbowInk>::new().checkbox().label("The same setting, as a checkbox"));
        });
    });
}
