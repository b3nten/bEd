use crate::{Theme, fonts, theme, tokens::*};

/// Text, by what it is (`TextRole`) and what it means (`Tone`).
///
/// ```ignore
/// ui.add(Text::title("Request"));
/// ui.add(Text::new(error).tone(Tone::Error));
/// ui.add(Text::caption("3 files").muted());
/// ```
#[must_use = "text does nothing until it's added to a Ui"]
pub struct Text {
    text: String,
    role: TextRole,
    tone: Tone,
    muted: bool,
    strong: Option<bool>,
    truncate: bool,
}

impl Text {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: TextRole::Body,
            tone: Tone::Neutral,
            muted: false,
            strong: None,
            truncate: false,
        }
    }
    pub fn caption(text: impl Into<String>) -> Self {
        Self::new(text).role(TextRole::Caption)
    }
    pub fn title(text: impl Into<String>) -> Self {
        Self::new(text).role(TextRole::Title)
    }
    pub fn heading(text: impl Into<String>) -> Self {
        Self::new(text).role(TextRole::Heading)
    }

    pub fn role(mut self, role: TextRole) -> Self {
        self.role = role;
        self
    }
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
    /// Less important than what's around it.
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }
    /// Bold, whatever its role.
    pub fn strong(mut self) -> Self {
        self.strong = Some(true);
        self
    }
    /// One line, cut short with an ellipsis, rather than wrapped.
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
}

impl egui::Widget for Text {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let theme = Theme::of(ui);
        let size = self.role.size(ui);
        let bold = fonts::monospace_bold(size);
        // A context without the UI's fonts (a test's) has no bold.
        let has_bold = ui.fonts(|fonts| fonts.definitions().families.contains_key(&bold.family));
        let font = if self.strong.unwrap_or(self.role.strong()) && has_bold {
            bold
        } else {
            egui::FontId::proportional(size)
        };
        let color = if self.muted {
            theme.palette.muted
        } else {
            self.tone.rgba(&theme.palette)
        };
        let text = egui::RichText::new(self.text)
            .font(font)
            .color(theme::color(color));
        let label = egui::Label::new(text);
        ui.add(if self.truncate {
            label.truncate()
        } else {
            label
        })
    }
}
