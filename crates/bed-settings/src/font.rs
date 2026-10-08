//! Shared installed-family resolution and desktop atlas lifecycle.
use dear_imgui_rs::{
    Context, FontAtlasLoaderError, FontConfig, FontId, FontLoader, FontLoaderFlags, FontSource, sys,
};
use fontdb::{Database, Family, Query, Source, Style, Weight};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const DEFAULT_FONT: &str = "Paper Mono";
/// Path, collection face index, actual bold and italic flags.
pub type Face = (PathBuf, u32, bool, bool);
/// Path, collection face index, color emoji flag.
pub type Fallback = (PathBuf, u32, bool);
#[derive(Clone, Debug)]
pub struct ResolvedFonts {
    pub family: String,
    pub faces: [Face; 4],
    pub fallbacks: Vec<Fallback>,
}

fn catalog() -> &'static Database {
    static CATALOG: OnceLock<Database> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    })
}
fn query_face(db: &Database, name: &str, bold: bool, italic: bool) -> Option<Face> {
    let id = db.query(&Query {
        families: &[Family::Name(name)],
        weight: if bold { Weight::BOLD } else { Weight::NORMAL },
        style: if italic { Style::Italic } else { Style::Normal },
        ..Query::default()
    })?;
    let face = db.face(id)?;
    let Source::File(path) = &face.source else {
        return None;
    };
    Some((
        path.clone(),
        face.index,
        face.weight.0 >= 600,
        face.style != Style::Normal,
    ))
}

impl ResolvedFonts {
    pub fn resolve(name: &str, root: &Path) -> Self {
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
            (
                root.join(format!(
                    "resources/fonts/PaperMono-{}.ttf",
                    if bold { "Bold" } else { "Regular" }
                )),
                0,
                bold,
                false,
            )
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
            if let Some((path, index, _, _)) = families
                .iter()
                .find_map(|name| query_face(db, name, false, false))
                && !faces.iter().any(|face| face.0 == path && face.1 == index)
                && !fallbacks
                    .iter()
                    .any(|face: &Fallback| face.0 == path && face.1 == index)
            {
                fallbacks.push((path, index, emoji));
            }
        }
        Self {
            family: family.into(),
            faces,
            fallbacks,
        }
    }
}

/// Bound native parsing to owned, immutable bytes before adding an atlas source.
struct FontData(Vec<u8>);
impl FontData {
    fn read(path: &Path, index: u32) -> Option<Self> {
        Self::validate(fs::read(path).ok()?, index)
    }
    fn validate(bytes: Vec<u8>, index: u32) -> Option<Self> {
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
        Some(Self(bytes))
    }
    fn source(&self, size: f32, config: FontConfig) -> FontSource<'_> {
        unsafe { FontSource::ttf_data_with_size(&self.0, size) }.with_config(config)
    }
}

#[derive(Default)]
pub struct Font {
    pub name: String,
    pub size: f32,
    pub main: Option<FontId>,
    pub large: Option<FontId>,
    pub resolved: Option<ResolvedFonts>,
    pub warning: Option<String>,
}
impl Font {
    pub fn set_font(&mut self, name: &str, size: f32) {
        self.name = name.into();
        self.size = size;
    }
    pub fn available_fonts(_: &Path) -> Vec<String> {
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
    pub fn load(
        &mut self,
        context: &mut Context,
        root: &Path,
        clear: bool,
    ) -> Result<(), FontAtlasLoaderError> {
        let atlas = context.font_atlas();
        if clear {
            atlas.clear();
        }
        let native_loader = unsafe { sys::bed_imgui_freetype_loader() };
        if unsafe { (*atlas.raw()).FontLoader } != native_loader {
            let loader: &'static FontLoader = unsafe { FontLoader::from_raw(native_loader) };
            atlas.set_font_loader(loader)?;
        }
        let mut resolved = ResolvedFonts::resolve(&self.name, root);
        let mut regular = FontData::read(&resolved.faces[0].0, resolved.faces[0].1);
        if regular.is_none() && resolved.family != DEFAULT_FONT {
            resolved = ResolvedFonts::resolve(DEFAULT_FONT, root);
            regular = FontData::read(&resolved.faces[0].0, resolved.faces[0].1);
        }
        self.warning = (resolved.family != self.name)
            .then(|| format!("Font '{}' is unavailable; using Paper Mono.", self.name));
        if let Some(regular) = regular {
            let fallback_data: Vec<_> = resolved
                .fallbacks
                .iter()
                .filter_map(|(path, index, emoji)| {
                    FontData::read(path, *index).map(|data| (data, *index, *emoji))
                })
                .collect();
            let mut sources = vec![regular.source(self.size, FontConfig::new().name("Bed Main"))];
            for (data, _, emoji) in &fallback_data {
                let mut config = FontConfig::new()
                    .merge_mode(true)
                    .name("Bed System Fallback");
                if *emoji {
                    config = config
                        .font_loader_flags(FontLoaderFlags::LOAD_COLOR | FontLoaderFlags::BITMAP)
                        .rasterizer_density(20.0 / self.size);
                }
                sources.push(data.source(self.size, config));
            }
            let first = unsafe { (*atlas.raw()).Sources.Size as usize };
            self.main = Some(atlas.add_font(&sources));
            // The binding has no FontNo setter. Set collection indices on the
            // just-added descriptors before any font bake or frame begins.
            unsafe {
                (*(*atlas.raw()).Sources.Data.add(first)).FontNo = resolved.faces[0].1;
                for (i, (_, index, _)) in fallback_data.iter().enumerate() {
                    (*(*atlas.raw()).Sources.Data.add(first + i + 1)).FontNo = *index;
                }
            }
            let first = unsafe { (*atlas.raw()).Sources.Size as usize };
            self.large =
                Some(atlas.add_font(&[regular.source(52.0, FontConfig::new().name("Bed Large"))]));
            unsafe {
                (*(*atlas.raw()).Sources.Data.add(first)).FontNo = resolved.faces[0].1;
            }
        } else {
            self.main = Some(atlas.add_font(&[FontSource::default_font_with_size(self.size)]));
            self.large = Some(atlas.add_font(&[FontSource::default_font_with_size(52.0)]));
            self.warning = Some("Packaged Paper Mono is missing or invalid".into());
        }
        self.resolved = Some(resolved);
        context.style_mut().set_font_size_base(self.size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }
    #[test]
    fn paper_is_valid_and_unknown_family_resolves_without_installed_fonts() {
        assert!(FontData::validate(vec![0; 100], 0).is_none());
        let resolved = ResolvedFonts::resolve("not a real installed family", root());
        assert_eq!(resolved.family, DEFAULT_FONT);
        for (path, index, _, _) in resolved.faces {
            assert!(FontData::read(&path, index).is_some());
        }
        assert_eq!(Font::available_fonts(root())[0], DEFAULT_FONT);
    }
    #[test]
    fn native_faces_and_host_atlas_survive_reload() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        let mut font = Font::default();
        font.set_font(DEFAULT_FONT, 20.0);
        font.load(&mut context, root(), false).unwrap();
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(font.main.unwrap().reference_size(), Some(20.0));
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
    }
    #[test]
    fn installed_monospace_family_resolves_collection_faces() {
        let Some(family) = Font::available_fonts(root())
            .into_iter()
            .find(|f| f != DEFAULT_FONT)
        else {
            return;
        };
        let resolved = ResolvedFonts::resolve(&family, root());
        assert_eq!(resolved.family, family);
        for (path, index, _, _) in &resolved.faces {
            assert!(FontData::read(path, *index).is_some());
        }
    }
}
