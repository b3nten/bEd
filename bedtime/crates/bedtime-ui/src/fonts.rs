//! Bundled UI typography and the matching bold monospace face.
use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

pub const DEFAULT_FAMILY: &str = "Bedfont";

/// The bold companion to `FontId::monospace`, installed by UI.
/// Hosts replacing the font definitions should retain this family and supply
/// their chosen monospace bold face with matching cell metrics.
pub fn monospace_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Monospace Bold".into()))
}

/// Bedfont (bundled Nordwand Mono) regular/bold, retaining egui's Unicode fallbacks. Applications
/// can customize these definitions before passing them to `Context::set_fonts`.
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
