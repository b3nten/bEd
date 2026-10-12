//! Installed-family resolution and owned ImGui atlas lifecycle.
//! Paper Mono is embedded so presentation never requires an application resource directory.
pub use crate::appearance::UiFontOptions;
use dear_imgui_rs::{Context, FontConfig, FontId, FontLoader, FontLoaderFlags, FontSource, sys};
use fontdb::{Database, Family, Query, Source, Style, Weight};
use std::{
    collections::BTreeSet,
    fs, io,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

pub const DEFAULT_FONT: &str = "Paper Mono";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontSourceData {
    File(PathBuf),
    Bytes(Arc<[u8]>),
}
impl FontSourceData {
    pub fn read(&self) -> io::Result<Arc<[u8]>> {
        match self {
            Self::File(path) => fs::read(path).map(Arc::from),
            Self::Bytes(bytes) => Ok(Arc::clone(bytes)),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFace {
    pub source: FontSourceData,
    pub index: u32,
    pub bold: bool,
    pub italic: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFallback {
    pub source: FontSourceData,
    pub index: u32,
    pub emoji: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFonts {
    pub family: String,
    pub faces: [ResolvedFace; 4],
    pub fallbacks: Vec<ResolvedFallback>,
}
fn catalog() -> &'static Database {
    static CATALOG: OnceLock<Database> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    })
}
pub fn available_fonts() -> Vec<String> {
    let families: BTreeSet<_> = catalog()
        .faces()
        .filter(|f| f.monospaced)
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .filter(|name| name != DEFAULT_FONT)
        .collect();
    std::iter::once(DEFAULT_FONT.to_string())
        .chain(families)
        .collect()
}
fn query_face(db: &Database, name: &str, bold: bool, italic: bool) -> Option<ResolvedFace> {
    let id = db.query(&Query {
        families: &[Family::Name(name)],
        weight: if bold { Weight::BOLD } else { Weight::NORMAL },
        style: if italic { Style::Italic } else { Style::Normal },
        ..Query::default()
    })?;
    let face = db.face(id)?;
    let source = match &face.source {
        Source::File(path) => FontSourceData::File(path.clone()),
        Source::Binary(bytes) => FontSourceData::Bytes(Arc::from(bytes.as_ref().as_ref())),
    };
    Some(ResolvedFace {
        source,
        index: face.index,
        bold: face.weight.0 >= 600,
        italic: face.style != Style::Normal,
    })
}
fn packaged_face(bold: bool) -> ResolvedFace {
    static REGULAR: OnceLock<Arc<[u8]>> = OnceLock::new();
    static BOLD: OnceLock<Arc<[u8]>> = OnceLock::new();
    let bytes = if bold {
        BOLD.get_or_init(|| {
            Arc::from(include_bytes!("../assets/fonts/PaperMono-Bold.ttf").as_slice())
        })
    } else {
        REGULAR.get_or_init(|| {
            Arc::from(include_bytes!("../assets/fonts/PaperMono-Regular.ttf").as_slice())
        })
    };
    ResolvedFace {
        source: FontSourceData::Bytes(Arc::clone(bytes)),
        index: 0,
        bold,
        italic: false,
    }
}
impl ResolvedFonts {
    pub fn resolve(name: &str) -> Self {
        let db = catalog();
        let installed = name != DEFAULT_FONT
            && db.faces().any(|face| {
                face.monospaced && face.families.iter().any(|(family, _)| family == name)
            });
        let family = if installed { name } else { DEFAULT_FONT };
        let faces = std::array::from_fn(|index| {
            let bold = index & 1 != 0;
            let italic = index & 2 != 0;
            if installed && let Some(face) = query_face(db, family, bold, italic) {
                return face;
            }
            packaged_face(bold)
        });
        let mut fallbacks = Vec::new();
        for (families, emoji) in [
            (
                &[
                    "Apple Symbols",
                    "DejaVu Sans",
                    "Noto Sans Symbols 2",
                    "Noto Sans Symbols",
                ][..],
                false,
            ),
            (
                &[
                    "Hiragino Sans",
                    "Hiragino Kaku Gothic ProN",
                    "Noto Sans CJK SC",
                    "Noto Sans CJK JP",
                    "WenQuanYi Zen Hei",
                ][..],
                false,
            ),
            (
                &["Apple SD Gothic Neo", "Noto Sans CJK KR", "NanumGothic"][..],
                false,
            ),
            (
                &["Apple Color Emoji", "Noto Color Emoji", "Segoe UI Emoji"][..],
                true,
            ),
        ] {
            if let Some(face) = families
                .iter()
                .find_map(|name| query_face(db, name, false, false))
                && !faces
                    .iter()
                    .any(|other| other.source == face.source && other.index == face.index)
                && !fallbacks.iter().any(|other: &ResolvedFallback| {
                    other.source == face.source && other.index == face.index
                })
            {
                fallbacks.push(ResolvedFallback {
                    source: face.source,
                    index: face.index,
                    emoji,
                });
            }
        }
        Self {
            family: family.into(),
            faces,
            fallbacks,
        }
    }
}

/// Validate uncertain native font sources before passing them to the atlas.
fn validated_bytes(source: &FontSourceData, index: u32) -> Option<Arc<[u8]>> {
    let bytes = source.read().ok()?;
    let length = i32::try_from(bytes.len()).ok()?;
    if length == 0 {
        return None;
    }
    let mut library = std::ptr::null_mut();
    let mut face = std::ptr::null_mut();
    unsafe {
        if sys::freetype::FT_Init_FreeType(&mut library) != 0 {
            return None;
        }
        let error = sys::freetype::FT_New_Memory_Face(
            library,
            bytes.as_ptr(),
            length.into(),
            index.into(),
            &mut face,
        );
        if error == 0 {
            sys::freetype::FT_Done_Face(face);
        }
        sys::freetype::FT_Done_FreeType(library);
        if error != 0 {
            return None;
        }
    }
    Some(bytes)
}
fn source(bytes: &[u8], size: f32, config: FontConfig) -> FontSource<'_> {
    unsafe { FontSource::ttf_data_with_size(bytes, size) }.with_config(config)
}

/// Main-thread atlas handles. Resolved sources are immutable bytes shared with
/// other font users; only this owner replaces the presentation atlas.
#[derive(Default)]
pub struct UiFonts {
    pub main: Option<FontId>,
    pub large: Option<FontId>,
    pub resolved: Option<ResolvedFonts>,
    pub warning: Option<String>,
    pub generation: u64,
    applied: Option<(UiFontOptions, bool)>,
}
impl UiFonts {
    pub fn available_fonts() -> Vec<String> {
        available_fonts()
    }
    pub fn size(&self) -> f32 {
        self.applied
            .as_ref()
            .map_or(20.0, |(options, _)| options.size)
    }
    /// Returns whether the atlas changed. Embedded contexts append fonts and
    /// preserve the host's default font size and pre-existing sources.
    pub fn apply(
        &mut self,
        context: &mut Context,
        options: &UiFontOptions,
        embedded: bool,
    ) -> io::Result<bool> {
        if !options.size.is_finite() || !(1.0..=256.0).contains(&options.size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "font size must be finite and between 1 and 256",
            ));
        }
        if self
            .applied
            .as_ref()
            .is_some_and(|(previous, mode)| previous == options && *mode == embedded)
        {
            return Ok(false);
        }
        let mut resolved = ResolvedFonts::resolve(&options.family);
        if validated_bytes(&resolved.faces[0].source, resolved.faces[0].index).is_none() {
            resolved = ResolvedFonts::resolve(DEFAULT_FONT);
        }
        // Freeze the validated sources for both atlas rasterization and shaping.
        for (index, face) in resolved.faces.iter_mut().enumerate() {
            if let Some(bytes) = validated_bytes(&face.source, face.index) {
                face.source = FontSourceData::Bytes(bytes);
            } else {
                *face = packaged_face(index & 1 != 0);
            }
        }
        resolved.fallbacks.retain_mut(|face| {
            if let Some(bytes) = validated_bytes(&face.source, face.index) {
                face.source = FontSourceData::Bytes(bytes);
                true
            } else {
                false
            }
        });
        let regular = resolved.faces[0].source.read()?;
        let fallback_data: Vec<_> = resolved
            .fallbacks
            .iter()
            .map(|face| {
                (
                    face.source.read().expect("resolved font bytes are owned"),
                    face.index,
                    face.emoji,
                )
            })
            .collect();
        let atlas = context.font_atlas();
        let native_loader = unsafe { sys::bed_imgui_freetype_loader() };
        // Installing the loader can fail with an existing foreign atlas; do it
        // before destructive mutation so the host's fonts remain valid.
        if unsafe { (*atlas.raw()).FontLoader } != native_loader {
            let loader: &'static FontLoader = unsafe { FontLoader::from_raw(native_loader) };
            atlas.set_font_loader(loader).map_err(io::Error::other)?;
        }
        if !embedded {
            atlas.clear();
        }
        let mut sources = vec![source(
            &regular,
            options.size,
            FontConfig::new().name("Desktop Main"),
        )];
        for (data, _, emoji) in &fallback_data {
            let mut config = FontConfig::new()
                .merge_mode(true)
                .name("Desktop System Fallback");
            if *emoji {
                config = config
                    .font_loader_flags(FontLoaderFlags::LOAD_COLOR | FontLoaderFlags::BITMAP)
                    .rasterizer_density(20.0 / options.size);
            }
            sources.push(source(data, options.size, config));
        }
        let first = unsafe { (*atlas.raw()).Sources.Size as usize };
        self.main = Some(atlas.add_font(&sources));
        // The binding has no FontNo setter. Set collection indices before bake.
        unsafe {
            (*(*atlas.raw()).Sources.Data.add(first)).FontNo = resolved.faces[0].index;
            for (i, (_, index, _)) in fallback_data.iter().enumerate() {
                (*(*atlas.raw()).Sources.Data.add(first + i + 1)).FontNo = *index;
            }
        }
        let first = unsafe { (*atlas.raw()).Sources.Size as usize };
        self.large = Some(atlas.add_font(&[source(
            &regular,
            52.0,
            FontConfig::new().name("Desktop Large"),
        )]));
        unsafe {
            (*(*atlas.raw()).Sources.Data.add(first)).FontNo = resolved.faces[0].index;
        }
        if !embedded {
            context.style_mut().set_font_size_base(options.size);
        }
        self.warning = (resolved.family != options.family).then(|| {
            format!(
                "Font '{}' is unavailable; using Paper Mono.",
                options.family
            )
        });
        self.resolved = Some(resolved);
        self.applied = Some((options.clone(), embedded));
        self.generation = self
            .generation
            .checked_add(1)
            .expect("font atlas generation overflow");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packaged_faces_work_without_application_assets() {
        assert!(
            validated_bytes(&FontSourceData::Bytes(Arc::from([0u8; 100].as_slice())), 0).is_none()
        );
        let resolved = ResolvedFonts::resolve("unknown installed family");
        assert_eq!(resolved.family, DEFAULT_FONT);
        for face in resolved.faces {
            assert!(validated_bytes(&face.source, face.index).is_some());
        }
        assert_eq!(available_fonts()[0], DEFAULT_FONT);
    }
    #[test]
    fn embedded_fonts_preserve_host_sources_and_unchanged_options_skip_reload() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        context.style_mut().set_font_size_base(27.0);
        let mut fonts = UiFonts::default();
        let options = UiFontOptions::default();
        assert!(fonts.apply(&mut context, &options, true).unwrap());
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        assert_eq!(fonts.main.unwrap().reference_size(), Some(20.0));
        assert_eq!(fonts.generation, 1);
        let first = fonts.main;
        assert!(!fonts.apply(&mut context, &options, true).unwrap());
        assert_eq!(fonts.main, first);
        assert_eq!(fonts.generation, 1);
        assert!(
            fonts
                .apply(
                    &mut context,
                    &UiFontOptions {
                        size: 31.0,
                        ..options
                    },
                    true
                )
                .unwrap()
        );
        assert_eq!(fonts.generation, 2);
        assert_eq!(host.reference_size(), Some(13.0));
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
    }
    #[test]
    fn invalid_options_and_foreign_loader_preserve_host_atlas() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .set_font_loader(FontLoader::stb_truetype())
            .unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        let mut fonts = UiFonts::default();
        assert!(
            fonts
                .apply(&mut context, &UiFontOptions::default(), true)
                .is_err()
        );
        assert!(
            fonts
                .apply(
                    &mut context,
                    &UiFontOptions {
                        size: f32::NAN,
                        ..UiFontOptions::default()
                    },
                    false
                )
                .is_err()
        );
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(fonts.generation, 0);
        assert_eq!(unsafe { (*context.font_atlas().raw()).Fonts.Size }, 1);
    }
}
