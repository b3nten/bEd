//! Installed font families, discovered on a background thread.
use bevy::prelude::*;
use egui::{FontData, FontDefinitions};
use fontdb::{Database, Family, Query, Weight};
use std::{collections::BTreeMap, sync::Arc, thread::JoinHandle};

/// An installed font family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledFamily {
    pub name: String,
    /// Every glyph has the same advance, as editors and terminals need.
    pub monospaced: bool,
}

/// The installed font families. Initialising this resource starts discovery on
/// a background thread; nothing waits for it. A theme naming an installed
/// family keeps its current fonts until discovery finishes.
#[derive(Resource)]
pub struct InstalledFonts(State);

enum State {
    Loading(JoinHandle<Catalog>),
    Ready(Catalog),
    Failed(String),
}

struct Catalog {
    database: Database,
    families: Vec<InstalledFamily>,
}

impl Default for InstalledFonts {
    fn default() -> Self {
        Self::discover(|| {
            let mut database = Database::new();
            database.load_system_fonts();
            database
        })
    }
}

impl InstalledFonts {
    /// Discover the fonts `load` puts in a database, on a background thread.
    pub(crate) fn discover(load: impl FnOnce() -> Database + Send + 'static) -> Self {
        let worker = std::thread::Builder::new()
            .name("installed-fonts".into())
            .spawn(move || Catalog::new(load()));
        Self(match worker {
            Ok(worker) => State::Loading(worker),
            Err(error) => State::Failed(format!("Couldn't look for installed fonts: {error}")),
        })
    }

    pub fn loading(&self) -> bool {
        matches!(self.0, State::Loading(_))
    }

    /// Why installed fonts aren't available.
    pub fn error(&self) -> Option<&str> {
        match &self.0 {
            State::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Installed families by name; empty until discovery finishes.
    pub fn families(&self) -> &[InstalledFamily] {
        match &self.0 {
            State::Ready(catalog) => &catalog.families,
            _ => &[],
        }
    }

    pub fn contains(&self, family: &str) -> bool {
        self.families().iter().any(|f| f.name == family)
    }

    /// `family`'s regular and bold faces in front of Bedfont, if it's installed.
    pub(crate) fn definitions(&self, family: &str) -> Option<FontDefinitions> {
        let State::Ready(catalog) = &self.0 else {
            return None;
        };
        let database = &catalog.database;
        let face = |weight| {
            let id = database.query(&Query {
                families: &[Family::Name(family)],
                weight,
                ..Query::default()
            })?;
            database.with_face_data(id, |bytes, index| {
                let mut data = FontData::from_owned(bytes.to_vec());
                data.index = index;
                Arc::new(data)
            })
        };
        let regular = face(Weight::NORMAL)?;
        // fontdb picks the closest weight; a family without bold uses its regular.
        let bold = face(Weight::BOLD).unwrap_or_else(|| regular.clone());
        Some(super::with_primary(regular, bold))
    }
}

impl Catalog {
    fn new(database: Database) -> Self {
        // A family is monospaced when all its faces are.
        let mut families = BTreeMap::<String, bool>::new();
        for face in database.faces() {
            let Some((name, _)) = face.families.first() else {
                continue;
            };
            // Names starting with '.' are the platform's private UI fonts.
            if name.starts_with('.') {
                continue;
            }
            *families.entry(name.clone()).or_insert(true) &= face.monospaced;
        }
        Self {
            database,
            families: families
                .into_iter()
                .map(|(name, monospaced)| InstalledFamily { name, monospaced })
                .collect(),
        }
    }
}

/// Take discovery's result once it's finished, without blocking.
pub(crate) fn poll(fonts: Option<ResMut<InstalledFonts>>) {
    let Some(mut fonts) = fonts else {
        return;
    };
    if !matches!(&fonts.0, State::Loading(worker) if worker.is_finished()) {
        return;
    }
    let State::Loading(worker) = std::mem::replace(&mut fonts.0, State::Failed(String::new()))
    else {
        unreachable!();
    };
    fonts.0 = match worker.join() {
        Ok(catalog) => State::Ready(catalog),
        Err(_) => State::Failed("Looking for installed fonts failed".into()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{DEFAULT_FAMILY, monospace_bold, resolve};
    use bevy::ecs::system::RunSystemOnce;

    /// A database holding only the bundled Bedfont faces.
    fn bundled() -> Database {
        let mut database = Database::new();
        for (name, face) in crate::fonts::definitions().font_data {
            if name.starts_with(DEFAULT_FAMILY) {
                database.load_font_data(face.font.to_vec());
            }
        }
        database
    }

    fn finish(world: &mut World) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while world.resource::<InstalledFonts>().loading() {
            assert!(std::time::Instant::now() < deadline);
            world.run_system_once(poll).unwrap();
            std::thread::yield_now();
        }
    }

    fn primary(fonts: &FontDefinitions) -> bool {
        fonts.font_data.contains_key("Primary Regular")
    }

    #[test]
    fn a_named_family_waits_for_discovery_then_applies_or_falls_back() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let mut world = World::new();
        world.insert_resource(InstalledFonts::discover(move || {
            wait.recv().unwrap();
            bundled()
        }));
        // Nothing blocks while discovery runs; the bundled fonts stay usable.
        world.run_system_once(poll).unwrap();
        assert!(world.resource::<InstalledFonts>().loading());
        assert!(world.resource::<InstalledFonts>().families().is_empty());
        assert!(resolve(&world, Some("Anything")).is_none());
        assert!(!primary(&resolve(&world, None).unwrap()));

        release.send(()).unwrap();
        finish(&mut world);
        let installed = world.resource::<InstalledFonts>();
        assert!(installed.error().is_none());
        let [family] = installed.families() else {
            panic!("{:?}", installed.families());
        };
        assert!(family.monospaced);
        let name = family.name.clone();
        assert!(primary(&resolve(&world, Some(&name)).unwrap()));
        assert!(!primary(&resolve(&world, Some("Missing family")).unwrap()));
        assert!(!primary(&resolve(&world, Some(DEFAULT_FAMILY)).unwrap()));
    }

    #[test]
    fn without_discovery_a_named_family_is_bedfont() {
        let world = World::new();
        assert!(!primary(&resolve(&world, Some("Anything")).unwrap()));
    }

    #[test]
    fn installed_regular_and_bold_replace_primary_fonts_and_keep_unicode_fallbacks() {
        let mut world = World::new();
        world.insert_resource(InstalledFonts::discover(bundled));
        finish(&mut world);
        let installed = world.resource::<InstalledFonts>();
        let family = installed.families()[0].name.clone();
        let context = egui::Context::default();
        context.set_fonts(installed.definitions(&family).unwrap());
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                let regular = egui::FontId::monospace(15.5);
                let bold = monospace_bold(15.5);
                assert_eq!(fonts.row_height(&regular), fonts.row_height(&bold));
                assert_eq!(
                    fonts.glyph_width(&regular, 'M'),
                    fonts.glyph_width(&bold, 'M')
                );
                for text in ["Regular", "∑ ⏵"] {
                    let galley =
                        fonts.layout_no_wrap(text.into(), regular.clone(), egui::Color32::WHITE);
                    assert!(galley.size().x > 0.0);
                }
                let normal = fonts.layout_no_wrap("Bold".into(), regular, egui::Color32::WHITE);
                let bold = fonts.layout_no_wrap("Bold".into(), bold, egui::Color32::WHITE);
                assert_ne!(normal.rows[0].visuals.mesh, bold.rows[0].visuals.mesh);
            });
        });
        output.textures_delta.clear();
        assert!(installed.definitions("Missing family").is_none());
    }

    #[test]
    #[ignore = "Reads installed fonts to time discovery; run explicitly"]
    fn installed_font_discovery_latency_probe() {
        let start = std::time::Instant::now();
        let mut world = World::new();
        world.init_resource::<InstalledFonts>();
        finish(&mut world);
        let families = world.resource::<InstalledFonts>().families();
        eprintln!(
            "Installed fonts: {:?}; {} families, {} monospaced",
            start.elapsed(),
            families.len(),
            families.iter().filter(|f| f.monospaced).count()
        );
    }
}
