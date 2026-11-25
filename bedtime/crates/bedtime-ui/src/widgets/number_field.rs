use super::TextField;
use crate::tokens::*;
use egui::emath::Numeric;
use std::ops::RangeInclusive;

/// A number to type.
///
/// ```ignore
/// ui.add(NumberField::new(&mut size).range(8.0..=72.0).decimals(1).suffix(" pt"));
/// ```
///
/// It shows the value (rounded to `decimals`, with its suffix); while it has
/// the keyboard it holds what's typed, and the value follows whenever that
/// reads as a number, kept inside the range. Only the text is rounded: the
/// value isn't changed by being shown.
#[must_use = "a number field does nothing until it's added to a Ui"]
pub struct NumberField<'a, N: Numeric> {
    value: &'a mut N,
    range: Option<RangeInclusive<N>>,
    decimals: usize,
    suffix: String,
    label: String,
    size: Size,
    width: Option<f32>,
}

impl<'a, N: Numeric> NumberField<'a, N> {
    /// Whole numbers for an integer type; two decimals otherwise.
    pub fn new(value: &'a mut N) -> Self {
        Self {
            value,
            range: None,
            decimals: if N::INTEGRAL { 0 } else { 2 },
            suffix: String::new(),
            label: String::new(),
            size: Size::default(),
            width: None,
        }
    }

    /// The least and most it can be.
    pub fn range(mut self, range: RangeInclusive<N>) -> Self {
        self.range = Some(range);
        self
    }
    /// How many decimal places it shows.
    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }
    /// Shown after the number (a unit).
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
    /// Its name, for accessibility and scripts.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    /// This wide, in points (room for about eight digits, unless set).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

impl<N: Numeric> egui::Widget for NumberField<'_, N> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = ui.next_auto_id().with("number");
        let before = self.value.to_f64();
        let decimals = self.decimals;
        let shown = format!("{before:.decimals$}");
        let focused = ui.memory(|memory| memory.has_focus(id));
        // Typed text is kept while it's being typed; otherwise the value shows.
        let typed = focused
            .then(|| ui.data(|data| data.get_temp::<String>(id)))
            .flatten();
        let mut text = match typed {
            Some(typed) => typed,
            None if focused => shown.clone(),
            None => format!("{shown}{}", self.suffix),
        };
        let width = self
            .width
            .unwrap_or(self.size.font(ui) * 5.5 + 2.0 * self.size.padding(ui).x);
        let mut field = TextField::new(&mut text)
            .id(id)
            .size(self.size)
            .width(width);
        if !self.label.is_empty() {
            field = field.label(self.label);
        }
        let mut response = ui.add(field);
        let mut value = before;
        if response.has_focus() {
            let number = text.trim().trim_end_matches(self.suffix.trim()).trim();
            if let Ok(parsed) = number.parse::<f64>()
                && parsed.is_finite()
                // What's shown, untouched, isn't a new value: it's rounded.
                && number != shown
            {
                value = if N::INTEGRAL { parsed.round() } else { parsed };
                if let Some(range) = &self.range {
                    value = value.clamp(range.start().to_f64(), range.end().to_f64());
                }
            }
            ui.data_mut(|data| data.insert_temp(id, text));
        } else {
            ui.data_mut(|data| data.remove::<String>(id));
        }
        // Changed means the number did, not that a key was pressed.
        let changed = value != before;
        if changed {
            *self.value = N::from_f64(value);
        }
        response.flags.set(egui::response::Flags::CHANGED, changed);
        response
    }
}
