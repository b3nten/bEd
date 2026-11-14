//! Colouring highlighted code: the roles a theme colours (`Palette::syntax`)
//! and which role each highlight capture (`keyword.control`, `string`, ...)
//! takes. Shared by editors, documentation and viewers so code looks the
//! same everywhere.
use super::{Theme, color};
use egui::Color32;

/// What a piece of code is, as far as colouring goes: one per colour in a
/// theme's `syntax` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyntaxRole {
    Text,
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Variable,
    Parameter,
    Property,
    Constant,
    Operator,
    Punctuation,
    Special,
}

impl SyntaxRole {
    /// The role of a highlight capture, by its first part (`keyword` in
    /// `keyword.control.rust`); `None` for captures no role covers.
    pub fn of_capture(capture: &str) -> Option<Self> {
        Some(match capture.split('.').next().unwrap_or(capture) {
            "comment" => Self::Comment,
            // `text.*`: Markdown titles, code and links.
            "string" | "character" | "text" => Self::String,
            "keyword" | "conditional" | "repeat" | "exception" | "include" | "preproc"
            | "storageclass" => Self::Keyword,
            "operator" => Self::Operator,
            "number" | "float" | "boolean" => Self::Number,
            "constant" => Self::Constant,
            "function" | "method" | "constructor" => Self::Function,
            "type" | "namespace" | "module" | "tag" | "attribute" => Self::Type,
            "variable" | "field" => Self::Variable,
            "parameter" => Self::Parameter,
            "property" => Self::Property,
            "punctuation" | "delimiter" => Self::Punctuation,
            "label" | "escape" | "special" | "embedded" | "macro" => Self::Special,
            _ => return None,
        })
    }
}

/// A theme's code colours, ready to paint. Colours affect glyphs only; font
/// and layout stay the same.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxPalette {
    pub text: Color32,
    pub comment: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub number: Color32,
    pub function: Color32,
    pub r#type: Color32,
    pub variable: Color32,
    pub parameter: Color32,
    pub property: Color32,
    pub constant: Color32,
    pub operator: Color32,
    pub punctuation: Color32,
    pub special: Color32,
}

impl Default for SyntaxPalette {
    fn default() -> Self {
        Self::from_theme(&Theme::default())
    }
}

impl SyntaxPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        let syntax = &theme.palette.syntax;
        Self {
            text: color(syntax.text),
            comment: color(syntax.comment),
            keyword: color(syntax.keyword),
            string: color(syntax.string),
            number: color(syntax.number),
            function: color(syntax.function),
            r#type: color(syntax.r#type),
            variable: color(syntax.variable),
            parameter: color(syntax.parameter),
            property: color(syntax.property),
            constant: color(syntax.constant),
            operator: color(syntax.operator),
            punctuation: color(syntax.punctuation),
            special: color(syntax.special),
        }
    }

    pub fn role(&self, role: SyntaxRole) -> Color32 {
        match role {
            SyntaxRole::Text => self.text,
            SyntaxRole::Comment => self.comment,
            SyntaxRole::Keyword => self.keyword,
            SyntaxRole::String => self.string,
            SyntaxRole::Number => self.number,
            SyntaxRole::Function => self.function,
            SyntaxRole::Type => self.r#type,
            SyntaxRole::Variable => self.variable,
            SyntaxRole::Parameter => self.parameter,
            SyntaxRole::Property => self.property,
            SyntaxRole::Constant => self.constant,
            SyntaxRole::Operator => self.operator,
            SyntaxRole::Punctuation => self.punctuation,
            SyntaxRole::Special => self.special,
        }
    }

    /// The colour of highlight `capture`, or `normal` if no role covers it.
    pub fn color(&self, capture: &str, normal: Color32) -> Color32 {
        SyntaxRole::of_capture(capture).map_or(normal, |role| self.role(role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_take_the_role_of_their_first_part() {
        assert_eq!(
            SyntaxRole::of_capture("keyword.control.rust"),
            Some(SyntaxRole::Keyword)
        );
        assert_eq!(SyntaxRole::of_capture("method"), Some(SyntaxRole::Function));
        assert_eq!(
            SyntaxRole::of_capture("text.title"),
            Some(SyntaxRole::String)
        );
        assert_eq!(SyntaxRole::of_capture("none"), None);
    }

    #[test]
    fn unknown_captures_keep_the_normal_colour() {
        let palette = SyntaxPalette::default();
        assert_eq!(palette.color("comment", Color32::RED), palette.comment);
        assert_eq!(palette.color("nothing", Color32::RED), Color32::RED);
    }
}
