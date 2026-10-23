//! Application font selection; UI2 owns the bundled faces and Unicode fallbacks.
use bedtime_ui2::{UiCamera, egui, fonts::DEFAULT_FAMILY};
use bevy::prelude::*;
use bevy_egui::EguiContext;
use fontdb::{Database, Family, Query, Weight};
use std::{
    collections::BTreeSet,
    sync::{Arc, LazyLock},
    thread::JoinHandle,
};

struct Catalog {
    database: Database,
    families: Vec<String>,
}

impl Catalog {
    fn scan() -> Self {
        let mut database = Database::new();
        database.load_system_fonts();
        let families: BTreeSet<_> = database
            .faces()
            // Editor wrapping and terminal cells require a fixed-width primary face.
            .filter(|face| face.monospaced)
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .filter(|name| name != DEFAULT_FAMILY && !name.starts_with('.'))
            .collect();
        Self {
            database,
            families: std::iter::once(DEFAULT_FAMILY.to_owned())
                .chain(families)
                .collect(),
        }
    }
}

// The application owns one scan, started alongside settings rather than by drawing
// Appearance. Only join a finished worker; font discovery never blocks a frame.
#[derive(Resource)]
enum SystemFonts {
    Loading(JoinHandle<Catalog>),
    Ready(Catalog),
    Failed(String),
}
impl Default for SystemFonts {
    fn default() -> Self {
        match std::thread::Builder::new()
            .name("bed2-font-catalog".into())
            .spawn(Catalog::scan)
        {
            Ok(worker) => Self::Loading(worker),
            Err(error) => Self::Failed(format!("Could not discover installed fonts: {error}")),
        }
    }
}
pub fn install(app: &mut App) {
    app.init_resource::<SystemFonts>();
}
fn poll_catalog(world: &mut World) {
    if world
        .get_resource::<SystemFonts>()
        .is_some_and(|state| matches!(state, SystemFonts::Loading(worker) if worker.is_finished()))
    {
        let SystemFonts::Loading(worker) = world.remove_resource::<SystemFonts>().unwrap() else {
            unreachable!();
        };
        world.insert_resource(match worker.join() {
            Ok(catalog) => SystemFonts::Ready(catalog),
            Err(_) => SystemFonts::Failed("Installed font discovery failed; using Bedfont".into()),
        });
    }
}

pub fn loading(world: &World) -> bool {
    matches!(
        world.get_resource::<SystemFonts>(),
        Some(SystemFonts::Loading(_))
    )
}
pub fn error(world: &World) -> Option<&str> {
    match world.get_resource::<SystemFonts>() {
        Some(SystemFonts::Failed(error)) => Some(error),
        _ => None,
    }
}
pub fn available(world: &World) -> &[String] {
    static BUNDLED: LazyLock<[String; 1]> = LazyLock::new(|| [DEFAULT_FAMILY.into()]);
    match world.get_resource::<SystemFonts>() {
        Some(SystemFonts::Ready(catalog)) => &catalog.families,
        _ => &*BUNDLED,
    }
}

fn definitions(database: &Database, family: &str) -> Option<egui::FontDefinitions> {
    let face = |weight| {
        let id = database.query(&Query {
            families: &[Family::Name(family)],
            weight,
            ..Query::default()
        })?;
        if !database.face(id)?.monospaced {
            return None;
        }
        database.with_face_data(id, |bytes, index| {
            let mut data = egui::FontData::from_owned(bytes.to_vec());
            data.index = index;
            Arc::new(data)
        })
    };
    let regular = face(Weight::NORMAL)?;
    // fontdb chooses the closest weight; families without bold retain their own face.
    let bold = face(Weight::BOLD).unwrap_or_else(|| regular.clone());
    let mut fonts = bedtime_ui2::fonts::definitions();
    fonts.font_data.insert("System Regular".into(), regular);
    fonts.font_data.insert("System Bold".into(), bold);
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&family)
            .unwrap()
            .insert(0, "System Regular".into());
    }
    fonts
        .families
        .get_mut(&bedtime_ui2::fonts::monospace_bold(1.0).family)
        .unwrap()
        .insert(0, "System Bold".into());
    Some(fonts)
}

#[derive(Component)]
struct AppliedFont {
    requested: String,
    #[cfg(feature = "automation")]
    resolved: String,
    context: egui::Context,
}

/// Runs after UI2 configures new contexts and before drawing. Font changes only
/// replace the atlas; panels, canonical editor contents and PTYs retain ownership.
pub fn apply(world: &mut World) {
    poll_catalog(world);
    let active = world
        .query_filtered::<Entity, With<bedtime_workbench2::ActiveWorkspace>>()
        .iter(world)
        .next();
    let desired = active
        .map(|workspace| crate::settings::effective_settings(world, workspace))
        .unwrap_or_else(|| {
            world
                .resource::<crate::settings::GlobalPreferences>()
                .0
                .clone()
        });
    let cameras: Vec<_> = world
        .query_filtered::<Entity, With<UiCamera>>()
        .iter(world)
        .collect();
    for camera in cameras {
        let context = world
            .get_mut::<EguiContext>(camera)
            .unwrap()
            .get_mut()
            .clone();
        if world.get::<AppliedFont>(camera).is_some_and(|applied| {
            applied.requested == desired.font_family && applied.context == context
        }) {
            continue;
        }
        // Defer a saved system-family preference until discovery finishes. Do not
        // record a temporary fallback as applied, or it would never be retried.
        if desired.font_family != DEFAULT_FAMILY && loading(world) {
            continue;
        }
        let system = match world.get_resource::<SystemFonts>() {
            Some(SystemFonts::Ready(catalog)) if desired.font_family != DEFAULT_FAMILY => {
                definitions(&catalog.database, &desired.font_family)
            }
            _ => None,
        };
        #[cfg(feature = "automation")]
        let resolved = if system.is_some() {
            desired.font_family.clone()
        } else {
            DEFAULT_FAMILY.into()
        };
        context.set_fonts(system.unwrap_or_else(bedtime_ui2::fonts::definitions));
        world.entity_mut(camera).insert(AppliedFont {
            requested: desired.font_family.clone(),
            #[cfg(feature = "automation")]
            resolved,
            context,
        });
    }
}

#[cfg(feature = "automation")]
pub fn inspect(world: &mut World) -> serde_json::Value {
    let applied: Vec<_> = world
        .query::<&AppliedFont>()
        .iter(world)
        .map(|font| serde_json::json!({"requested": font.requested, "resolved": font.resolved}))
        .collect();
    serde_json::json!({"available": available(world), "loading": loading(world), "error": error(world), "applied": applied})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_discovery_keeps_bundled_fonts_available_and_retries_saved_family() {
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            wait.recv().unwrap();
            Catalog {
                database: Database::new(),
                families: vec![DEFAULT_FAMILY.into()],
            }
        });
        let mut world = World::new();
        world.insert_resource(SystemFonts::Loading(worker));
        world.insert_resource(crate::settings::GlobalPreferences(
            crate::settings::Settings {
                font_family: "Missing saved family".into(),
                ..default()
            },
        ));
        let camera = world
            .spawn((UiCamera(Entity::PLACEHOLDER), EguiContext::default()))
            .id();
        // These calls must finish while the worker is deliberately blocked.
        poll_catalog(&mut world);
        assert!(loading(&world));
        assert_eq!(available(&world), &[DEFAULT_FAMILY]);
        apply(&mut world);
        assert!(world.get::<AppliedFont>(camera).is_none());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while loading(&world) {
            assert!(std::time::Instant::now() < deadline);
            poll_catalog(&mut world);
            std::thread::yield_now();
        }
        apply(&mut world);
        assert_eq!(
            world.get::<AppliedFont>(camera).unwrap().requested,
            "Missing saved family"
        );
        assert!(error(&world).is_none());
    }

    #[test]
    #[ignore = "Reads installed fonts to measure discovery; run explicitly"]
    fn system_font_discovery_latency_probe() {
        let start = std::time::Instant::now();
        let catalog = Catalog::scan();
        eprintln!(
            "System font scan: {:?}; {} faces, {} monospace families",
            start.elapsed(),
            catalog.database.faces().count(),
            catalog.families.len()
        );
        assert_eq!(catalog.families[0], DEFAULT_FAMILY);
    }

    #[test]
    fn installed_regular_and_bold_faces_replace_primary_fonts_and_keep_unicode_fallbacks() {
        let mut database = Database::new();
        for (name, face) in bedtime_ui2::fonts::definitions().font_data {
            if !name.starts_with(DEFAULT_FAMILY) {
                continue;
            }
            database.load_font_data(face.font.to_vec());
        }
        let family = database
            .faces()
            .find(|face| face.monospaced)
            .unwrap()
            .families[0]
            .0
            .clone();
        let context = egui::Context::default();
        context.set_fonts(definitions(&database, &family).unwrap());
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                let regular = egui::FontId::monospace(15.5);
                let bold = bedtime_ui2::fonts::monospace_bold(15.5);
                assert_eq!(fonts.row_height(&regular), fonts.row_height(&bold));
                assert_eq!(
                    fonts.glyph_width(&regular, 'M'),
                    fonts.glyph_width(&bold, 'M')
                );
                for text in ["Regular", "∑ ⏵"] {
                    assert!(
                        fonts
                            .layout_no_wrap(text.into(), regular.clone(), egui::Color32::WHITE)
                            .size()
                            .x
                            > 0.0
                    );
                }
                let normal = fonts.layout_no_wrap("Bold".into(), regular, egui::Color32::WHITE);
                let bold = fonts.layout_no_wrap("Bold".into(), bold, egui::Color32::WHITE);
                assert_ne!(normal.rows[0].visuals.mesh, bold.rows[0].visuals.mesh);
            });
        });
        output.textures_delta.clear();
        assert!(definitions(&database, "Missing font family").is_none());
    }
}
