//! Typography: the bundled Bedfont, the theme's font family, and (with the
//! `system-fonts` feature) the installed families it can name.
//!
//! The UI runtime applies fonts itself: when a context first draws, and when
//! the theme's `font_family` changes. Hosts never call `set_fonts`.
use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

#[cfg(feature = "system-fonts")]
pub(crate) mod installed;
#[cfg(feature = "system-fonts")]
pub use installed::{InstalledFamily, InstalledFonts};

pub const DEFAULT_FAMILY: &str = "Bedfont";

/// The bold companion to `FontId::monospace`, installed by UI.
pub fn monospace_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Monospace Bold".into()))
}

/// Bedfont (bundled Nordwand Mono) regular/bold, retaining egui's Unicode fallbacks.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            DEFAULT_FAMILY,
            include_bytes!("../assets/fonts/NordwandMono-Regular.ttf").as_slice(),
        ),
        (
            "Bedfont-Bold",
            include_bytes!("../assets/fonts/NordwandMono-Bold.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&family)
            .unwrap()
            .insert(0, DEFAULT_FAMILY.into());
    }
    let mut bold = fonts.families[&FontFamily::Monospace].clone();
    bold.insert(0, "Bedfont-Bold".into());
    fonts.families.insert(monospace_bold(1.0).family, bold);
    fonts
}

/// Bedfont's definitions with `regular` and `bold` in front: they draw what
/// they cover, Bedfont and egui's fallbacks the rest.
pub fn with_primary(regular: Arc<FontData>, bold: Arc<FontData>) -> FontDefinitions {
    let mut fonts = definitions();
    fonts.font_data.insert("Primary Regular".into(), regular);
    fonts.font_data.insert("Primary Bold".into(), bold);
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&family)
            .unwrap()
            .insert(0, "Primary Regular".into());
    }
    fonts
        .families
        .get_mut(&monospace_bold(1.0).family)
        .unwrap()
        .insert(0, "Primary Bold".into());
    fonts
}

/// The definitions for `family` (`None` or Bedfont: the bundled fonts). `None`
/// while installed fonts are still being discovered: keep the current fonts and
/// ask again. A family that can't be found falls back to Bedfont.
pub(crate) fn resolve(
    world: &bevy::prelude::World,
    family: Option<&str>,
) -> Option<FontDefinitions> {
    let Some(family) = family.filter(|family| *family != DEFAULT_FAMILY) else {
        return Some(definitions());
    };
    #[cfg(feature = "system-fonts")]
    if let Some(installed) = world.get_resource::<InstalledFonts>() {
        if installed.loading() {
            return None;
        }
        if let Some(fonts) = installed.definitions(family) {
            return Some(fonts);
        }
    }
    #[cfg(not(feature = "system-fonts"))]
    let _ = (world, family);
    Some(definitions())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_and_bold_have_matching_monospace_metrics_and_distinct_glyphs() {
        let context = egui::Context::default();
        context.set_fonts(definitions());
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().fonts_mut(|fonts| {
                for size in [12.0, 15.5, 24.0] {
                    let regular = FontId::monospace(size);
                    let bold = monospace_bold(size);
                    assert_eq!(fonts.row_height(&regular), fonts.row_height(&bold));
                    for ch in ['M', 'i', '0', ' '] {
                        assert_eq!(
                            fonts.glyph_width(&regular, ch),
                            fonts.glyph_width(&bold, ch)
                        );
                    }
                    let regular =
                        fonts.layout_no_wrap("Bold".into(), regular, egui::Color32::WHITE);
                    let bold = fonts.layout_no_wrap("Bold".into(), bold, egui::Color32::WHITE);
                    assert_eq!(regular.size(), bold.size());
                    assert_ne!(regular.rows[0].visuals.mesh, bold.rows[0].visuals.mesh);
                }
            });
        });
        output.textures_delta.clear();
    }
}
