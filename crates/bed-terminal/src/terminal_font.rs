//! Terminal font containers, style selection and host-atlas loading.
//! Fonts are appended before NewFrame; the host retains atlas ownership.
use dear_imgui_rs::{
    Context, FontConfig, FontId, FontLoader, FontLoaderFlags, FontSource, Ui, sys,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use swash::scale::image::Image;
use swash::{CacheKey, FontRef};

/// Validated font bytes retained for terminal shaping. The key stays stable
/// across borrows so Swash's font caches can reuse their parsed tables.
pub(crate) struct TerminalShapeFont {
    bytes: Arc<[u8]>,
    offset: u32,
    key: CacheKey,
    index: usize,
    em_scale: f32,
    pub color: bool,
}
impl TerminalShapeFont {
    fn new(bytes: &[u8], index: usize, color: bool) -> Option<Self> {
        let font = FontRef::from_index(bytes, index)?;
        let mut result = Self {
            offset: font.offset,
            key: font.key,
            bytes: Arc::from(bytes),
            index,
            em_scale: 1.0,
            color,
        };
        if !color {
            // ImGui interprets font size as ascender-to-descender height,
            // whereas Swash uses pixels per em. Shape at the equivalent size.
            let rasterizer = TerminalFontRasterizer::new(&result, 16.0).ok()?;
            unsafe {
                let face = &*rasterizer.face;
                result.em_scale = face.units_per_EM as f32
                    / (i32::from(face.ascender) - i32::from(face.descender)) as f32;
            }
            if !result.em_scale.is_finite() || result.em_scale <= 0.0 {
                return None;
            }
        }
        Some(result)
    }
    pub(crate) fn font(&self) -> FontRef<'_> {
        FontRef {
            data: &self.bytes,
            offset: self.offset,
            key: self.key,
        }
    }
    pub(crate) fn shaping_size(&self, size: f32) -> f32 {
        size * self.em_scale
    }
}

/// One UI-thread-owned FreeType face at a physical pixel size. Its immutable
/// bytes outlive the native face; glyph-slot buffers are copied before reuse.
pub(crate) struct TerminalFontRasterizer {
    library: sys::freetype::FT_Library,
    face: sys::freetype::FT_Face,
    _bytes: Arc<[u8]>,
}
impl TerminalFontRasterizer {
    pub(crate) fn new(font: &TerminalShapeFont, height: f32) -> io::Result<Self> {
        use sys::freetype as ft;
        let mut result = Self {
            library: std::ptr::null_mut(),
            face: std::ptr::null_mut(),
            _bytes: Arc::clone(&font.bytes),
        };
        // These calls borrow only validated immutable bytes retained above.
        // Drop also handles partial initialization when FreeType returns an error.
        unsafe {
            if ft::FT_Init_FreeType(&mut result.library) != 0
                || ft::FT_New_Memory_Face(
                    result.library,
                    font.bytes.as_ptr(),
                    font.bytes.len() as _,
                    font.index as _,
                    &mut result.face,
                ) != 0
            {
                return Err(io::Error::other("Cannot initialize terminal FreeType face"));
            }
            let mut request = ft::FT_Size_RequestRec {
                size_request_type: ft::FT_SIZE_REQUEST_TYPE_REAL_DIM,
                width: 0,
                height: (height * 64.0) as _,
                horiResolution: 0,
                vertResolution: 0,
            };
            if ft::FT_Request_Size(result.face, &mut request) != 0 {
                return Err(io::Error::other("Cannot size terminal FreeType face"));
            }
        }
        Ok(result)
    }
    pub(crate) fn metrics(&mut self) -> io::Result<[f32; 3]> {
        use sys::freetype as ft;
        unsafe {
            let glyph = ft::FT_Get_Char_Index(self.face, 'M' as _);
            if ft::FT_Load_Glyph(self.face, glyph, ft::FT_LOAD_NO_BITMAP) != 0 {
                return Err(io::Error::other("Cannot measure terminal font"));
            }
            let metrics = (*(*self.face).size).metrics;
            let ascent = (metrics.ascender as f32 / 64.0).ceil();
            let descent = (metrics.descender as f32 / 64.0).ceil();
            Ok([
                ((*(*self.face).glyph).advance.x as f32 / 64.0).max(1.0),
                (ascent - descent).max(1.0),
                ascent,
            ])
        }
    }
    pub(crate) fn rasterize(&mut self, glyph: u16) -> io::Result<Image> {
        use sys::freetype as ft;
        unsafe {
            // Match the UI loader: grayscale, normal hinting, outline glyphs.
            if ft::FT_Load_Glyph(self.face, glyph.into(), ft::FT_LOAD_NO_BITMAP) != 0
                || ft::FT_Render_Glyph((*self.face).glyph, ft::FT_RENDER_MODE_NORMAL) != 0
            {
                return Err(io::Error::other("Cannot rasterize terminal glyph"));
            }
            let slot = &*(*self.face).glyph;
            let bitmap = &slot.bitmap;
            let width = bitmap.width as usize;
            let height = bitmap.rows as usize;
            // Font input is validated at load time; bound native output too.
            if width > 1022 || height > 1022 {
                return Err(io::Error::other("Terminal glyph exceeds atlas page size"));
            }
            let mut image = Image::default();
            image.placement.left = slot.bitmap_left;
            image.placement.top = slot.bitmap_top;
            image.placement.width = width as u32;
            image.placement.height = height as u32;
            if width == 0 || height == 0 {
                return Ok(image);
            }
            if bitmap.buffer.is_null() {
                return Err(io::Error::other("FreeType returned an empty glyph buffer"));
            }
            let mode = bitmap.pixel_mode as u32;
            if mode != ft::FT_PIXEL_MODE_GRAY && mode != ft::FT_PIXEL_MODE_MONO {
                return Err(io::Error::other("Unsupported terminal glyph pixel format"));
            }
            image.data.reserve(width * height);
            for y in 0..height {
                let row = bitmap.buffer.offset(y as isize * bitmap.pitch as isize);
                for x in 0..width {
                    image.data.push(if mode == ft::FT_PIXEL_MODE_GRAY {
                        *row.add(x)
                    } else if *row.add(x / 8) & (0x80 >> (x % 8)) != 0 {
                        255
                    } else {
                        0
                    });
                }
            }
            Ok(image)
        }
    }
}
impl Drop for TerminalFontRasterizer {
    fn drop(&mut self) {
        unsafe {
            if !self.face.is_null() {
                sys::freetype::FT_Done_Face(self.face);
            }
            if !self.library.is_null() {
                sys::freetype::FT_Done_FreeType(self.library);
            }
        }
    }
}

// Parse font containers with ttf-parser before passing bytes to the native atlas.
// Collection indices and style flags are facts from the OpenType tables.
struct FaceData {
    bytes: Vec<u8>,
    faces: Vec<(i32, bool, bool)>,
}
impl FaceData {
    fn read(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        i32::try_from(bytes.len()).ok()?;
        let count = ttf_parser::fonts_in_collection(&bytes)
            .unwrap_or(1)
            .min(256);
        let faces: Vec<_> = (0..count)
            .filter_map(|index| {
                let face = ttf_parser::Face::parse(&bytes, index).ok()?;
                Some((index as i32, face.is_bold(), face.is_italic()))
            })
            .collect();
        if faces.is_empty() {
            return None;
        }
        Some(FaceData { bytes, faces })
    }
    fn source(&self, size: f32, config: FontConfig) -> FontSource<'_> {
        // add_font copies the validated container into atlas-owned storage.
        unsafe { FontSource::ttf_data_with_size(&self.bytes, size) }.with_config(config)
    }
    fn matching(&self, bold: bool, italic: bool) -> (i32, bool, bool) {
        self.faces
            .iter()
            .copied()
            .find(|face| (face.1, face.2) == (bold, italic))
            .unwrap_or_else(|| *self.faces.first().expect("parsed font has a face"))
    }
}

#[derive(Default)]
pub struct TerminalFonts {
    pub regular: Option<FontId>,
    pub bold: Option<FontId>,
    pub italic: Option<FontId>,
    pub bold_italic: Option<FontId>,
    pub size: f32,
    pub bad_weight: [bool; 4],
    pub bad_slant: [bool; 4],
    pub(crate) shape_fonts: Vec<TerminalShapeFont>,
    pub(crate) shape_styles: [usize; 4],
    generation: u64,
}

impl TerminalFonts {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn for_style(&self, index: usize) -> Option<FontId> {
        let styles = [self.regular, self.bold, self.italic, self.bold_italic];
        styles.get(index).copied().flatten()
    }
    pub fn shaped_metrics(&self) -> [f32; 3] {
        match self.shape_fonts.get(self.shape_styles[0]) {
            None => [8.0, 16.0, 12.0],
            Some(face) => {
                let font = face.font();
                let pixels = face.shaping_size(self.size);
                let metrics = font.metrics(&[]).scale(pixels);
                let ascent = metrics.ascent.ceil();
                let descent = metrics.descent.ceil();
                let width = font
                    .glyph_metrics(&[])
                    .scale(pixels)
                    .advance_width(font.charmap().map('M'));
                [width.ceil().max(1.0), (ascent + descent).max(1.0), ascent]
            }
        }
    }
    pub fn metrics(&self, ui: &Ui) -> [f32; 2] {
        match self.regular.and_then(|id| ui.baked_font(id, self.size)) {
            None => [8.0, 16.0],
            Some(mut baked) => {
                let height = (baked.ascent().ceil() - baked.descent().floor()).max(1.0);
                let width = baked
                    .char_advance('M')
                    .filter(|value| *value > 0.0)
                    .unwrap_or(height * 0.6);
                [width.ceil(), height]
            }
        }
    }
    pub fn reload(&mut self, context: &mut Context, root: &Path, size: f32) -> io::Result<()> {
        let override_path = std::env::var_os("TERMINAL_FONT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        self.reload_from(context, root, size, override_path.as_deref())
    }
    fn reload_from(
        &mut self,
        context: &mut Context,
        root: &Path,
        size: f32,
        override_path: Option<&Path>,
    ) -> io::Result<()> {
        self.reload_sources(context, root, size, override_path, None)
    }
    /// Face descriptors are supplied by the desktop's shared font resolver.
    pub fn reload_resolved(
        &mut self,
        context: &mut Context,
        root: &Path,
        size: f32,
        faces: &[(PathBuf, u32, bool, bool); 4],
        fallbacks: &[(PathBuf, u32, bool)],
    ) -> io::Result<()> {
        self.reload_sources(context, root, size, None, Some((faces, fallbacks)))
    }
    #[allow(clippy::type_complexity)]
    fn reload_sources(
        &mut self,
        context: &mut Context,
        root: &Path,
        size: f32,
        override_path: Option<&Path>,
        selected: Option<(&[(PathBuf, u32, bool, bool); 4], &[(PathBuf, u32, bool)])>,
    ) -> io::Result<()> {
        let pixels = if size.is_finite() {
            size.clamp(4.0, 64.0)
        } else {
            16.0
        };
        let custom = override_path.is_some();
        let normal_path = override_path
            .map(Path::to_path_buf)
            .or_else(|| selected.map(|(faces, _)| faces[0].0.clone()))
            .unwrap_or_else(|| root.join("resources/fonts/PaperMono-Regular.ttf"));
        let normal = FaceData::read(&normal_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "No valid terminal font was found",
            )
        })?;

        // Resolve all sources before touching the host atlas. A custom font has
        // four uniform slots; desktop selections use their explicit face indices.
        let mut variants = Vec::with_capacity(4);
        for style in 0..4 {
            let path = if custom {
                None
            } else if let Some((faces, _)) = selected {
                Some(faces[style].0.clone())
            } else {
                let bundled = root.join(if style & 1 == 0 {
                    "resources/fonts/PaperMono-Regular.ttf"
                } else {
                    "resources/fonts/PaperMono-Bold.ttf"
                });
                Some(if bundled.is_file() {
                    bundled
                } else {
                    system_variant(style).unwrap_or_else(|| normal_path.clone())
                })
            };
            variants.push(path.as_deref().and_then(FaceData::read));
        }
        let mut fallbacks: Vec<(FaceData, u32, bool)> = Vec::new();
        if !custom {
            if let Some((_, sources)) = selected {
                for (path, index, color) in sources {
                    if let Some(data) = FaceData::read(path)
                        .filter(|data| data.faces.iter().any(|face| face.0 == *index as i32))
                    {
                        fallbacks.push((data, *index, *color));
                    }
                }
            } else {
                for candidates in system_fallbacks() {
                    if let Some(data) = candidates
                        .iter()
                        .find_map(|name| FaceData::read(Path::new(name.0)))
                    {
                        let index = data.faces[0].0 as u32;
                        let color = candidates[0].1;
                        fallbacks.push((data, index, color));
                    }
                }
            }
        }
        let atlas = context.font_atlas();
        let loader = unsafe { sys::bed_imgui_freetype_loader() };
        if unsafe { (*atlas.raw()).FontLoader } != loader {
            atlas
                .set_font_loader(unsafe { FontLoader::from_raw(loader) })
                .map_err(io::Error::other)?;
        }
        let mut ids = [None; 4];
        let mut shape_fonts = Vec::new();
        let mut shape_styles = [0; 4];
        let mut bad_weight = [false; 4];
        let mut bad_slant = [false; 4];
        for style in 0..4 {
            let data = variants[style].as_ref().unwrap_or(&normal);
            let wanted = (style & 1 != 0, style & 2 != 0);
            let face = if custom {
                data.faces[0]
            } else {
                selected
                    .and_then(|(faces, _)| {
                        data.faces
                            .iter()
                            .copied()
                            .find(|face| face.0 == faces[style].1 as i32)
                    })
                    .unwrap_or_else(|| data.matching(wanted.0, wanted.1))
            };
            bad_weight[style] = !custom && wanted.0 != face.1;
            bad_slant[style] = !custom && wanted.1 != face.2;
            let shaping =
                TerminalShapeFont::new(&data.bytes, face.0 as usize, false).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Terminal font cannot be shaped")
                })?;
            shape_styles[style] = shape_fonts.len();
            shape_fonts.push(shaping);
            let config = FontConfig::new()
                .name(&format!("Bed terminal style {style}"))
                .font_loader_flags(if custom {
                    FontLoaderFlags::NONE
                } else {
                    FontLoaderFlags::LOAD_COLOR
                });
            let mut sources = vec![data.source(pixels, config)];
            // Explicit desktop fallbacks cover every slot. Legacy system
            // fallbacks only extend the regular slot, matching its UI behavior.
            let merge = selected.is_some() || style == 0;
            if merge {
                for (fallback, _, color) in &fallbacks {
                    let mut config = FontConfig::new().merge_mode(true);
                    if *color {
                        config = config
                            .font_loader_flags(
                                FontLoaderFlags::LOAD_COLOR | FontLoaderFlags::BITMAP,
                            )
                            .rasterizer_density(20.0 / pixels);
                    }
                    sources.push(fallback.source(pixels, config));
                }
            }
            let first = unsafe { (*atlas.raw()).Sources.Size as usize };
            ids[style] = Some(atlas.add_font(&sources));
            // The binding supports collection data but does not expose FontNo.
            // Set only the descriptors just appended, before the next frame.
            unsafe {
                let descriptors = (*atlas.raw()).Sources.Data.add(first);
                (*descriptors).FontNo = face.0 as u32;
                if merge {
                    for (offset, (_, index, _)) in fallbacks.iter().enumerate() {
                        (*descriptors.add(offset + 1)).FontNo = *index;
                    }
                }
            }
        }
        for (data, index, color) in &fallbacks {
            if let Some(font) = TerminalShapeFont::new(&data.bytes, *index as usize, *color) {
                shape_fonts.push(font);
            }
        }
        // Joining scripts need shaping fonts even when the desktop only asks
        // for symbol/CJK/emoji coverage. Avoid duplicating explicit fallbacks.
        if !custom {
            for path in joining_script_fonts() {
                if selected.is_some_and(|(_, sources)| {
                    sources
                        .iter()
                        .any(|(existing, _, _)| existing == Path::new(path))
                }) {
                    continue;
                }
                if let Some(data) = FaceData::read(Path::new(path))
                    && let Some(font) =
                        TerminalShapeFont::new(&data.bytes, data.faces[0].0 as usize, false)
                {
                    shape_fonts.push(font);
                }
            }
        }
        [self.regular, self.bold, self.italic, self.bold_italic] = ids;
        self.size = pixels;
        self.bad_weight = bad_weight;
        self.bad_slant = bad_slant;
        self.shape_fonts = shape_fonts;
        self.shape_styles = shape_styles;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }
}

fn system_variant(style: usize) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let names = [
            "DejaVuSansMono.ttf",
            "DejaVuSansMono-Bold.ttf",
            "DejaVuSansMono-Oblique.ttf",
            "DejaVuSansMono-BoldOblique.ttf",
        ];
        for directory in [
            "/usr/share/fonts/truetype/dejavu",
            "/usr/share/fonts/dejavu-sans-mono-fonts",
        ] {
            let path = Path::new(directory).join(names[style]);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let _ = style;
    None
}
fn system_fallbacks() -> Vec<Vec<(&'static str, bool)>> {
    #[cfg(target_os = "macos")]
    return vec![
        vec![
            ("/System/Library/Fonts/Apple Symbols.ttf", false),
            ("/System/Library/Fonts/Symbol.ttf", false),
        ],
        vec![
            ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", false),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", false),
        ],
        vec![("/System/Library/Fonts/Apple Color Emoji.ttc", true)],
    ];
    #[cfg(not(target_os = "macos"))]
    return vec![
        vec![(
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            false,
        )],
        vec![
            ("/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf", true),
            (
                "/usr/share/fonts/google-noto-emoji-color-fonts/NotoColorEmoji.ttf",
                true,
            ),
            ("/usr/share/fonts/noto/NotoColorEmoji.ttf", true),
        ],
    ];
}
fn joining_script_fonts() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    return &[
        "/System/Library/Fonts/GeezaPro.ttc",
        "/System/Library/Fonts/Supplemental/Devanagari Sangam MN.ttc",
    ];
    #[cfg(not(target_os = "macos"))]
    return &[
        "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, FramePrepareOptions};

    #[test]
    fn terminal_outline_pixels_and_metrics_match_the_ui_at_each_density() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload_from(&mut context, root, 20.0, None).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let atlas = context.font_atlas().raw();
        // The native UI loader is the independent pixel/size oracle. All lazy
        // glyph loads and immediate atlas reads occur before any frame locks it.
        unsafe {
            for style in 0..4 {
                let shape_font = &fonts.shape_fonts[fonts.shape_styles[style]];
                let ui_font = *(*atlas).Fonts.Data.add(style);
                for size in [16.0, 20.0, 24.0] {
                    for density in [1.0, 1.25, 2.0] {
                        let baked = sys::ImFont_GetFontBaked(ui_font, size, density);
                        assert!(!baked.is_null());
                        let mut rasterizer =
                            TerminalFontRasterizer::new(shape_font, size * density).unwrap();
                        let metrics = rasterizer.metrics().unwrap();
                        assert!(((*baked).Ascent - metrics[2] / density).abs() < 0.0001);
                        assert!(
                            ((*baked).Descent - (metrics[2] - metrics[1]) / density).abs() < 0.0001
                        );
                        for ch in ['M', 'a', 'e', 'é', 'g', '_', 'f'] {
                            let glyph = sys::ImFontBaked_FindGlyphNoFallback(baked, ch as u32);
                            assert!(!glyph.is_null());
                            let image = rasterizer
                                .rasterize(shape_font.font().charmap().map(ch))
                                .unwrap();
                            assert!(
                                ((*glyph).X0 - image.placement.left as f32 / density).abs()
                                    < 0.0001
                            );
                            assert!(
                                ((*glyph).Y0 - (metrics[2] - image.placement.top as f32) / density)
                                    .abs()
                                    < 0.0001
                            );
                            let rect = &*sys::igImFontAtlasPackGetRect(atlas, (*glyph).PackId);
                            assert_eq!(rect.w as u32, image.placement.width);
                            assert_eq!(rect.h as u32, image.placement.height);
                            let texture = &*(*atlas).TexData;
                            let bpp = texture.BytesPerPixel as usize;
                            assert!(bpp == 1 || bpp == 4);
                            let width = rect.w as usize;
                            let mut alpha = Vec::with_capacity(image.data.len());
                            for y in 0..rect.h as usize {
                                for x in 0..width {
                                    let pixel = (rect.y as usize + y) * texture.Width as usize
                                        + rect.x as usize
                                        + x;
                                    alpha.push(*texture.Pixels.add(pixel * bpp + bpp - 1));
                                }
                            }
                            assert_eq!(
                                image.data, alpha,
                                "{ch}, style {style}, size {size}, density {density}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn custom_font_reload_preserves_host_fonts_and_uses_uniform_styles() {
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let host = context.font_atlas().add_font(&[FontSource::default_font()]);
        let mut fonts = TerminalFonts::default();
        fonts
            .reload_from(
                &mut context,
                root,
                18.0,
                Some(&root.join("resources/fonts/PaperMono-Regular.ttf")),
            )
            .unwrap();
        assert_eq!(fonts.bad_weight, [false; 4]);
        assert_eq!(fonts.bad_slant, [false; 4]);
        assert_eq!(
            fonts.shape_fonts.len(),
            4,
            "an override has no implicit fallbacks"
        );
        let atlas = context.font_atlas().raw();
        unsafe {
            assert_eq!((*atlas).Fonts.Size, 5, "the existing host font is retained");
            let configurations =
                std::slice::from_raw_parts((*atlas).Sources.Data, (*atlas).Sources.Size as usize);
            for source in &configurations[1..] {
                assert!(source.FontDataOwnedByAtlas && !source.MergeMode);
                assert_eq!((source.FontNo, source.FontLoaderFlags), (0, 0));
                assert_eq!(source.SizePixels, 18.0);
            }
        }
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("custom font rendering")
            .position([20.0, 20.0], Condition::Always)
            .size([500.0, 300.0], Condition::Always)
            .build(|| {
                for style in 0..4 {
                    let id = fonts.for_style(style).unwrap();
                    assert_ne!(id, host);
                    let _font = ui.push_font_with_size(Some(id), fonts.size);
                    ui.text("Same override in each slot");
                }
                let [width, height] = fonts.metrics(ui);
                assert!(width > 0.0 && height > width);
            });
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    }

    #[test]
    fn malformed_font_containers_are_rejected_before_atlas_loading() {
        let path =
            std::env::temp_dir().join(format!("bed-invalid-terminal-font-{}", std::process::id()));
        fs::write(&path, b"OTTOinvalid font directory").unwrap();
        assert!(FaceData::read(&path).is_none());
        fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn paper_default_has_valid_metrics_and_four_style_slots() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload_from(&mut context, root, 20.0, None).unwrap();
        assert_eq!(fonts.bad_weight, [false; 4]);
        assert_eq!(fonts.bad_slant, [false, false, true, true]);
        let atlas = context.font_atlas().raw();
        let source_indices: std::collections::BTreeSet<_> = unsafe {
            std::slice::from_raw_parts((*atlas).Sources.Data, (*atlas).Sources.Size as usize)
                .iter()
                .filter(|source| !source.MergeMode)
                .map(|source| source.FontNo)
                .collect()
        };
        assert_eq!(source_indices.len(), 1);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        unsafe {
            let font = *(*atlas).Fonts.Data;
            let baked = sys::ImFont_GetFontBaked(font, 20.0, 1.0);
            let codepoint = u32::from('M');
            assert!(
                !sys::ImFontBaked_FindGlyphNoFallback(baked, codepoint).is_null(),
                "Missing {codepoint:x}"
            );
        }
    }
}
