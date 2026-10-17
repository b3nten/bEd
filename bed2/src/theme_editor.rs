//! Live, in-memory editing of the window's current theme.
use crate::tool_panels::{PanelTools, ToolRequest};
use bedtime_ui2::{UiFrame, egui, themes};
use bevy::prelude::*;

pub(crate) fn draw(mut frame: UiFrame, mut tools: ResMut<PanelTools>) {
    let entity = tools.theme;
    let (name, defaults) = if entity == tools.gallery.light {
        ("Light theme", themes::light())
    } else {
        assert_eq!(
            entity, tools.gallery.dark,
            "The demo uses light and dark themes"
        );
        ("Dark theme", themes::dark())
    };
    let mut edited = frame.theme.clone();
    let padding = frame.panel_settings.padding;
    frame.scroll("theme-editor", |mut frame| {
        frame.padded(padding, |frame| {
            frame.ui.horizontal_wrapped(|ui| {
                ui.heading(name);
                if ui.button("Reset theme").clicked() {
                    edited = defaults.clone();
                }
            });
            frame
                .ui
                .weak("Changes apply immediately and are not saved.");
            frame.ui.separator();
            frame.ui.heading("Colors");
            egui::Grid::new(("theme-colors", entity))
                .num_columns(3)
                .show(frame.ui, |ui| {
                    let p = &mut edited.palette;
                    let d = &defaults.palette;
                    for (label, value, default) in [
                        ("Background", &mut p.background, d.background),
                        ("Surface", &mut p.surface, d.surface),
                        ("Elevated", &mut p.elevated, d.elevated),
                        ("Border", &mut p.border, d.border),
                        ("Text", &mut p.text, d.text),
                        ("Muted", &mut p.muted, d.muted),
                        ("Accent", &mut p.accent, d.accent),
                        ("Selection", &mut p.selection, d.selection),
                        ("Success", &mut p.success, d.success),
                        ("Warning", &mut p.warning, d.warning),
                        ("Error", &mut p.error, d.error),
                    ] {
                        ui.label(label);
                        // Theme colors are unmultiplied sRGB, including alpha.
                        let mut rgba = value.map(|v| (v * 255.0).round() as u8);
                        ui.horizontal(|ui| {
                            if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                                *value = rgba.map(|v| v as f32 / 255.0);
                            }
                            ui.monospace(format!(
                                "#{:02X}{:02X}{:02X}{:02X}",
                                rgba[0], rgba[1], rgba[2], rgba[3]
                            ));
                        });
                        if ui.small_button("Reset").clicked() {
                            *value = default;
                        }
                        ui.end_row();
                    }
                });
            frame.ui.separator();
            frame.ui.heading("Dimensions");
            egui::Grid::new(("theme-dimensions", entity))
                .num_columns(3)
                .show(frame.ui, |ui| {
                    let m = &mut edited.metrics;
                    let d = &defaults.metrics;
                    for (label, value, default, range) in [
                        ("Font size", &mut m.font_size, d.font_size, 8.0..=72.0),
                        ("Widget rounding", &mut m.rounding, d.rounding, 0.0..=64.0),
                        (
                            "Popup rounding",
                            &mut m.popup_rounding,
                            d.popup_rounding,
                            0.0..=64.0,
                        ),
                        ("Popup gap", &mut m.popup_gap, d.popup_gap, 0.0..=128.0),
                    ] {
                        ui.label(label);
                        ui.add(egui::DragValue::new(value).speed(0.25).range(range));
                        if ui.small_button("Reset").clicked() {
                            *value = default;
                        }
                        ui.end_row();
                    }
                    for (label, value, default) in [
                        ("Padding", &mut m.padding, d.padding),
                        ("Spacing", &mut m.spacing, d.spacing),
                        ("Tooltip padding", &mut m.tooltip_padding, d.tooltip_padding),
                        ("Menu padding", &mut m.menu_padding, d.menu_padding),
                        ("Menu spacing", &mut m.menu_spacing, d.menu_spacing),
                    ] {
                        ui.label(label);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut value[0])
                                    .prefix("X ")
                                    .speed(0.25)
                                    .range(0.0..=128.0),
                            );
                            ui.add(
                                egui::DragValue::new(&mut value[1])
                                    .prefix("Y ")
                                    .speed(0.25)
                                    .range(0.0..=128.0),
                            );
                        });
                        if ui.small_button("Reset").clicked() {
                            *value = default;
                        }
                        ui.end_row();
                    }
                });
        });
    });
    if edited != *frame.theme {
        tools.requests.push(ToolRequest::Theme {
            entity,
            value: edited,
        });
    }
}
