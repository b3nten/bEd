//! Worker-owned FreeType faces. No native handles or borrowed font data cross threads.
use crate::webfont::{self, MAX_FONT_BYTES};
use dear_imgui_sys::freetype as ft;
use harfrust::{Buffer, Direction, Feature, Font, ShapeOptions, ShaperFont, Tag};
use std::{collections::HashMap, ffi::CStr, ptr, slice, sync::Arc};

const MAX_GLYPH_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct FontInfo {
    pub family: String,
    pub style: String,
    pub face_count: u32,
    pub face_index: u32,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_height: i16,
    pub monospace: bool,
    pub names: Vec<(String, String)>,
    /// Every glyph, including unencoded alternates and ligatures.
    pub glyphs: Vec<Option<u32>>,
    pub mappings: Vec<(u32, u32)>,
}

#[derive(Clone, Debug)]
pub(crate) struct GlyphDetails {
    pub id: u32,
    pub name: String,
    pub codepoint: Option<u32>,
    pub advance: ft::FT_Pos,
    pub bearing: [ft::FT_Pos; 2],
    pub size: [ft::FT_Pos; 2],
}

pub(crate) struct GlyphBitmap {
    pub size: [u32; 2],
    pub left: i32,
    pub top: i32,
    pub rgba: Vec<u8>,
    pub color: bool,
}

#[derive(Debug)]
pub(crate) struct ShapedGlyph {
    pub id: u32,
    #[cfg(test)]
    pub cluster: u32,
    pub position: [f32; 2],
    pub advance: [f32; 2],
}

pub(crate) struct ShapedLine {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
    pub rtl: bool,
}

struct Face {
    library: ft::FT_Library,
    raw: ft::FT_Face,
    // FT_New_Memory_Face borrows this immutable buffer until FT_Done_Face.
    _bytes: Arc<[u8]>,
}
impl Face {
    fn new(bytes: Arc<[u8]>, index: u32) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
            return Err("Font must contain between 1 byte and 64 MiB".into());
        }
        let length = ft::FT_Long::try_from(bytes.len()).map_err(|_| "Font is too large")?;
        // Validate the length against the font library's native C integer type.
        #[allow(clippy::unnecessary_fallible_conversions)]
        let face_index =
            ft::FT_Long::try_from(index).map_err(|_| "Collection face index is too large")?;
        let mut library = ptr::null_mut();
        let mut raw = ptr::null_mut();
        // SAFETY: output pointers are valid, the input buffer is length-bounded
        // and retained by the returned owner. Both handles stay on this thread.
        unsafe {
            check(ft::FT_Init_FreeType(&mut library), "initialize FreeType")?;
            if let Err(error) = check(
                ft::FT_New_Memory_Face(library, bytes.as_ptr(), length, face_index, &mut raw),
                "read this font face",
            ) {
                ft::FT_Done_FreeType(library);
                return Err(error);
            }
        }
        Ok(Self {
            library,
            raw,
            _bytes: bytes,
        })
    }
}
impl Drop for Face {
    fn drop(&mut self) {
        // SAFETY: unique handles are destroyed before their borrowed bytes.
        unsafe {
            ft::FT_Done_Face(self.raw);
            ft::FT_Done_FreeType(self.library);
        }
    }
}

fn check(error: ft::FT_Error, action: &str) -> Result<(), String> {
    if error == 0 {
        Ok(())
    } else {
        Err(format!("Could not {action} (FreeType error {error})"))
    }
}

pub(crate) struct FontEngine {
    face: Face,
    font: Font,
    pub info: Arc<FontInfo>,
    bitmaps: HashMap<(u32, u32), Arc<GlyphBitmap>>,
    cache_bytes: usize,
}
impl FontEngine {
    pub fn new(bytes: Arc<[u8]>, index: u32) -> Result<Self, String> {
        // Decode once on the worker so FreeType and HarfRust use identical
        // SFNT bytes. The document retains the original compressed source.
        let bytes = webfont::decode(bytes)?;
        let first = Face::new(Arc::clone(&bytes), 0)?;
        // A restored collection may have been replaced by a smaller one.
        let index = index.min(unsafe { (*first.raw).num_faces.saturating_sub(1).max(0) as u32 });
        let face = if index == 0 {
            first
        } else {
            Face::new(Arc::clone(&bytes), index)?
        };
        // This small outer Arc adapts the shared document buffer to read-fonts'
        // owned source interface without making a second copy of the font.
        let source: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes);
        let font = Font::new(source, index).ok_or("HarfRust could not read this OpenType font")?;
        if font.units_per_em() == 0 {
            return Err("Font has an invalid units-per-em value".into());
        }
        let info = Arc::new(inspect(&face)?);
        Ok(Self {
            face,
            font,
            info,
            bitmaps: HashMap::new(),
            cache_bytes: 0,
        })
    }

    pub fn shape(
        &self,
        text: &str,
        pixels: f32,
        ligatures: bool,
        kerning: bool,
        direction: usize,
    ) -> Result<ShapedLine, String> {
        let mut buffer = Buffer::new();
        buffer.push_str(text);
        if direction == 1 {
            buffer.set_direction(Direction::LeftToRight);
        }
        if direction == 2 {
            buffer.set_direction(Direction::RightToLeft);
        }
        buffer.guess_segment_properties();
        let rtl = buffer.direction() == Direction::RightToLeft;
        let features = [
            Feature::new(Tag::new(b"liga"), u32::from(ligatures), ..),
            Feature::new(Tag::new(b"clig"), u32::from(ligatures), ..),
            Feature::new(Tag::new(b"kern"), u32::from(kerning), ..),
        ];
        let shaper = ShaperFont::new(&self.font).with_scale((pixels * 64.0).round() as i32);
        harfrust::shape(
            &shaper,
            &mut buffer,
            ShapeOptions::new().features(&features),
        )
        .map_err(|error| format!("Could not shape sample: {error:?}"))?;
        let mut pen = [0.0, 0.0];
        let glyphs = buffer
            .glyph_infos()
            .iter()
            .zip(buffer.glyph_positions())
            .map(|(info, pos)| {
                let glyph = ShapedGlyph {
                    id: info.glyph_id,
                    #[cfg(test)]
                    cluster: info.cluster,
                    position: [
                        pen[0] + pos.x_offset as f32 / 64.0,
                        pen[1] - pos.y_offset as f32 / 64.0,
                    ],
                    advance: [pos.x_advance as f32 / 64.0, -pos.y_advance as f32 / 64.0],
                };
                pen[0] += glyph.advance[0];
                pen[1] += glyph.advance[1];
                glyph
            })
            .collect();
        Ok(ShapedLine {
            glyphs,
            width: pen[0],
            rtl,
        })
    }

    pub fn details(&self, id: u32) -> Result<GlyphDetails, String> {
        // SAFETY: this worker exclusively owns the face and its glyph slot.
        unsafe {
            check(
                ft::FT_Load_Glyph(
                    self.face.raw,
                    id,
                    ft::FT_LOAD_NO_SCALE | ft::FT_LOAD_NO_HINTING,
                ),
                "read glyph metrics",
            )?;
            let metrics = &(*(*self.face.raw).glyph).metrics;
            let mut name = [0u8; 256];
            let named = ft::FT_Get_Glyph_Name(
                self.face.raw,
                id,
                name.as_mut_ptr().cast(),
                name.len() as u32,
            ) == 0;
            Ok(GlyphDetails {
                id,
                name: if named {
                    CStr::from_ptr(name.as_ptr().cast())
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                },
                codepoint: self.info.glyphs.get(id as usize).copied().flatten(),
                advance: metrics.horiAdvance,
                bearing: [metrics.horiBearingX, metrics.horiBearingY],
                size: [metrics.width, metrics.height],
            })
        }
    }

    pub fn bitmap(&mut self, id: u32, pixels: u32) -> Result<Arc<GlyphBitmap>, String> {
        let pixels = pixels.clamp(1, 1024);
        if let Some(bitmap) = self.bitmaps.get(&(id, pixels)) {
            return Ok(Arc::clone(bitmap));
        }
        let bitmap = Arc::new(rasterize(&self.face, id, pixels)?);
        if self.cache_bytes + bitmap.rgba.len() > MAX_CACHE_BYTES {
            self.bitmaps.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += bitmap.rgba.len();
        self.bitmaps.insert((id, pixels), Arc::clone(&bitmap));
        Ok(bitmap)
    }
}

fn inspect(face: &Face) -> Result<FontInfo, String> {
    // SAFETY: all fields and strings belong to this live FreeType face. Copied
    // metadata contains no native pointers and may be sent to the UI thread.
    unsafe {
        let raw = &*face.raw;
        let glyph_count = usize::try_from(raw.num_glyphs)
            .ok()
            .filter(|n| (1..=1_000_000).contains(n))
            .ok_or("Font has an invalid glyph count")?;
        let string = |value: *const ft::FT_String| {
            if value.is_null() {
                String::new()
            } else {
                CStr::from_ptr(value).to_string_lossy().into_owned()
            }
        };
        let mut info = FontInfo {
            family: string(raw.family_name),
            style: string(raw.style_name),
            face_count: u32::try_from(raw.num_faces)
                .ok()
                .filter(|n| *n > 0)
                .ok_or("Invalid font collection")?,
            face_index: raw.face_index as u32 & 0xffff,
            units_per_em: raw.units_per_EM,
            ascender: raw.ascender,
            descender: raw.descender,
            line_height: raw.height,
            monospace: raw.face_flags & ft::FT_FACE_FLAG_FIXED_WIDTH != 0,
            names: Vec::new(),
            glyphs: vec![None; glyph_count],
            mappings: Vec::new(),
        };
        if ft::FT_Select_Charmap(face.raw, ft::FT_ENCODING_UNICODE) == 0 {
            let mut id = 0;
            let mut codepoint = ft::FT_Get_First_Char(face.raw, &mut id);
            while id != 0 && codepoint <= 0x10ffff {
                if char::from_u32(codepoint as u32).is_some() && (id as usize) < glyph_count {
                    info.glyphs[id as usize].get_or_insert(codepoint as u32);
                    info.mappings.push((codepoint as u32, id));
                }
                let next = ft::FT_Get_Next_Char(face.raw, codepoint, &mut id);
                if next <= codepoint {
                    break;
                }
                codepoint = next;
            }
        }
        for (name_id, label) in [
            (5, "Version"),
            (0, "Copyright"),
            (8, "Manufacturer"),
            (9, "Designer"),
            (13, "License"),
            (14, "License URL"),
        ] {
            let mut best = None;
            for index in 0..ft::FT_Get_Sfnt_Name_Count(face.raw).min(4096) {
                let mut record = std::mem::zeroed::<ft::FT_SfntName>();
                if ft::FT_Get_Sfnt_Name(face.raw, index, &mut record) != 0
                    || record.name_id != name_id
                    || record.string.is_null()
                    || record.string_len > 65536
                {
                    continue;
                }
                let bytes = slice::from_raw_parts(record.string, record.string_len as usize);
                let value = match record.platform_id {
                    0 | 3 => String::from_utf16_lossy(
                        &bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|b| u16::from_be_bytes([b[0], b[1]]))
                            .collect::<Vec<_>>(),
                    ),
                    1 if bytes.is_ascii() => String::from_utf8_lossy(bytes).into_owned(),
                    _ => continue,
                };
                if !value.is_empty() {
                    best = Some(value);
                }
                if best.is_some() && record.platform_id == 3 && record.language_id == 0x409 {
                    break;
                }
            }
            if let Some(value) = best {
                info.names.push((label.into(), value));
            }
        }
        Ok(info)
    }
}

fn rasterize(face: &Face, id: u32, pixels: u32) -> Result<GlyphBitmap, String> {
    // SAFETY: the face and its slot are worker-local; the bitmap is copied
    // before any further glyph load. FreeType validates the input font buffer.
    unsafe {
        let mut ratio = 1.0;
        if ft::FT_Set_Pixel_Sizes(face.raw, 0, pixels) != 0 {
            let raw = &*face.raw;
            if raw.num_fixed_sizes <= 0
                || raw.num_fixed_sizes > 1024
                || raw.available_sizes.is_null()
            {
                return Err("Font cannot be rendered at this size".into());
            }
            let sizes = slice::from_raw_parts(raw.available_sizes, raw.num_fixed_sizes as usize);
            let (index, size) = sizes
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| (s.y_ppem / 64 - pixels as ft::FT_Pos).abs())
                .unwrap();
            check(
                ft::FT_Select_Size(face.raw, index as i32),
                "select bitmap font size",
            )?;
            ratio = pixels as f32 / (size.y_ppem as f32 / 64.0).max(1.0);
        }
        check(
            ft::FT_Load_Glyph(
                face.raw,
                id,
                ft::FT_LOAD_RENDER | ft::FT_LOAD_NO_HINTING | ft::FT_LOAD_COLOR,
            ),
            "render glyph",
        )?;
        let slot = &*(*face.raw).glyph;
        let bitmap = &slot.bitmap;
        let width = u32::try_from(bitmap.width).map_err(|_| "Invalid glyph width")?;
        let height = u32::try_from(bitmap.rows).map_err(|_| "Invalid glyph height")?;
        if width == 0 || height == 0 {
            return Ok(GlyphBitmap {
                size: [0, 0],
                left: 0,
                top: 0,
                rgba: Vec::new(),
                color: false,
            });
        }
        if u64::from(width) * u64::from(height) > (MAX_GLYPH_BYTES / 4) as u64 {
            return Err("Glyph bitmap exceeds 4 MiB".into());
        }
        let color = u32::from(bitmap.pixel_mode as u8) == ft::FT_PIXEL_MODE_BGRA;
        let row_bytes = match u32::from(bitmap.pixel_mode as u8) {
            ft::FT_PIXEL_MODE_BGRA => width as usize * 4,
            ft::FT_PIXEL_MODE_GRAY => width as usize,
            ft::FT_PIXEL_MODE_MONO => (width as usize).div_ceil(8),
            _ => return Err("Unsupported glyph bitmap pixel format".into()),
        };
        if bitmap.buffer.is_null() || bitmap.pitch.unsigned_abs() < row_bytes as u32 {
            return Err("Invalid glyph bitmap stride".into());
        }
        let size = [
            (width as f32 * ratio).round() as u32,
            (height as f32 * ratio).round() as u32,
        ];
        if u64::from(size[0]) * u64::from(size[1]) > (MAX_GLYPH_BYTES / 4) as u64 {
            return Err("Scaled glyph exceeds 4 MiB".into());
        }
        let mut rgba = vec![0; size[0] as usize * size[1] as usize * 4];
        for y in 0..size[1] {
            let source_y = ((y as f32 / ratio) as u32).min(height - 1);
            let row = slice::from_raw_parts(
                bitmap
                    .buffer
                    .offset(source_y as isize * bitmap.pitch as isize),
                row_bytes,
            );
            for x in 0..size[0] {
                let source_x = ((x as f32 / ratio) as u32).min(width - 1) as usize;
                let pixel = &mut rgba[(y as usize * size[0] as usize + x as usize) * 4..][..4];
                if color {
                    let source = &row[source_x * 4..][..4];
                    let alpha = u32::from(source[3]);
                    for c in 0..3 {
                        pixel[c] = (u32::from(source[2 - c]) * 255)
                            .checked_div(alpha)
                            .unwrap_or(0)
                            .min(255) as u8;
                    }
                    pixel[3] = source[3];
                } else {
                    pixel[..3].fill(255);
                    pixel[3] = if u32::from(bitmap.pixel_mode as u8) == ft::FT_PIXEL_MODE_MONO {
                        if row[source_x / 8] & (0x80 >> (source_x % 8)) != 0 {
                            255
                        } else {
                            0
                        }
                    } else {
                        (u32::from(row[source_x]) * 255
                            / bitmap.num_grays.saturating_sub(1).max(1) as u32)
                            .min(255) as u8
                    };
                }
            }
        }
        Ok(GlyphBitmap {
            size,
            left: (slot.bitmap_left as f32 * ratio).round() as i32,
            top: (slot.bitmap_top as f32 * ratio).round() as i32,
            rgba,
            color,
        })
    }
}
