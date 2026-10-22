//! Shared syntax contributions in two standalone editor views, without Workbench.
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
    analysis: bedtime_treesitter2::AnalysisSession,
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
        document: TextDocument::new("struct Cat {\n    name: String,\n}\nimpl Cat {\n    fn greet(&self) {\n        println!(\"Hello, {}!\", self.name);\n    }\n}\n"),
        analysis: bedtime_treesitter2::AnalysisSession::new(),
        views: Default::default(),
    });
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let mut state = world.entity_mut(context.view).take::<Example>().unwrap();
    bedtime_editor2::syntax::update_analysis(&mut state.analysis, &state.document, "rust").unwrap();
    let Example {
        analysis, views, ..
    } = &mut state;
    for view in views {
        view.set_analysis(analysis);
        view.syntax_palette = bedtime_editor2::syntax::SyntaxPalette::from_theme(context.theme);
    }
    if state.analysis.status() == &bedtime_treesitter2::AnalysisStatus::Pending {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
    ui.heading("Syntax contributions");
    ui.weak("Shared text and history · independent selections, scrolling and line numbers");
    ui.columns(2, |columns| {
        for (index, ui) in columns.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut state.views[index].settings.line_numbers,
                    "Line numbers",
                );
                state.views[index].editing_menu(ui);
                if ui.button("Fold all").clicked() {
                    let snapshot = state.document.snapshot().unwrap();
                    let _ = state.views[index].fold_all(&snapshot);
                }
                if ui.button("Unfold all").clicked() {
                    state.views[index].unfold_all();
                }
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
            let Example {
                document, views, ..
            } = &mut state;
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
