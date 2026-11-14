//! What's in a font file, read once on the document's load worker: its
//! names, metrics, the characters it has, and glyph outlines on demand.
use bedtime_document::DocumentModel;
use bedtime_filesystem::{ByteModel, FileModel};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    string::StringId,
};
use std::sync::Arc;

pub const SUPPORTED_EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc"];

/// A font file's first face.
#[derive(Clone, Debug, Default)]
pub struct FontFile(pub Option<Arc<Face>>);

#[derive(Debug)]
pub struct Face {
    pub bytes: Arc<[u8]>,
    /// Faces in the file (a collection has several; the first is shown).
    pub faces: u32,
    pub family: String,
    pub style: String,
    pub version: Option<String>,
    pub designer: Option<String>,
    pub glyphs: u16,
    pub units_per_em: u16,
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: Option<f32>,
    pub x_height: Option<f32>,
    pub monospace: bool,
    /// Variation axes: tag, smallest, default, largest.
    pub axes: Vec<(String, f32, f32, f32)>,
    /// The characters it has, in order.
    pub characters: Vec<char>,
}

impl PartialEq for FontFile {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl DocumentModel for FontFile {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl ByteModel for FontFile {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Face::read(bytes).map(|face| Self(Some(Arc::new(face))))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        match &self.0 {
            Some(face) => out.write_all(&face.bytes),
            None => Ok(()),
        }
    }
}

impl FileModel for FontFile {
    const MAX_LEN: u64 = 64 * 1024 * 1024;
}

impl Face {
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let font = FontRef::from_index(bytes, 0)
            .map_err(|_| "It isn't a font this can show (TrueType or OpenType)".to_owned())?;
        let faces = match skrifa::raw::FileRef::new(bytes) {
            Ok(skrifa::raw::FileRef::Collection(collection)) => collection.len(),
            _ => 1,
        };
        let name = |id: StringId| {
            font.localized_strings(id)
                .english_or_first()
                .map(|name| name.to_string())
        };
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        let mut characters: Vec<char> = font
            .charmap()
            .mappings()
            .filter_map(|(code, _)| char::from_u32(code))
            .filter(|c| !c.is_control())
            .collect();
        characters.sort_unstable();
        characters.dedup();
        let axes = font
            .axes()
            .iter()
            .map(|axis| {
                (
                    axis.tag().to_string(),
                    axis.min_value(),
                    axis.default_value(),
                    axis.max_value(),
                )
            })
            .collect();
        Ok(Self {
            bytes: bytes.into(),
            faces,
            family: name(StringId::TYPOGRAPHIC_FAMILY_NAME)
                .or_else(|| name(StringId::FAMILY_NAME))
                .unwrap_or_else(|| "Untitled".into()),
            style: name(StringId::TYPOGRAPHIC_SUBFAMILY_NAME)
                .or_else(|| name(StringId::SUBFAMILY_NAME))
                .unwrap_or_else(|| "Regular".into()),
            version: name(StringId::VERSION_STRING),
            designer: name(StringId::DESIGNER).or_else(|| name(StringId::MANUFACTURER)),
            glyphs: metrics.glyph_count,
            units_per_em: metrics.units_per_em,
            ascent: metrics.ascent,
            descent: metrics.descent,
            cap_height: metrics.cap_height,
            x_height: metrics.x_height,
            monospace: metrics.is_monospace,
            axes,
            characters,
        })
    }

    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.bytes, 0).ok()
    }

    /// `c`'s glyph: its outline (in font units, y up), advance and name.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let font = self.font()?;
        let id = font.charmap().map(c)?;
        let mut outline = Outline::default();
        if let Some(glyph) = font.outline_glyphs().get(id) {
            glyph
                .draw(
                    DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                    &mut outline,
                )
                .ok()?;
        }
        let advance = font
            .glyph_metrics(Size::unscaled(), LocationRef::default())
            .advance_width(id)
            .unwrap_or(0.0);
        let name = font.glyph_names().get(id).map(|name| name.to_string());
        Some(Glyph {
            id: id.to_u32(),
            name,
            advance,
            outline,
        })
    }
}

pub struct Glyph {
    pub id: u32,
    pub name: Option<String>,
    pub advance: f32,
    pub outline: Outline,
}

/// A glyph's outline, as path commands.
#[derive(Default, Clone, Debug)]
pub struct Outline {
    pub commands: Vec<Command>,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Move([f32; 2]),
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

impl OutlinePen for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::Move([x, y]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::Line([x, y]));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.commands.push(Command::Quad([cx, cy], [x, y]));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands
            .push(Command::Cubic([cx0, cy0], [cx1, cy1], [x, y]));
    }
    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

impl Outline {
    /// Points on the outline, and control points off it.
    pub fn points(&self) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
        let mut on = Vec::new();
        let mut off = Vec::new();
        for command in &self.commands {
            match *command {
                Command::Move(p) | Command::Line(p) => on.push(p),
                Command::Quad(c, p) => {
                    off.push(c);
                    on.push(p);
                }
                Command::Cubic(c0, c1, p) => {
                    off.extend([c0, c1]);
                    on.push(p);
                }
                Command::Close => {}
            }
        }
        (on, off)
    }

    /// Flattened into polylines, mapped through `to` (font units to screen).
    pub fn polylines(&self, to: impl Fn([f32; 2]) -> [f32; 2]) -> Vec<Vec<[f32; 2]>> {
        let mut lines: Vec<Vec<[f32; 2]>> = Vec::new();
        let mut at = [0.0, 0.0];
        let mut start = at;
        for command in &self.commands {
            match *command {
                Command::Move(p) => {
                    lines.push(vec![to(p)]);
                    at = p;
                    start = p;
                }
                Command::Line(p) => {
                    if let Some(line) = lines.last_mut() {
                        line.push(to(p));
                    }
                    at = p;
                }
                Command::Quad(c, p) => {
                    if let Some(line) = lines.last_mut() {
                        for i in 1..=12 {
                            let t = i as f32 / 12.0;
                            let u = 1.0 - t;
                            line.push(to([
                                u * u * at[0] + 2.0 * u * t * c[0] + t * t * p[0],
                                u * u * at[1] + 2.0 * u * t * c[1] + t * t * p[1],
                            ]));
                        }
                    }
                    at = p;
                }
                Command::Cubic(c0, c1, p) => {
                    if let Some(line) = lines.last_mut() {
                        for i in 1..=16 {
                            let t = i as f32 / 16.0;
                            let u = 1.0 - t;
                            let point = |a: usize| {
                                u * u * u * at[a]
                                    + 3.0 * u * u * t * c0[a]
                                    + 3.0 * u * t * t * c1[a]
                                    + t * t * t * p[a]
                            };
                            line.push(to([point(0), point(1)]));
                        }
                    }
                    at = p;
                }
                Command::Close => {
                    if let Some(line) = lines.last_mut() {
                        line.push(to(start));
                    }
                    at = start;
                }
            }
        }
        lines
    }

    /// Filled, as straight-alpha white pixels `size` square, mapped through
    /// `to` (font units to pixels).
    pub fn rasterize(&self, size: u32, to: impl Fn([f32; 2]) -> [f32; 2]) -> Option<Vec<u8>> {
        use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};
        let mut path = PathBuilder::new();
        for command in &self.commands {
            match *command {
                Command::Move(p) => {
                    let [x, y] = to(p);
                    path.move_to(x, y);
                }
                Command::Line(p) => {
                    let [x, y] = to(p);
                    path.line_to(x, y);
                }
                Command::Quad(c, p) => {
                    let ([cx, cy], [x, y]) = (to(c), to(p));
                    path.quad_to(cx, cy, x, y);
                }
                Command::Cubic(c0, c1, p) => {
                    let ([ax, ay], [bx, by], [x, y]) = (to(c0), to(c1), to(p));
                    path.cubic_to(ax, ay, bx, by, x, y);
                }
                Command::Close => path.close(),
            }
        }
        let mut pixmap = Pixmap::new(size, size)?;
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 255, 255, 255);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        Some(pixmap.take_demultiplied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Face {
        Face::read(include_bytes!(
            "../../../../tests/fixtures/fonts/DejaVuSans.ttf"
        ))
        .unwrap()
    }

    #[test]
    fn names_metrics_and_characters_are_read() {
        let face = dejavu();
        assert_eq!(face.family, "DejaVu Sans");
        assert_eq!(face.style, "Book");
        assert_eq!(face.units_per_em, 2048);
        assert!(face.glyphs > 1000);
        assert!(face.characters.contains(&'A') && face.characters.contains(&'é'));
        assert!(face.characters.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(Face::read(b"not a font").is_err());
    }

    #[test]
    fn glyph_outlines_fill_and_flatten() {
        let face = dejavu();
        let glyph = face.glyph('O').unwrap();
        assert!(glyph.advance > 0.0);
        let (on, off) = glyph.outline.points();
        assert!(!on.is_empty() && !off.is_empty());
        // Two contours: outside and inside.
        let lines = glyph.outline.polylines(|p| p);
        assert_eq!(lines.len(), 2);
        let scale = 64.0 / 2048.0;
        let pixels = glyph
            .outline
            .rasterize(64, |[x, y]| [x * scale, 56.0 - y * scale])
            .unwrap();
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        // The middle of an O is empty.
        let middle = (32 * 64 + 22) * 4;
        assert_eq!(pixels[middle + 3], 0);
        assert!(face.glyph('\u{10FFFF}').is_none());
    }
}
