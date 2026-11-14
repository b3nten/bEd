//! Syntax for the documents editors show. The analysis itself is
//! `bedtime-syntax` (re-exported here); this adds what needs the workbench:
//! languages as file formats, a document's language, and running analysis
//! for each document an editor or `SyntaxViewer` shows (`DocumentSyntax`).
pub use bedtime_syntax::*;
mod documents;
pub use documents::*;

/// Each language as a text format (`FormatId("rust")`), and a detector that
/// finds them by file name as `language_name` does.
pub fn register_language_formats(app: &mut bevy::prelude::App) {
    use bedtime_workbench::{Format, FormatId, FormatsAppExt, TEXT};
    for &language in LANGUAGES {
        app.register_format(Format::new(FormatId(language)).conforms_to([TEXT]));
    }
    app.register_format_detector(|path| language_name(&path.to_string_lossy()).map(FormatId));
}

/// The language `document` is highlighted and analysed as: the one closest
/// to its format (SVG's is XML), or, for a document without a format (not
/// opened from a file), its title's. A file opened as plain text has none.
pub fn document_language(
    world: &bevy::prelude::World,
    document: bedtime_workbench::DocumentId,
    title: &str,
) -> Option<&'static str> {
    use bedtime_workbench::{FormatId, Formats, WorkbenchWorldExt};
    let Some(format) = world.workbench().document_format(document) else {
        return language_name(title);
    };
    let formats = world.get_resource::<Formats>()?;
    LANGUAGES
        .iter()
        .copied()
        .filter_map(|language| Some((formats.distance(format, FormatId(language))?, language)))
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, language)| language)
}
