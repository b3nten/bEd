//! Font slots and metrics translated from pinned imgui-terminal/terminal.cpp.
//! See LICENSES/terminal-adapter-BSL-1.1.txt and NOTICE for attribution.
//! Fonts are appended before NewFrame; the host retains atlas ownership.
use dear_imgui_rs::{
    Context, FontConfig, FontId, FontLoader, FontLoaderFlags, FontSource, Ui, sys,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

struct FaceData {
    bytes: Vec<u8>,
    faces: Vec<(i32, bool, bool)>,
}
impl FaceData {
    fn read(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let length = i32::try_from(bytes.len()).ok()?;
        if length == 0 {
            return None;
        }
        let mut library = std::ptr::null_mut();
        let mut face = std::ptr::null_mut();
        let mut faces = Vec::new();
        // FT owns no memory here: immutable bytes remain alive until every
        // temporary face is destroyed. Each collection index is validated by
        // the same bounded parser used by the native ImGui loader.
        unsafe {
            if sys::freetype::FT_Init_FreeType(&mut library) != 0 {
                return None;
            }
            if sys::freetype::FT_New_Memory_Face(
                library,
                bytes.as_ptr(),
                length.into(),
                0,
                &mut face,
            ) == 0
            {
                let count = (*face).num_faces.clamp(1, 256);
                for index in 0..count as usize {
                    if index != 0
                        && sys::freetype::FT_New_Memory_Face(
                            library,
                            bytes.as_ptr(),
                            length.into(),
                            index as _,
                            &mut face,
                        ) != 0
                    {
                        continue;
                    }
                    let flags = (*face).style_flags;
                    faces.push((
                        index as i32,
                        flags & sys::freetype::FT_STYLE_FLAG_BOLD != 0,
                        flags & sys::freetype::FT_STYLE_FLAG_ITALIC != 0,
                    ));
                    sys::freetype::FT_Done_Face(face);
                }
            }
            sys::freetype::FT_Done_FreeType(library);
        }
        (!faces.is_empty()).then_some(Self { bytes, faces })
    }
    fn source(&self, size: f32, config: FontConfig) -> FontSource<'_> {
        // Input has passed bounded FreeType validation; add_font copies it into
        // native atlas storage before this owner can be dropped.
        unsafe { FontSource::ttf_data_with_size(&self.bytes, size) }.with_config(config)
    }
    fn matching(&self, bold: bool, italic: bool) -> (i32, bool, bool) {
        self.faces
            .iter()
            .copied()
            .find(|&(_, b, i)| b == bold && i == italic)
            .unwrap_or(self.faces[0])
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
}

impl TerminalFonts {
    pub fn for_style(&self, index: usize) -> Option<FontId> {
        [self.regular, self.bold, self.italic, self.bold_italic]
            .get(index)
            .copied()
            .flatten()
    }
    pub fn reload(
        &mut self,
        context: &mut Context,
        resources_root: &Path,
        size: f32,
    ) -> io::Result<()> {
        let override_path = std::env::var_os("TERMINAL_FONT")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        self.reload_from(context, resources_root, size, override_path.as_deref())
    }
    fn reload_from(
        &mut self,
        context: &mut Context,
        resources_root: &Path,
        size: f32,
        override_path: Option<&Path>,
    ) -> io::Result<()> {
        self.size = if size.is_finite() {
            size.clamp(6.0, 72.0)
        } else {
            16.0
        };
        let atlas = context.font_atlas();
        let native_loader = unsafe { sys::bed_imgui_freetype_loader() };
        if unsafe { (*atlas.raw()).FontLoader } != native_loader {
            let loader: &'static FontLoader = unsafe { FontLoader::from_raw(native_loader) };
            atlas.set_font_loader(loader).map_err(io::Error::other)?;
        }
        let regular_path = override_path
            .map(PathBuf::from)
            .or_else(|| regular_path(resources_root));
        let regular = regular_path
            .as_deref()
            .and_then(FaceData::read)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "No valid terminal font was found",
                )
            })?;
        let symbols = if override_path.is_none() {
            symbol_paths(resources_root)
                .into_iter()
                .find_map(|path| FaceData::read(&path))
        } else {
            None
        };
        let cjk = if override_path.is_none() {
            cjk_paths()
                .into_iter()
                .find_map(|path| FaceData::read(&path))
        } else {
            None
        };
        let emoji = if override_path.is_none() {
            emoji_paths(resources_root)
                .into_iter()
                .find_map(|path| FaceData::read(&path))
        } else {
            None
        };
        for (index, (want_bold, want_italic)) in
            [(false, false), (true, false), (false, true), (true, true)]
                .into_iter()
                .enumerate()
        {
            let variant = if override_path.is_none() {
                variant_path(index).and_then(|path| FaceData::read(&path))
            } else {
                None
            };
            let data = variant.as_ref().unwrap_or(&regular);
            let (face_index, actual_bold, actual_italic) = if override_path.is_some() {
                data.faces[0]
            } else {
                data.matching(want_bold, want_italic)
            };
            self.bad_weight[index] = override_path.is_none() && want_bold != actual_bold;
            self.bad_slant[index] = override_path.is_none() && want_italic != actual_italic;
            let config = FontConfig::new()
                .name(
                    [
                        "Bed Terminal",
                        "Bed Terminal Bold",
                        "Bed Terminal Italic",
                        "Bed Terminal Bold Italic",
                    ][index],
                )
                .font_loader_flags(if override_path.is_some() {
                    FontLoaderFlags::NONE
                } else {
                    FontLoaderFlags::LOAD_COLOR
                });
            let mut sources = vec![data.source(self.size, config)];
            if index == 0 {
                for fallback in [symbols.as_ref(), cjk.as_ref()].into_iter().flatten() {
                    sources.push(fallback.source(self.size, FontConfig::new().merge_mode(true)));
                }
                if let Some(emoji) = &emoji {
                    let config = FontConfig::new()
                        .merge_mode(true)
                        .font_loader_flags(FontLoaderFlags::LOAD_COLOR | FontLoaderFlags::BITMAP);
                    #[cfg(not(windows))]
                    let config = config.rasterizer_density(20.0 / self.size);
                    sources.push(emoji.source(self.size, config));
                }
            }
            let first = unsafe { (*atlas.raw()).Sources.Size };
            let font = atlas.add_font(&sources);
            // dear-imgui-rs does not expose FontNo. The validated collection
            // face is selected on the just-appended, atlas-owned descriptor
            // before any bake/frame. No raw input/font pointers escape.
            unsafe {
                (*(*atlas.raw()).Sources.Data.add(first as usize)).FontNo = face_index as u32;
            }
            match index {
                0 => self.regular = Some(font),
                1 => self.bold = Some(font),
                2 => self.italic = Some(font),
                _ => self.bold_italic = Some(font),
            }
        }
        Ok(())
    }
    pub fn metrics(&self, ui: &Ui) -> [f32; 2] {
        let Some(mut font) = self.regular.and_then(|font| ui.baked_font(font, self.size)) else {
            return [8.0, 16.0];
        };
        let height = font.ascent().ceil() + (-font.descent()).ceil();
        let advance = font.char_advance('M').unwrap_or(height * 0.6);
        [
            if advance > 0.0 {
                advance.ceil()
            } else {
                (height * 0.6).ceil()
            },
            height.max(1.0),
        ]
    }
}

fn first_file(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}
fn regular_path(root: &Path) -> Option<PathBuf> {
    let paths = vec![
        #[cfg(target_os = "macos")]
        PathBuf::from("/System/Library/Fonts/Menlo.ttc"),
        #[cfg(windows)]
        windows_fonts().join("consola.ttf"),
        #[cfg(all(unix, not(target_os = "macos")))]
        PathBuf::from("/usr/share/fonts/truetype/msttcorefonts/Menlo.ttf"),
        #[cfg(all(unix, not(target_os = "macos")))]
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
        #[cfg(all(unix, not(target_os = "macos")))]
        PathBuf::from("/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf"),
        root.join("resources/fonts/NotoSansMono-Regular.ttf"),
    ];
    first_file(paths)
}
fn variant_path(index: usize) -> Option<PathBuf> {
    #[cfg(windows)]
    return [
        "consola.ttf",
        "consolab.ttf",
        "consolai.ttf",
        "consolaz.ttf",
    ]
    .get(index)
    .map(|name| windows_fonts().join(name));
    #[cfg(all(unix, not(target_os = "macos")))]
    return [
        "DejaVuSansMono.ttf",
        "DejaVuSansMono-Bold.ttf",
        "DejaVuSansMono-Oblique.ttf",
        "DejaVuSansMono-BoldOblique.ttf",
    ]
    .get(index)
    .and_then(|name| {
        first_file([
            PathBuf::from("/usr/share/fonts/truetype/dejavu").join(name),
            PathBuf::from("/usr/share/fonts/dejavu-sans-mono-fonts").join(name),
        ])
    });
    #[cfg(target_os = "macos")]
    {
        let _ = index;
        None
    }
}
fn symbol_paths(root: &Path) -> Vec<PathBuf> {
    vec![
        #[cfg(target_os = "macos")]
        PathBuf::from("/System/Library/Fonts/Apple Symbols.ttf"),
        #[cfg(target_os = "macos")]
        PathBuf::from("/System/Library/Fonts/Symbol.ttf"),
        root.join("resources/fonts/DejaVuSans.ttf"),
    ]
}
fn cjk_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return vec![
        PathBuf::from("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
        PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
    ];
    #[cfg(windows)]
    return vec![
        windows_fonts().join("msgothic.ttc"),
        windows_fonts().join("msyh.ttc"),
    ];
    #[cfg(all(unix, not(target_os = "macos")))]
    return vec![PathBuf::from(
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    )];
}
fn emoji_paths(root: &Path) -> Vec<PathBuf> {
    vec![
        #[cfg(target_os = "macos")]
        PathBuf::from("/System/Library/Fonts/Apple Color Emoji.ttc"),
        #[cfg(windows)]
        windows_fonts().join("seguiemj.ttf"),
        root.join("resources/fonts/Emoji.ttf"),
    ]
}
#[cfg(windows)]
fn windows_fonts() -> PathBuf {
    PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, FramePrepareOptions};

    #[test]
    fn override_has_four_uniform_slots_and_appends_to_host_atlas() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let atlas = context.font_atlas();
        let data = FaceData::read(&root.join("resources/fonts/SourceCodePro-Regular.ttf")).unwrap();
        let host = atlas.add_font(&[data.source(16.0, FontConfig::new())]);
        atlas.add_font(&[FontSource::default_font()]);
        let mut fonts = TerminalFonts::default();
        fonts
            .reload_from(
                &mut context,
                root,
                16.0,
                Some(&root.join("resources/fonts/SourceCodePro-Regular.ttf")),
            )
            .unwrap();
        let raw = context.font_atlas().raw();
        unsafe {
            assert_eq!((*raw).Fonts.Size, 6);
            let sources =
                std::slice::from_raw_parts((*raw).Sources.Data, (*raw).Sources.Size as usize);
            for source in &sources[sources.len() - 4..] {
                assert_eq!(source.SizePixels, 16.0);
                assert_eq!(source.FontNo, 0);
                assert_eq!(source.FontLoaderFlags, 0);
                assert!(!source.MergeMode);
                assert!(source.FontDataOwnedByAtlas);
            }
        }
        assert_eq!(fonts.bad_weight, [false; 4]);
        assert_eq!(fonts.bad_slant, [false; 4]);
        for slot in 0..4 {
            assert_ne!(fonts.for_style(slot).unwrap(), host);
        }
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("terminal fonts")
            .position([10.0, 10.0], Condition::Always)
            .size([400.0, 200.0], Condition::Always)
            .build(|| {
                let metrics = fonts.metrics(ui);
                assert!(metrics[0] > 0.0 && metrics[1] > metrics[0]);
                for slot in 0..4 {
                    let _font = ui.push_font_with_size(fonts.for_style(slot), fonts.size);
                    ui.text("ASCII and ⣿");
                }
            });
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn menlo_collection_selects_distinct_native_faces_and_base_fallbacks() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload_from(&mut context, root, 20.0, None).unwrap();
        assert_eq!(fonts.bad_weight, [false; 4]);
        assert_eq!(fonts.bad_slant, [false; 4]);
        let atlas = context.font_atlas().raw();
        let source_indices: std::collections::BTreeSet<_> = unsafe {
            std::slice::from_raw_parts((*atlas).Sources.Data, (*atlas).Sources.Size as usize)
                .iter()
                .filter(|source| !source.MergeMode)
                .map(|source| source.FontNo)
                .collect()
        };
        assert_eq!(source_indices.len(), 4);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        unsafe {
            let font = *(*atlas).Fonts.Data;
            let baked = sys::ImFont_GetFontBaked(font, 20.0, 1.0);
            for codepoint in [u32::from('M'), 0x2800, 0x4e2d, 0x1f680] {
                assert!(
                    !sys::ImFontBaked_FindGlyphNoFallback(baked, codepoint).is_null(),
                    "Missing {codepoint:x}"
                );
            }
        }
    }
}
