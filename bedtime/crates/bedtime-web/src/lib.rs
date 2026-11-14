//! A web browser panel for workbench apps, on macOS (the system's web view,
//! WKWebView, shows the pages). The address bar is under the page: back,
//! forward, reload and the address, which shows a link's target while the
//! pointer is over it. The page's title is the tab's, and its address is
//! saved with the panel.
//!
//! The web view is a native view, not part of the app's picture. By default
//! (`DrawWebPages`) it's kept see-through over the panel, taking the mouse
//! and keyboard, while the panel draws snapshots of it taken one after
//! another: the page then clips, rounds, fades and sits under menus like the
//! rest of the app. With the setting off the web view shows itself, over
//! everything the app draws.
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{Panel, persistence::ReflectPersist};
use bevy::prelude::*;

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
mod view;

/// Open a web browser: a new one, in the focused area.
pub struct ShowWeb;
impl bedtime_workbench::Action for ShowWeb {
    const KEY: &'static str = "web.show";
    const LABEL: &'static str = "Web Browser";
}

pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::settings::SettingsAppExt;
        app.register_type::<WebPanel>()
            .init_resource::<ScreenWarp>()
            .init_resource::<WebOptions>()
            .register_setting::<DrawWebPages>();
        #[cfg(target_os = "macos")]
        view::install(app);
    }
}

bedtime_workbench::settings! {
    "Web" {
        /// Whether web panels draw their pages, or leave the system's web
        /// view showing over them.
        pub DrawWebPages: bool {
            default: true,
            key: "web.draw_pages",
            label: "Draw web pages",
            description: "Pages fit in like the rest of the app: under menus and dragged \
                          tabs, with rounded corners and screen effects. This takes more \
                          processor time while a page is changing. Turned off, the \
                          system's web view shows over the panel: quicker, but it covers \
                          whatever is drawn on top of it.",
            apply: |world, draw| world.resource_mut::<WebOptions>().draw_pages = *draw,
            visible: |_, _| cfg!(target_os = "macos"),
        }
    }
}

/// Show `url` in a web panel: one in the active layout already showing it
/// (focused), else a new one, somewhere other than `from` (the panel it was
/// opened from; with none, the focused area). Returns the panel.
pub fn open_url(world: &mut World, url: &str, from: Option<Entity>) -> Result<Entity, String> {
    use bedtime_workbench::{LayoutsWorldExt, PanelsWorldExt, Placement};
    if cfg!(not(target_os = "macos")) {
        return Err("Web pages show on macOS".into());
    }
    let layout = world.layouts_mut().ensure_active()?;
    let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
    let showing = world.layouts().panels(layout).into_iter().find(|&panel| {
        world
            .get::<WebPanel>(panel)
            .is_some_and(|web| same(&web.url, url))
    });
    if let Some(panel) = showing {
        world.layouts_mut().focus_panel(panel)?;
        return Ok(panel);
    }
    world.open_panel_at(
        WebPanel {
            url: url.to_string(),
        },
        from.map_or(Placement::Focused, Placement::From),
    )
}

/// On a web panel: an address it's to go to. The panel takes it when it's
/// next drawn; inserting it again replaces it, so the latest wins.
#[derive(Component, Clone, Debug)]
pub struct Navigate(pub String);

/// `text` as a web address, if it is plainly one: `https://…` or `http://…`
/// as written, `www.…` and a local server (`localhost…`, `127.0.0.1…`)
/// given their scheme. A bare `example.com` isn't: file names look the same
/// (`main.rs`, `notes.md`).
pub fn web_address(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let plainly = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("www.")
        || lower.starts_with("localhost")
        || lower.starts_with("127.0.0.1");
    (plainly && !text.contains(char::is_whitespace)).then(|| with_scheme(text))
}

/// How web panels show their pages, as the settings resolve it.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct WebOptions {
    /// See `DrawWebPages`.
    pub draw_pages: bool,
}

impl Default for WebOptions {
    fn default() -> Self {
        use bedtime_workbench::settings::Setting;
        Self {
            draw_pages: DrawWebPages::default(),
        }
    }
}

/// A web page. Its address is saved with the panel; a new panel is blank.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
#[cfg_attr(target_os = "macos", require(view::WebBar))]
pub struct WebPanel {
    pub url: String,
}

impl Panel for WebPanel {
    const LABEL: &'static str = "Web";

    #[cfg(target_os = "macos")]
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        view::draw(ui, world, context, panel);
    }

    /// Elsewhere, no pages.
    #[cfg(not(target_os = "macos"))]
    fn draw(ui: &mut egui::Ui, _: &mut World, context: &UiContext<'_>, _: Entity) {
        ui.painter().text(
            ui.available_rect_before_wrap().center(),
            egui::Align2::CENTER_CENTER,
            "Web pages show on macOS",
            egui::FontId::proportional(14.0),
            bedtime_ui::theme::color(context.theme.palette.muted),
        );
    }
}

/// How the app bends its picture on the way to the screen (a curved-screen
/// effect), if it does: the function takes a position on the screen and the
/// screen's rectangle, and gives the position in the picture that shows
/// there. The app keeps it current. A web view showing itself, which the
/// effect doesn't reach, uses it to stay inside its panel as the panel
/// appears.
#[derive(Resource, Default, Clone)]
pub struct ScreenWarp(pub Option<std::sync::Arc<WarpFn>>);

pub type WarpFn = dyn Fn(egui::Pos2, egui::Rect) -> egui::Pos2 + Send + Sync;

/// The largest rectangle on the screen that shows only what's inside `rect`
/// of the picture. `warp` is taken to push positions away from the screen's
/// centre, more the further out they are (as a curved screen does).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn fit_inside(rect: egui::Rect, screen: egui::Rect, warp: &WarpFn) -> egui::Rect {
    let centre = screen.center();
    let shown = |x: f32, y: f32| warp(egui::pos2(x, y), screen);
    let mut fit = rect;
    // Each side moves by how far the picture it shows is from the side of
    // `rect`, a few times over: the warp is gentle, so this settles.
    for _ in 0..12 {
        // Along a side, the picture reaches furthest out at the end
        // furthest from the centre line, and least far at the nearest.
        let ys = [fit.min.y, fit.max.y, centre.y.clamp(fit.min.y, fit.max.y)];
        let xs = [fit.min.x, fit.max.x, centre.x.clamp(fit.min.x, fit.max.x)];
        let left = ys
            .iter()
            .map(|y| shown(fit.min.x, *y).x)
            .fold(f32::MAX, f32::min);
        let right = ys
            .iter()
            .map(|y| shown(fit.max.x, *y).x)
            .fold(f32::MIN, f32::max);
        let top = xs
            .iter()
            .map(|x| shown(*x, fit.min.y).y)
            .fold(f32::MAX, f32::min);
        let bottom = xs
            .iter()
            .map(|x| shown(*x, fit.max.y).y)
            .fold(f32::MIN, f32::max);
        fit.min.x += rect.min.x - left;
        fit.max.x += rect.max.x - right;
        fit.min.y += rect.min.y - top;
        fit.max.y += rect.max.y - bottom;
        if fit.width() <= 0.0 || fit.height() <= 0.0 {
            return egui::Rect::from_center_size(rect.center(), egui::Vec2::ZERO);
        }
    }
    fit
}

/// A typed address as a URL: `example.com` is `https://example.com`, and a
/// local server is plain `http`.
pub fn with_scheme(address: &str) -> String {
    let address = address.trim();
    if address.contains("://") || address.starts_with("data:") || address.starts_with("about:") {
        address.to_string()
    } else if address.starts_with("localhost") || address.starts_with("127.0.0.1") {
        format!("http://{address}")
    } else {
        format!("https://{address}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A curved-screen effect: the screen shows the picture from further
    /// out, more so towards the edges.
    fn curved(position: egui::Pos2, screen: egui::Rect) -> egui::Pos2 {
        let centre = (position - screen.min) / screen.size() - egui::vec2(0.5, 0.5);
        let warped = egui::vec2(0.5, 0.5) + centre * (1.0 + 0.8 * 0.25 * centre.length_sq());
        screen.min + warped * screen.size()
    }

    #[test]
    fn a_fitted_rectangle_shows_only_its_panel_and_nearly_all_of_it() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        for panel in [
            egui::Rect::from_min_max(egui::pos2(4.0, 30.0), egui::pos2(1196.0, 760.0)),
            egui::Rect::from_min_max(egui::pos2(4.0, 30.0), egui::pos2(500.0, 400.0)),
            egui::Rect::from_min_max(egui::pos2(700.0, 420.0), egui::pos2(1196.0, 760.0)),
        ] {
            let fit = fit_inside(panel, screen, &curved);
            // Every point of its border shows something inside the panel.
            for step in 0..=20 {
                let along = step as f32 / 20.0;
                for point in [
                    fit.min + egui::vec2(fit.width() * along, 0.0),
                    fit.min + egui::vec2(fit.width() * along, fit.height()),
                    fit.min + egui::vec2(0.0, fit.height() * along),
                    fit.min + egui::vec2(fit.width(), fit.height() * along),
                ] {
                    assert!(
                        panel.expand(0.01).contains(curved(point, screen)),
                        "{panel:?}"
                    );
                }
            }
            // And it isn't needlessly small: each side touches the panel's.
            let shown = [fit.min, fit.max, fit.left_bottom(), fit.right_top()]
                .map(|corner| curved(corner, screen));
            let near = |pick: fn(egui::Pos2) -> f32, side: f32| {
                shown
                    .iter()
                    .any(|corner| (pick(*corner) - side).abs() < 0.5)
            };
            assert!(
                near(|p| p.x, panel.min.x) && near(|p| p.x, panel.max.x),
                "{panel:?}"
            );
            assert!(
                near(|p| p.y, panel.min.y) && near(|p| p.y, panel.max.y),
                "{panel:?}"
            );
        }
    }

    #[test]
    fn no_warp_leaves_the_rectangle_alone() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let panel = egui::Rect::from_min_max(egui::pos2(4.0, 30.0), egui::pos2(500.0, 400.0));
        assert_eq!(fit_inside(panel, screen, &|position, _| position), panel);
    }

    #[test]
    fn only_plain_web_addresses_are_taken_for_one() {
        let address = |text: &str| web_address(text);
        assert_eq!(
            address("https://bevy.org/learn").as_deref(),
            Some("https://bevy.org/learn")
        );
        assert_eq!(
            address("www.google.ca").as_deref(),
            Some("https://www.google.ca")
        );
        assert_eq!(
            address("localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            address("127.0.0.1:8000/x").as_deref(),
            Some("http://127.0.0.1:8000/x")
        );
        // File names, and anything else, aren't.
        for text in [
            "main.rs",
            "notes.md",
            "bevy.org",
            "src/lib.rs:12",
            "mailto:a@b.c",
            "two words",
        ] {
            assert_eq!(address(text), None, "{text}");
        }
    }

    #[test]
    fn typed_addresses_get_a_scheme() {
        assert_eq!(with_scheme(" example.com "), "https://example.com");
        assert_eq!(with_scheme("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(with_scheme("127.0.0.1:8000"), "http://127.0.0.1:8000");
        assert_eq!(with_scheme("http://example.com"), "http://example.com");
        assert_eq!(with_scheme("about:blank"), "about:blank");
    }
}
