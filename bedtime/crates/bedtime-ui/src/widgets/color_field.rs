use super::TextField;
use crate::{theme::Rgba, tokens::*};

/// A colour: a swatch that opens a picker, and the colour in hex to type.
///
/// ```ignore
/// ui.add(ColorField::new(&mut palette.accent));
/// ```
///
/// The hex is `#rrggbb`, or `#rrggbbaa` when the colour isn't opaque, as
/// theme files write colours; typing takes it with or without the `#`.
#[must_use = "a colour field does nothing until it's added to a Ui"]
pub struct ColorField<'a> {
    color: &'a mut Rgba,
    size: Size,
    width: f32,
}

impl<'a> ColorField<'a> {
    pub fn new(color: &'a mut Rgba) -> Self {
        Self {
            color,
            size: Size::default(),
            width: 96.0,
        }
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    /// How wide the hex is, in points.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque.
pub fn to_hex(color: &Rgba) -> String {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// A typed colour: six or eight hex digits, with or without `#`.
pub fn from_hex(text: &str) -> Option<Rgba> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |i: usize| {
        digits
            .get(i..i + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .map(|v| v as f32 / 255.0)
    };
    Some([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if digits.len() == 8 { channel(6)? } else { 1.0 },
    ])
}

impl egui::Widget for ColorField<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = ui.next_auto_id().with("color");
        let before = *self.color;
        let gap = Space::Xs.px(ui);
        let height = self.size.height(ui);
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        let mut response = ui
            // Room for the swatch (a square) and the hex.
            .allocate_ui_with_layout(
                egui::vec2(height + gap + self.width, height),
                layout,
                |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    // The swatch and its picker are egui's.
                    ui.spacing_mut().interact_size = egui::Vec2::splat(height);
                    let [r, g, b, a] = self
                        .color
                        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                    let mut picked = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                    let swatch = ui
                        .push_id(id, |ui| {
                            egui::widgets::color_picker::color_edit_button_srgba(
                                ui,
                                &mut picked,
                                egui::color_picker::Alpha::OnlyBlend,
                            )
                        })
                        .inner;
                    if swatch.changed() {
                        *self.color = picked.to_srgba_unmultiplied().map(|v| v as f32 / 255.0);
                    }
                    // Typed text is kept while the field has the keyboard;
                    // otherwise it shows the colour.
                    let text_id = id.with("hex");
                    let focused = ui.memory(|memory| memory.has_focus(text_id));
                    let mut text = focused
                        .then(|| ui.data(|data| data.get_temp::<String>(text_id)))
                        .flatten()
                        .unwrap_or_else(|| to_hex(self.color));
                    let field = ui.add(
                        TextField::new(&mut text)
                            .id(text_id)
                            .size(self.size)
                            .width(self.width),
                    );
                    if field.changed()
                        && let Some(typed) = from_hex(&text)
                        && typed != *self.color
                    {
                        *self.color = typed;
                    }
                    let focused = field.has_focus();
                    ui.data_mut(|data| {
                        if focused {
                            data.insert_temp(text_id, text);
                        } else {
                            data.remove::<String>(text_id);
                        }
                    });
                },
            )
            .response;
        if *self.color != before {
            response.mark_changed();
        }
        response
    }
}
