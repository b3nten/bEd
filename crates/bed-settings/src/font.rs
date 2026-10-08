//! Font selection and atlas lifecycle translated from ned util/font.{h,cpp}.
use dear_imgui_rs::{
    Context, FontAtlasLoaderError, FontConfig, FontId, FontLoader, FontLoaderFlags, FontSource, sys,
};
use std::{fs, path::Path};

pub const FONT_NAMES: [&str; 8] = [
    "SourceCodePro-Regular",
    "JetBrainsMonoNL-Regular",
    "NotoSansMono-Regular",
    "NotoSansMono-Thin",
    "NotoSansMono-Light",
    "VT323-Regular",
    "IBM_MDA",
    "VT100",
];

// These static, zero-terminated ranges are retained as legacy preload metadata,
// exactly as in upstream. ImGui 1.92 managed renderers load any glyph supported
// by the source; inclusion ranges do not exclude glyphs at runtime.
const MAIN_GLYPH_RANGES: &[sys::ImWchar] = &[
    0x0020, 0x00ff, 0x2500, 0x257f, 0x2580, 0x259f, 0x25a0, 0x25ff, 0x2600, 0x26ff, 0x2700, 0x27bf,
    0x2900, 0x297f, 0x2b00, 0x2bff, 0x3000, 0x303f, 0xe000, 0xe0ff, 0x1f300, 0x1f9ff, 0x1f600,
    0x1f64f, 0x1f680, 0x1f6ff, 0x1f900, 0x1f9ff, 0xfe00, 0xfe0f, 0x1f000, 0x1f02f, 0x1f0a0,
    0x1f0ff, 0x1f100, 0x1f64f, 0x1f650, 0x1f67f, 0x1f700, 0x1f77f, 0x1f780, 0x1f7ff, 0x1f800,
    0x1f8ff, 0x1fa00, 0x1fa6f, 0x1fa70, 0x1faff, 0x1fb00, 0x1fbff, 0,
];
const EMOJI_GLYPH_RANGES: &[sys::ImWchar] = &[
    0x2600, 0x26ff, 0x2700, 0x27bf, 0xfe00, 0xfe0f, 0x1f000, 0x1f02f, 0x1f0a0, 0x1f0ff, 0x1f100,
    0x1f64f, 0x1f650, 0x1f67f, 0x1f680, 0x1f6ff, 0x1f700, 0x1f77f, 0x1f780, 0x1f7ff, 0x1f800,
    0x1f8ff, 0x1f900, 0x1f9ff, 0x1fa00, 0x1fa6f, 0x1fa70, 0x1faff, 0x1fb00, 0x1fbff, 0,
];
const BRAILLE_GLYPH_RANGES: &[sys::ImWchar] = &[0x2800, 0x28ff, 0];
const LARGE_GLYPH_RANGES: &[sys::ImWchar] = &[0x0020, 0x00ff, 0];

fn add_font_with_legacy_ranges(
    atlas: &dear_imgui_rs::FontAtlas,
    sources: &[FontSource<'_>],
    ranges: &[&'static [sys::ImWchar]],
) -> FontId {
    assert_eq!(sources.len(), ranges.len());
    let raw = atlas.raw();
    let first = unsafe { (*raw).Sources.Size };
    let font = atlas.add_font(sources);
    // The safe addition validates descriptors, owns copied bytes, and verifies
    // unlocked main-thread access. No further mutation or frame intervenes.
    // Each range is immutable process-lifetime storage, as required by native
    // GlyphRanges (not copied by ImGui). The binding uses 32-bit ImWchar.
    unsafe {
        assert_eq!((*raw).Sources.Size, first + sources.len() as i32);
        for (index, ranges) in ranges.iter().enumerate() {
            (*(*raw).Sources.Data.add(first as usize + index)).GlyphRanges = ranges.as_ptr();
        }
    }
    font
}

/// Own an immutable font buffer validated by the same bounded-memory FreeType
/// parser used by ImGui. Native atlas addition copies it into atlas-owned memory.
struct FreeTypeFontData(Vec<u8>);
impl FreeTypeFontData {
    fn read(path: &Path) -> Option<Self> {
        Self::from_bytes(fs::read(path).ok()?)
    }
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let length = i32::try_from(bytes.len()).ok()?;
        if length <= 0 {
            return None;
        }
        let mut library = std::ptr::null_mut();
        let mut face = std::ptr::null_mut();
        // FreeType's memory face accepts an explicit buffer length, and neither
        // handle escapes this validation. Font bytes remain immutable afterwards.
        unsafe {
            if sys::freetype::FT_Init_FreeType(&mut library) != 0 {
                return None;
            }
            let error = sys::freetype::FT_New_Memory_Face(
                library,
                bytes.as_ptr(),
                length.into(),
                0,
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
        // `self` owns complete immutable FT-validated bytes through add_font;
        // Font::load verifies that the atlas uses this same FreeType parser.
        // FreeType owns atlas-wide state and cannot be a per-source override.
        unsafe { FontSource::ttf_data_with_size(&self.0, size) }.with_config(config)
    }
}

#[derive(Default)]
pub struct Font {
    pub name: String,
    pub size: f32,
    pub main: Option<FontId>,
    pub large: Option<FontId>,
}

impl Font {
    pub fn set_font(&mut self, name: &str, size: f32) {
        self.name = name.to_owned();
        self.size = size;
    }
    pub fn available_fonts(resources_root: &Path) -> Vec<&'static str> {
        FONT_NAMES
            .into_iter()
            .filter(|name| {
                resources_root
                    .join("resources/fonts")
                    .join(format!("{name}.ttf"))
                    .is_file()
            })
            .collect()
    }
    pub fn load(
        &mut self,
        context: &mut Context,
        resources_root: &Path,
        clear_atlas: bool,
    ) -> Result<(), FontAtlasLoaderError> {
        let atlas = context.font_atlas();
        if clear_atlas {
            atlas.clear();
        }
        // The callback table is native static storage. Loader selection must
        // precede any sources; when appending to a host atlas, its loader must
        // already be FreeType. A rejected change leaves existing fonts intact.
        let native_loader = unsafe { sys::bed_imgui_freetype_loader() };
        if unsafe { (*atlas.raw()).FontLoader } != native_loader {
            let loader: &'static FontLoader = unsafe { FontLoader::from_raw(native_loader) };
            atlas.set_font_loader(loader)?;
        }
        let directory = resources_root.join("resources/fonts");
        let font_data = FreeTypeFontData::read(&directory.join(format!("{}.ttf", self.name)));
        let mut config = FontConfig::new().name("Bed Main");
        if self.name.contains("vt100") || self.name.contains("VT100") {
            config = config.glyph_offset([3.0, 0.0]).pixel_snap_h(true);
        }
        self.main =
            Some(match font_data.as_ref() {
                Some(data) => {
                    let braille = FreeTypeFontData::read(&directory.join("DejaVuSans.ttf"));
                    let emoji_path = ["seguiemj.ttf", "Emoji.ttf"]
                        .into_iter()
                        .map(|name| directory.join(name))
                        .find(|path| path.is_file());
                    let emoji = emoji_path.as_deref().and_then(FreeTypeFontData::read);
                    let mut sources = vec![data.source(self.size, config)];
                    let mut ranges = vec![MAIN_GLYPH_RANGES];
                    if let Some(data) = &braille {
                        sources.push(data.source(
                            self.size,
                            FontConfig::new().name("Bed Braille").merge_mode(true),
                        ));
                        ranges.push(BRAILLE_GLYPH_RANGES);
                    }
                    if let Some(data) = &emoji {
                        sources.push(
                            data.source(
                                self.size * 0.7,
                                FontConfig::new()
                                    .name("Bed Emoji")
                                    .merge_mode(true)
                                    .font_loader_flags(FontLoaderFlags::LOAD_COLOR),
                            ),
                        );
                        ranges.push(EMOJI_GLYPH_RANGES);
                    }
                    add_font_with_legacy_ranges(atlas, &sources, &ranges)
                }
                None => atlas
                    .add_font(&[FontSource::default_font()
                        .with_config(FontConfig::new().pixel_snap_h(true))]),
            });
        self.large =
            Some(match font_data.as_ref() {
                Some(data) => add_font_with_legacy_ranges(
                    atlas,
                    &[data.source(52.0, FontConfig::new().name("Bed Large"))],
                    &[LARGE_GLYPH_RANGES],
                ),
                None => atlas
                    .add_font(&[FontSource::default_font()
                        .with_config(FontConfig::new().pixel_snap_h(true))]),
            });
        context.style_mut().set_font_size_base(self.size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, FramePrepareOptions};

    #[test]
    fn bounded_freetype_validation_rejects_invalid_and_accepts_all_bundled_fonts() {
        assert!(FreeTypeFontData::from_bytes(Vec::new()).is_none());
        assert!(FreeTypeFontData::from_bytes(vec![0; 200]).is_none());
        assert!(FreeTypeFontData::from_bytes(b"not a font".to_vec()).is_none());
        let directory =
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join("resources/fonts");
        let mut count = 0;
        for path in fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
        {
            if path.extension().is_some_and(|extension| extension == "ttf") {
                assert!(
                    FreeTypeFontData::read(&path).is_some(),
                    "{}",
                    path.display()
                );
                count += 1;
            }
        }
        assert!(count >= 8);
    }

    #[test]
    fn native_atlas_merges_braille_and_color_emoji_with_exact_source_sizes() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let mut font = Font::default();
        font.set_font("NotoSansMono-Regular", 16.0);
        font.load(
            &mut context,
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
            true,
        )
        .unwrap();
        let atlas = context.font_atlas().raw();
        // The context owns these sources throughout the assertions; no frame or
        // atlas mutation runs concurrently, and the original input buffers died.
        unsafe {
            assert_eq!((*atlas).Fonts.Size, 2);
            assert_eq!((*atlas).Sources.Size, 4);
            let sources = std::slice::from_raw_parts((*atlas).Sources.Data, 4);
            assert_eq!(
                sources
                    .iter()
                    .map(|source| source.SizePixels)
                    .collect::<Vec<_>>(),
                [16.0, 16.0, 16.0 * 0.7, 52.0]
            );
            assert_eq!(
                sources
                    .iter()
                    .map(|source| source.MergeMode)
                    .collect::<Vec<_>>(),
                [false, true, true, false]
            );
            assert_eq!((*atlas).FontLoader, sys::bed_imgui_freetype_loader());
            assert!(sources.iter().all(|source| source.FontLoader.is_null()));
            assert!(
                sources
                    .iter()
                    .all(|source| source.GlyphExcludeRanges.is_null())
            );
            for (source, ranges) in sources.iter().zip([
                MAIN_GLYPH_RANGES,
                BRAILLE_GLYPH_RANGES,
                EMOJI_GLYPH_RANGES,
                LARGE_GLYPH_RANGES,
            ]) {
                assert_eq!(
                    std::slice::from_raw_parts(source.GlyphRanges, ranges.len()),
                    ranges
                );
            }
            assert!(sources.iter().all(|source| source.FontDataOwnedByAtlas));
            assert_eq!(sources[2].FontLoaderFlags, FontLoaderFlags::LOAD_COLOR.0);
        }
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        unsafe {
            let main = *(*atlas).Fonts.Data;
            let large = *(*atlas).Fonts.Data.add(1);
            let baked = sys::ImFont_GetFontBaked(main, 16.0, 1.0);
            let ascii = sys::ImFontBaked_FindGlyphNoFallback(baked, 'A' as u32);
            assert!(!ascii.is_null());
            assert_eq!((*ascii).Colored(), 0);
            let braille = sys::ImFontBaked_FindGlyphNoFallback(baked, 0x28ff);
            assert!(!braille.is_null());
            // Original merge priority gives DejaVu's overlapping smiley glyph
            // priority, while the SVG emoji source supplies a colored rocket.
            let smiley = sys::ImFontBaked_FindGlyphNoFallback(baked, 0x1f600);
            assert!(!smiley.is_null());
            assert_eq!((*smiley).Colored(), 0);
            let emoji = sys::ImFontBaked_FindGlyphNoFallback(baked, 0x1f680);
            assert!(!emoji.is_null());
            assert_eq!((*emoji).Colored(), 1);
            assert!((*atlas).TexPixelsUseColors);
            // Legacy inclusion ranges do not restrict dynamic glyph access.
            assert!(!sys::ImFontBaked_FindGlyphNoFallback(baked, 0x0416).is_null());
            let large_baked = sys::ImFont_GetFontBaked(large, 52.0, 1.0);
            assert!(!sys::ImFontBaked_FindGlyphNoFallback(large_baked, 'A' as u32).is_null());
            assert!(sys::ImFontBaked_FindGlyphNoFallback(large_baked, 0x28ff).is_null());
            assert!(sys::ImFontBaked_FindGlyphNoFallback(large_baked, 0x1f600).is_null());
        }
        // The native dynamic-glyph probes above change texture state; refresh
        // the legacy capability before using the binding's frame lifecycle.
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let _font = ui.push_font(font.main.unwrap());
        ui.window("native fonts")
            .position([10.0, 10.0], Condition::Always)
            .size([400.0, 200.0], Condition::Always)
            .build(|| ui.text("A ⣿ 😀 🚀 Ж"));
        drop(_font);
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    }

    #[test]
    fn append_and_clear_font_lifecycle_preserves_host_fonts_then_rebuilds() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        let mut font = Font::default();
        font.set_font("SourceCodePro-Regular", 18.0);
        font.load(
            &mut context,
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
            false,
        )
        .unwrap();
        assert_eq!(unsafe { (*context.font_atlas().raw()).Fonts.Size }, 3);
        assert_ne!(font.main.unwrap(), host);
        font.load(
            &mut context,
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
            true,
        )
        .unwrap();
        assert_eq!(unsafe { (*context.font_atlas().raw()).Fonts.Size }, 2);
        assert!(font.main.is_some());
        assert!(font.large.is_some());
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
    }

    #[test]
    fn incompatible_host_loader_rejects_append_without_changing_existing_fonts() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .set_font_loader(FontLoader::stb_truetype())
            .unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        let mut font = Font::default();
        font.set_font("SourceCodePro-Regular", 18.0);
        assert_eq!(
            font.load(
                &mut context,
                Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
                false
            ),
            Err(FontAtlasLoaderError::SourcesAlreadyAdded { source_count: 1 })
        );
        assert!(font.main.is_none());
        assert!(font.large.is_none());
        let atlas = context.font_atlas().raw();
        unsafe {
            assert_eq!((*atlas).Fonts.Size, 1);
            assert_eq!((*atlas).Sources.Size, 1);
            assert_ne!((*atlas).FontLoader, sys::bed_imgui_freetype_loader());
        }
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let guard = ui.push_font(host);
        ui.window("host fonts")
            .position([10.0, 10.0], Condition::Always)
            .size([400.0, 200.0], Condition::Always)
            .build(|| ui.text("host still renders"));
        drop(guard);
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    }

    #[test]
    fn missing_main_font_uses_original_default_fallbacks_without_merging() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let mut font = Font::default();
        font.set_font("missing-font", 18.0);
        font.load(
            &mut context,
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
            true,
        )
        .unwrap();
        let atlas = context.font_atlas().raw();
        unsafe {
            assert_eq!((*atlas).Fonts.Size, 2);
            assert_eq!((*atlas).Sources.Size, 2);
            let sources = std::slice::from_raw_parts((*atlas).Sources.Data, 2);
            for source in sources {
                assert_eq!(source.SizePixels, 13.0);
                assert!(source.PixelSnapH);
                assert!(!source.MergeMode);
                assert_ne!(source.Flags & sys::ImFontFlags_ImplicitRefSize, 0);
            }
        }
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
    }
}
