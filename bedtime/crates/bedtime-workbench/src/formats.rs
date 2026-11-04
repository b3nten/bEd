//! Formats (`files` feature): what a file's bytes are (CSV, Rust, SVG),
//! separate from the model that holds them in memory (`TextBuffer`).
//!
//! Plugins register formats with the file names and extensions they're
//! known by and what they conform to, as macOS's Uniform Type Identifiers
//! do: CSV conforms to text, SVG to XML and image, and everything to `DATA`.
//! Viewers register for formats (`.formats([CSV])`), and a file opens with
//! the viewer whose format is closest to the file's, in steps of
//! conformance: a CSV opens in the table viewer (its exact format) over the
//! editor (text, one step up) over hex (data, two). On a tie, the format's
//! default viewer (`set_default_viewer`) wins, else the first registered.
//!
//! A file's format is its full name's (`Cargo.lock`), else its extension's,
//! else a detector's (`register_format_detector`), else, looking at its first
//! bytes, `TEXT` or `DATA`. A format that conforms to text but whose file
//! isn't text counts as `DATA`. An open records the format on the document
//! (`DocumentFormat`); "Open As" asks for another (`PathOptions::format`).
use crate::*;
use bevy::prelude::*;

/// A format's identifier: `FormatId("csv")`. Plugins sharing a format use
/// the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormatId(pub &'static str);

/// Any file: what every format conforms to, in the end.
pub const DATA: FormatId = FormatId("data");
/// Text: UTF-8 without NUL bytes.
pub const TEXT: FormatId = FormatId("text");

/// A format: what it's known by, and what it conforms to.
#[derive(Clone, Debug)]
pub struct Format {
    pub id: FormatId,
    /// Whole file names, matched without case (`cargo.lock`).
    pub names: Vec<&'static str>,
    /// Extensions, matched without case and without the dot (`csv`).
    pub extensions: Vec<&'static str>,
    /// More general formats, most fitting first. Without any, `DATA`.
    pub conforms_to: Vec<FormatId>,
}

impl Format {
    pub fn new(id: FormatId) -> Self {
        Self {
            id,
            names: Vec::new(),
            extensions: Vec::new(),
            conforms_to: Vec::new(),
        }
    }
    pub fn names(mut self, names: impl IntoIterator<Item = &'static str>) -> Self {
        self.names.extend(names);
        self
    }
    pub fn extensions(mut self, extensions: impl IntoIterator<Item = &'static str>) -> Self {
        self.extensions.extend(extensions);
        self
    }
    pub fn conforms_to(mut self, formats: impl IntoIterator<Item = FormatId>) -> Self {
        self.conforms_to.extend(formats);
        self
    }

    fn parents(&self) -> &[FormatId] {
        if self.conforms_to.is_empty() && self.id != DATA {
            &[DATA]
        } else {
            &self.conforms_to
        }
    }
}

/// A file's format, from its path, if names or extensions don't say.
pub type FormatDetector = fn(&std::path::Path) -> Option<FormatId>;

/// The registered formats.
#[derive(Resource)]
pub struct Formats {
    formats: Vec<Format>,
    detectors: Vec<FormatDetector>,
    /// Each format's default viewer, over closer ones.
    defaults: Vec<(FormatId, ViewerId)>,
}

impl Default for Formats {
    fn default() -> Self {
        Self {
            formats: vec![Format::new(DATA), Format::new(TEXT).conforms_to([DATA])],
            detectors: Vec::new(),
            defaults: Vec::new(),
        }
    }
}

impl Formats {
    pub fn get(&self, id: FormatId) -> Option<&Format> {
        self.formats.iter().find(|format| format.id == id)
    }

    /// The format of the file at `path`, whose first bytes are `head`.
    pub fn detect(&self, path: &std::path::Path, head: &[u8]) -> FormatId {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
        let named = self
            .formats
            .iter()
            .find(|format| {
                format
                    .names
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&name))
            })
            .or_else(|| {
                let extension = extension.as_deref()?;
                self.formats.iter().find(|format| {
                    format
                        .extensions
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(extension))
                })
            })
            .map(|format| format.id)
            .or_else(|| {
                self.detectors
                    .iter()
                    .find_map(|detect| detect(path))
                    // One no plugin registered says nothing about the file.
                    .filter(|&id| self.get(id).is_some())
            });
        let text = is_text(head);
        match named {
            Some(format) if text || self.distance(format, TEXT).is_none() => format,
            _ if text => TEXT,
            _ => DATA,
        }
    }

    /// How many steps of conformance from `from` to `to` (`0`: the same), if
    /// `from` conforms to it at all. An unregistered format conforms to
    /// `DATA` only.
    pub fn distance(&self, from: FormatId, to: FormatId) -> Option<u32> {
        let mut seen = vec![from];
        let mut level = vec![from];
        let mut steps = 0;
        while !level.is_empty() {
            if level.contains(&to) {
                return Some(steps);
            }
            let mut next = Vec::new();
            for id in level {
                let parents: &[FormatId] = match self.get(id) {
                    Some(format) => format.parents(),
                    None if id == DATA => &[],
                    None => &[DATA],
                };
                for &parent in parents {
                    if !seen.contains(&parent) {
                        seen.push(parent);
                        next.push(parent);
                    }
                }
            }
            level = next;
            steps += 1;
        }
        None
    }

    /// The viewer the app or a plugin made `format`'s default.
    pub fn default_viewer(&self, format: FormatId) -> Option<ViewerId> {
        self.defaults
            .iter()
            .find(|(id, _)| *id == format)
            .map(|(_, viewer)| *viewer)
    }

    fn register(&mut self, format: Format) {
        match self.formats.iter_mut().find(|known| known.id == format.id) {
            // Plugins share formats: what each says about one adds up.
            Some(known) => {
                for name in format.names {
                    if !known.names.contains(&name) {
                        known.names.push(name);
                    }
                }
                for extension in format.extensions {
                    if !known.extensions.contains(&extension) {
                        known.extensions.push(extension);
                    }
                }
                for parent in format.conforms_to {
                    if !known.conforms_to.contains(&parent) {
                        known.conforms_to.push(parent);
                    }
                }
            }
            None => self.formats.push(format),
        }
    }
}

/// UTF-8 (the sample may end mid-character) without NUL bytes.
pub fn is_text(head: &[u8]) -> bool {
    let utf8 = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    };
    utf8 && !head.contains(&0)
}

pub trait FormatsAppExt {
    /// Add a format, or more names, extensions or parents to one another
    /// plugin registered.
    fn register_format(&mut self, format: Format) -> &mut Self;
    /// Ask `detect` for a file's format when no name or extension matches.
    fn register_format_detector(&mut self, detect: FormatDetector) -> &mut Self;
    /// Open files of exactly `format` with viewer `P`, over closer viewers.
    /// The latest call wins, so an app overrides its plugins'.
    fn set_default_viewer<P: 'static>(&mut self, format: FormatId) -> &mut Self;
}

impl FormatsAppExt for App {
    fn register_format(&mut self, format: Format) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<Formats>()
            .register(format);
        self
    }

    fn register_format_detector(&mut self, detect: FormatDetector) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<Formats>()
            .detectors
            .push(detect);
        self
    }

    fn set_default_viewer<P: 'static>(&mut self, format: FormatId) -> &mut Self {
        let mut formats = self.world_mut().get_resource_or_init::<Formats>();
        formats.defaults.retain(|(id, _)| *id != format);
        formats.defaults.push((format, ViewerId::of::<P>()));
        self
    }
}

/// On a document entity: the format it was opened as.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentFormat(pub FormatId);

impl WorkbenchRef {
    /// The format `document` was opened as, if it came from a path.
    pub fn document_format(&self, document: DocumentId) -> Option<FormatId> {
        self.world
            .get::<DocumentFormat>(document.entity())
            .map(|format| format.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const CSV: FormatId = FormatId("csv");
    const XML: FormatId = FormatId("xml");
    const IMAGE: FormatId = FormatId("image");
    const SVG: FormatId = FormatId("svg");
    const TOML: FormatId = FormatId("toml");

    fn formats() -> Formats {
        let mut formats = Formats::default();
        formats.register(
            Format::new(CSV)
                .extensions(["csv", "tsv"])
                .conforms_to([TEXT]),
        );
        formats.register(Format::new(XML).extensions(["xml"]).conforms_to([TEXT]));
        formats.register(Format::new(IMAGE).extensions(["png"]));
        formats.register(
            Format::new(SVG)
                .extensions(["svg"])
                .conforms_to([IMAGE, XML]),
        );
        formats.register(Format::new(TOML).extensions(["toml"]).conforms_to([TEXT]));
        // Another plugin adds a name to the same format.
        formats.register(Format::new(TOML).names(["Cargo.lock"]));
        formats.register(Format::new(FormatId("rust")).conforms_to([TEXT]));
        formats.detectors.push(|path| {
            let extension = path.extension()?;
            match extension.to_str()? {
                "rs" => Some(FormatId("rust")),
                "zig" => Some(FormatId("zig")),
                _ => None,
            }
        });
        formats
    }

    #[test]
    fn files_are_known_by_name_then_extension_then_detectors_then_content() {
        let formats = formats();
        assert_eq!(formats.detect(Path::new("a/Cargo.lock"), b"x"), TOML);
        assert_eq!(formats.detect(Path::new("t.TSV"), b"a\tb"), CSV);
        assert_eq!(
            formats.detect(Path::new("main.rs"), b"fn"),
            FormatId("rust")
        );
        // A detector's format nobody registered: by content.
        assert_eq!(formats.detect(Path::new("main.zig"), b"const"), TEXT);
        assert_eq!(formats.detect(Path::new("notes.poop"), b"plain"), TEXT);
        assert_eq!(formats.detect(Path::new("blob"), &[0, 1, 2]), DATA);
        // Named as text but not text: just data.
        assert_eq!(formats.detect(Path::new("x.csv"), &[0, 159]), DATA);
        // A binary format stays itself.
        assert_eq!(formats.detect(Path::new("a.png"), &[137, 0]), IMAGE);
    }

    #[test]
    fn distance_counts_steps_of_conformance() {
        let formats = formats();
        assert_eq!(formats.distance(CSV, CSV), Some(0));
        assert_eq!(formats.distance(CSV, TEXT), Some(1));
        assert_eq!(formats.distance(CSV, DATA), Some(2));
        assert_eq!(formats.distance(SVG, IMAGE), Some(1));
        assert_eq!(formats.distance(SVG, TEXT), Some(2));
        assert_eq!(formats.distance(IMAGE, TEXT), None);
        // Unregistered formats are data.
        assert_eq!(formats.distance(FormatId("zig"), DATA), Some(1));
        assert_eq!(formats.distance(FormatId("zig"), TEXT), None);
    }
}
