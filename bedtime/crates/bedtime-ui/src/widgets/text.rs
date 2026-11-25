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
    color: Option<egui::Color32>,
    italic: bool,
    strikethrough: bool,
    wrap: bool,
    clickable: bool,
    selectable: bool,
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
            color: None,
            italic: false,
            strikethrough: false,
            wrap: false,
            clickable: false,
            selectable: false,
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
    /// A colour of the caller's own (a graph's, a chart's), where no `Tone`
    /// says it.
    pub fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }
    /// Struck through: gone, hidden or done.
    pub fn strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }
    /// Wrapped at the width it's in, even in a row (where text otherwise
    /// runs on).
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }
    /// Its text can be selected and copied, even where labels otherwise
    /// can't be: something worth taking away, like a signature.
    pub fn selectable(mut self) -> Self {
        self.selectable = true;
        self
    }
    /// It can be clicked (`response.clicked()`): a name that goes somewhere.
    pub fn clickable(mut self) -> Self {
        self.clickable = true;
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
        let mut text = egui::RichText::new(self.text)
            .font(font)
            .color(self.color.unwrap_or(theme::color(color)));
        if self.italic {
            text = text.italics();
        }
        if self.strikethrough {
            text = text.strikethrough();
        }
        let mut label = egui::Label::new(text);
        if self.truncate {
            label = label.truncate();
        } else if self.wrap {
            label = label.wrap();
        }
        if self.selectable {
            label = label.selectable(true);
        }
        if self.clickable {
            label = label.sense(egui::Sense::click());
        }
        ui.add(label)
    }
}
