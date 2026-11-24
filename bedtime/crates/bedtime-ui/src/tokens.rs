//! The sizes and tones components are made of. Each resolves against the
//! `Ui` it's used in, so a zoomed panel's components are zoomed with it.
use crate::theme::{self, Palette, Rgba};

/// The body font size where `ui` is.
fn body(ui: &egui::Ui) -> f32 {
    ui.style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |font| font.size)
}

/// Space between and around things: steps of a quarter of the font size
/// (4 points at the default size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Space {
    None,
    /// 1 step: inside a compact control.
    Xs,
    /// 2 steps: between related things.
    #[default]
    Sm,
    /// 3 steps: between controls.
    Md,
    /// 4 steps: around a group.
    Lg,
    /// 6 steps: between sections.
    Xl,
}

impl Space {
    pub const ALL: [Self; 6] = [Self::None, Self::Xs, Self::Sm, Self::Md, Self::Lg, Self::Xl];

    /// In points.
    pub fn px(self, ui: &egui::Ui) -> f32 {
        let step = (body(ui) / 4.0).round();
        step * match self {
            Self::None => 0.0,
            Self::Xs => 1.0,
            Self::Sm => 2.0,
            Self::Md => 3.0,
            Self::Lg => 4.0,
            Self::Xl => 6.0,
        }
    }
}

/// A control's size: its text, its padding and so its height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

impl Size {
    pub const ALL: [Self; 3] = [Self::Sm, Self::Md, Self::Lg];

    /// The size of a control's text.
    pub fn font(self, ui: &egui::Ui) -> f32 {
        body(ui)
            * match self {
                Self::Sm => 0.85,
                Self::Md => 1.0,
                Self::Lg => 1.1,
            }
    }

    /// The space inside a control, around its content. `Md` is the theme's
    /// own padding.
    pub fn padding(self, ui: &egui::Ui) -> egui::Vec2 {
        let padding = ui.spacing().button_padding;
        match self {
            Self::Sm => egui::vec2(padding.x * 0.75, (padding.y * 0.67).round()),
            Self::Md => padding,
            Self::Lg => egui::vec2(padding.x * 1.75, padding.y * 2.0),
        }
    }

    /// A control's height: a line of its text and its padding. `Md` is
    /// `theme::control_height`.
    pub fn height(self, ui: &egui::Ui) -> f32 {
        (self.font(ui) * theme::LINE_HEIGHT).ceil() + 2.0 * self.padding(ui).y
    }
}

/// How round corners are, from the theme's rounding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Radius {
    None,
    Sm,
    #[default]
    Md,
    Lg,
    /// A pill, or a circle.
    Full,
}

impl Radius {
    pub const ALL: [Self; 5] = [Self::None, Self::Sm, Self::Md, Self::Lg, Self::Full];

    /// In points; `Full` is more than any control is tall.
    pub fn px(self, ui: &egui::Ui) -> f32 {
        let rounding = f32::from(ui.visuals().widgets.inactive.corner_radius.nw);
        match self {
            Self::None => 0.0,
            Self::Sm => rounding / 2.0,
            Self::Md => rounding,
            Self::Lg => rounding * 2.0,
            Self::Full => 255.0,
        }
    }
}

/// What something means, as a colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
}

impl Tone {
    pub const ALL: [Self; 5] = [
        Self::Neutral,
        Self::Accent,
        Self::Success,
        Self::Warning,
        Self::Error,
    ];

    pub fn rgba(self, palette: &Palette) -> Rgba {
        match self {
            Self::Neutral => palette.text,
            Self::Accent => palette.accent,
            Self::Success => palette.success,
            Self::Warning => palette.warning,
            Self::Error => palette.error,
        }
    }
}

/// What a piece of text is, as a size and weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextRole {
    /// Small print: hints, counts.
    Caption,
    #[default]
    Body,
    /// The name of a group or a card.
    Title,
    /// The name of a page.
    Heading,
}

impl TextRole {
    pub const ALL: [Self; 4] = [Self::Caption, Self::Body, Self::Title, Self::Heading];

    pub fn size(self, ui: &egui::Ui) -> f32 {
        body(ui)
            * match self {
                Self::Caption => 0.85,
                Self::Body => 1.0,
                Self::Title => 1.1,
                Self::Heading => 1.4,
            }
    }

    /// Titles and headings are bold.
    pub fn strong(self) -> bool {
        matches!(self, Self::Title | Self::Heading)
    }
}
