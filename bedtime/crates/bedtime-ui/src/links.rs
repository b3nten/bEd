//! Links that leave what's showing them: a web address clicked in a
//! terminal, in Markdown, in a tooltip. `world.open_externally_from(url,
//! panel)` (or `open_externally(url)`, when no panel asked) hands one to the
//! app's `ExternalOpener`, which by default gives it to the system (its
//! browser, its mail program). An app can put its own in, to show web pages
//! in a panel of its own, say: the panel it was opened from tells it where
//! (`Placement::From`).
use crate::egui;
use bevy::prelude::*;

/// What opens a link that leaves the app, given the panel it was opened
/// from, if one was. Replace the resource to open some (or all) another way;
/// `open_with_system` is the default, and what to fall back to.
#[derive(Resource, Clone, Copy)]
pub struct ExternalOpener(pub fn(&mut World, &str, Option<Entity>) -> Result<(), String>);

impl Default for ExternalOpener {
    fn default() -> Self {
        Self(|_, url, _| open_with_system(url))
    }
}

/// Open a web link (or any URL) with the system: `open` on macOS, else
/// `xdg-open`.
pub fn open_with_system(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open {url}: {error}"))
}

/// `world.open_externally_from(url, panel)`, `world.open_externally(url)`.
pub trait LinksWorldExt {
    /// Open a link that leaves the app, as the app's `ExternalOpener` does
    /// (the system's way, without one). For a link no panel asked for;
    /// from a panel, `open_externally_from`.
    fn open_externally(&mut self, url: &str) -> Result<(), String>;
    /// Open a link clicked (or asked for) in `panel`, so an opener showing
    /// it in the app can put it somewhere else than on that panel.
    fn open_externally_from(&mut self, url: &str, panel: Entity) -> Result<(), String>;
}

impl LinksWorldExt for World {
    fn open_externally(&mut self, url: &str) -> Result<(), String> {
        open(self, url, None)
    }

    fn open_externally_from(&mut self, url: &str, panel: Entity) -> Result<(), String> {
        open(self, url, Some(panel))
    }
}

fn open(world: &mut World, url: &str, from: Option<Entity>) -> Result<(), String> {
    let open = world
        .get_resource::<ExternalOpener>()
        .copied()
        .unwrap_or_default();
    (open.0)(world, url, from)
}

/// From drawing code that has no world: have `url` opened as
/// `world.open_externally` would, once this frame's drawing is done.
pub fn request_open_externally(context: &egui::Context, url: impl Into<String>) {
    let url = url.into();
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<String>>(requests())
            .push(url);
    });
}

fn requests() -> egui::Id {
    egui::Id::new("bedtime.links.requested")
}

/// Open what drawing asked for.
pub(crate) fn open_requested(world: &mut World) {
    let contexts: Vec<egui::Context> = world
        .query::<&mut bevy_egui::EguiContext>()
        .iter_mut(world)
        .map(|mut context| context.get_mut().clone())
        .collect();
    for context in contexts {
        let urls = context
            .data_mut(|data| data.remove_temp::<Vec<String>>(requests()))
            .unwrap_or_default();
        for url in urls {
            if let Err(error) = world.open_externally(&url) {
                warn!("{error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Opened(Vec<(String, Option<Entity>)>);

    #[test]
    fn an_apps_opener_gets_links_asked_with_or_without_the_world() {
        let mut world = World::new();
        world.init_resource::<Opened>();
        world.insert_resource(ExternalOpener(|world, url, from| {
            world
                .resource_mut::<Opened>()
                .0
                .push((url.to_string(), from));
            Ok(())
        }));
        world.open_externally("https://example.com").unwrap();
        let panel = world.spawn_empty().id();
        world
            .open_externally_from("https://bevy.org", panel)
            .unwrap();

        let mut drawn = bevy_egui::EguiContext::default();
        let context = drawn.get_mut().clone();
        world.spawn(drawn);
        request_open_externally(&context, "mailto:someone@example.com");
        open_requested(&mut world);
        // Asked once, opened once.
        open_requested(&mut world);
        assert_eq!(
            world.resource::<Opened>().0,
            [
                ("https://example.com".to_owned(), None),
                ("https://bevy.org".to_owned(), Some(panel)),
                ("mailto:someone@example.com".to_owned(), None)
            ]
        );
    }
}
