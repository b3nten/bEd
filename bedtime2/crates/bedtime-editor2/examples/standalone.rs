//! Two views of one in-memory document, without Workbench or language plugins.
use bedtime_editor2::{EditorView, Navigation, Reveal, TextDocument, TextSelection, TextSession};
use bedtime_ui2::*;
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bedtime text editor".into(),
                    resolution: (1100, 720).into(),
                    ..default()
                }),
                ..default()
            }),
            UiPlugin,
        ))
        .add_systems(Startup, setup)
        .run()
}

#[derive(Component)]
struct Example {
    document: TextDocument,
    views: [EditorView; 2],
}

fn setup(world: &mut World) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    let theme = world.spawn(themes::dark()).id();
    world
        .entity_mut(window)
        .insert((UiSurface, UsesTheme(theme)));
    world.spawn((Camera2d, UiCamera(window)));
    let view = register_view(world, window, draw);
    world.entity_mut(view).insert(Example {
        document: TextDocument::new("One document, two independent views.\n\nType in either pane. Undo and redo use the same history.\nUse Editing for wrapping, indentation and Find/Replace.\nLine numbers can be toggled per view.\nThe cursor uses bed1's motion.\n\nNo Workbench, LSP, syntax or structure plugin is installed.\n"),
        views: Default::default(),
    });
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let mut state = world.entity_mut(context.view).take::<Example>().unwrap();
    ui.heading("Text editor");
    ui.weak("Shared text and history · independent selections, scrolling and line numbers");
    ui.columns(2, |columns| {
        for (index, ui) in columns.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut state.views[index].settings.line_numbers,
                    "Line numbers",
                );
                state.views[index].editing_menu(ui);
                if ui.button("Go to beginning").clicked() {
                    let snapshot = state.document.snapshot().unwrap();
                    state.views[index]
                        .request_navigation(
                            &snapshot,
                            Navigation {
                                revision: snapshot.revision,
                                range: 0..0,
                                selection: Some(TextSelection::default()),
                                reveal: Reveal::Minimal,
                                focus: true,
                            },
                        )
                        .unwrap();
                }
            });
            ui.separator();
            let Example { document, views } = &mut state;
            if let Err(error) = views[index].show(
                ui,
                egui::Id::new(("standalone", index)),
                document,
                context.motion,
            ) {
                ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            }
        }
    });
    world.entity_mut(context.view).insert(state);
}
