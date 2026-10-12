//! Grapheme shaping and bounded, frame-safe ImGui terminal textures.
//!
//! The PTY worker publishes immutable rows. Only `prepare` shapes changed rows
//! and changes managed textures, before NewFrame. Drawing reuses those quads;
//! selection, link hover and cursor blinking never invalidate glyph layout.
use crate::{
    terminal::*,
    terminal_font::{TerminalFontRasterizer, TerminalFonts},
};
use dear_imgui_rs::{
    Context, ManagedTextureId, OwnedTextureData, TextureFormat, TextureRegion, TextureSubresource,
    Ui, sys,
};
use std::{
    collections::{HashMap, HashSet},
    io,
};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    shape::{Direction, ShapeContext},
    text::{
        Codepoint, Script,
        cluster::{CharCluster, Parser, Status, Token},
    },
};

const PAGE_SIZE: u32 = 1024;
const MAX_GLYPH_PAGES: usize = 8;
const MAX_GLYPH_ENTRIES: usize = 16_384;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const IMAGE_RESIDENT_BYTES: usize = 96 * 1024 * 1024;
const MAX_IMAGE_TEXTURES: usize = 256;
const IMAGE_UPLOAD_BYTES_PER_FRAME: usize = 8 * 1024 * 1024;
const DEFAULT_FG: usize = 258;
const DEFAULT_BG: usize = 259;

#[derive(Clone, Copy, Default, Debug)]
pub struct TerminalRendererStats {
    pub prepared_rows: u64,
    pub rasterized_glyphs: u64,
    pub glyph_upload_bytes: u64,
    pub image_upload_bytes: u64,
    pub glyph_resident_bytes: usize,
    pub image_resident_bytes: usize,
    pub image_resident_textures: usize,
    pub rejected_images: u64,
}
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
struct GlyphKey {
    font: usize,
    glyph: u16,
}
#[derive(Clone, Copy)]
struct AtlasGlyph {
    page: usize,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    color: bool,
}
struct GlyphPage {
    texture: ManagedTextureId,
    x: u32,
    y: u32,
    shelf_height: u32,
}
impl GlyphPage {
    fn allocate(&mut self, width: u32, height: u32) -> Option<[u32; 2]> {
        // One transparent pixel separates glyphs, including at page edges.
        let w = width.checked_add(2)?;
        let h = height.checked_add(2)?;
        if w > PAGE_SIZE || h > PAGE_SIZE {
            return None;
        }
        if self.x + w > PAGE_SIZE {
            self.x = 0;
            self.y += self.shelf_height;
            self.shelf_height = 0;
        }
        if self.y + h > PAGE_SIZE {
            return None;
        }
        let position = [self.x + 1, self.y + 1];
        self.x += w;
        self.shelf_height = self.shelf_height.max(h);
        Some(position)
    }
}
#[derive(Clone, Copy)]
struct Quad {
    page: usize,
    p0: [f32; 2],
    p1: [f32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: u32,
    blink: bool,
    column: usize,
    end_column: usize,
}
struct ContextCellShape {
    font: usize,
    end_column: usize,
    glyphs: Vec<(u16, f32, f32)>,
}
struct Background {
    x0: f32,
    x1: f32,
    color: u32,
    default: bool,
}
struct Decoration {
    p0: [f32; 2],
    p1: [f32; 2],
    color: u32,
    blink: bool,
}
#[derive(Default)]
struct PreparedRow {
    quads: Vec<Quad>,
    backgrounds: Vec<Background>,
    decorations: Vec<Decoration>,
}
struct ImageTile {
    texture: ManagedTextureId,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}
struct PreparedImage {
    revision: u64,
    width: usize,
    height: usize,
    bytes: usize,
    tile_count: usize,
    tiles: Vec<ImageTile>,
    next_tile: usize,
}

pub struct TerminalRenderer {
    shape: ShapeContext,
    scale: ScaleContext,
    rasterizers: HashMap<usize, TerminalFontRasterizer>,
    font_generation: u64,
    density: f32,
    metrics: [f32; 3],
    palette: Option<[[u8; 3]; 260]>,
    faint_palette: Option<[[u8; 3]; 260]>,
    reverse: bool,
    rows: HashMap<u64, PreparedRow>,
    row_order: Vec<u64>,
    glyphs: HashMap<GlyphKey, Option<AtlasGlyph>>,
    pages: Vec<GlyphPage>,
    reset_atlas: bool,
    reset_images: bool,
    images: HashMap<u64, PreparedImage>,
    rejected_images: HashSet<(u64, u64)>,
    stats: TerminalRendererStats,
    warning: Option<String>,
}
impl Default for TerminalRenderer {
    fn default() -> Self {
        Self {
            shape: ShapeContext::new(),
            scale: ScaleContext::new(),
            rasterizers: HashMap::new(),
            font_generation: 0,
            density: 1.0,
            metrics: [8.0, 16.0, 12.0],
            palette: None,
            faint_palette: None,
            reverse: false,
            rows: HashMap::new(),
            row_order: Vec::new(),
            glyphs: HashMap::new(),
            pages: Vec::new(),
            reset_atlas: false,
            reset_images: false,
            images: HashMap::new(),
            rejected_images: HashSet::new(),
            stats: TerminalRendererStats::default(),
            warning: None,
        }
    }
}
impl TerminalRenderer {
    pub fn metrics(&self) -> [f32; 2] {
        [self.metrics[0], self.metrics[1]]
    }
    pub fn density(&self) -> f32 {
        self.density
    }
    pub fn stats(&self) -> TerminalRendererStats {
        self.stats
    }
    pub fn invalidate_textures(&mut self) {
        self.reset_atlas = true;
        self.reset_images = true;
    }
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
    pub fn has_pending_uploads(&self) -> bool {
        self.images
            .values()
            .any(|image| image.next_tile < image.tile_count)
    }
    /// Retire this session's textures between frames. Context retains any
    /// references held by render snapshots until the GPU has consumed them.
    pub fn clear(&mut self, context: &mut Context) {
        for page in self.pages.drain(..) {
            let _ = context.remove_texture(page.texture);
        }
        for (_, image) in self.images.drain() {
            for tile in image.tiles {
                let _ = context.remove_texture(tile.texture);
            }
        }
        self.glyphs.clear();
        self.rejected_images.clear();
        self.rows.clear();
        self.rasterizers.clear();
        self.row_order.clear();
        self.stats.glyph_resident_bytes = 0;
        self.stats.image_resident_bytes = 0;
        self.stats.image_resident_textures = 0;
    }
    pub fn prepare(
        &mut self,
        context: &mut Context,
        snapshot: &TerminalSnapshot,
        fonts: &TerminalFonts,
        density: f32,
    ) -> io::Result<()> {
        self.warning = None;
        if fonts.shape_fonts.is_empty() {
            self.warning = Some("Terminal fonts have not been loaded".into());
        }
        let density = if density.is_finite() {
            density.clamp(0.5, 8.0)
        } else {
            1.0
        };
        if self.font_generation != fonts.generation() || self.density != density || self.reset_atlas
        {
            for page in self.pages.drain(..) {
                context
                    .remove_texture(page.texture)
                    .map_err(io::Error::other)?;
            }
            self.glyphs.clear();
            self.rows.clear();
            self.rasterizers.clear();
            self.font_generation = fonts.generation();
            self.density = density;
            self.metrics = fonts.shaped_metrics();
            if let Some(font) = fonts.shape_fonts.get(fonts.shape_styles[0]) {
                let mut rasterizer = TerminalFontRasterizer::new(font, fonts.size * density)?;
                self.metrics = rasterizer.metrics()?.map(|value| value / density);
                self.rasterizers.insert(fonts.shape_styles[0], rasterizer);
            }
            // The PTY reports integer physical cell sizes. Snap the logical
            // grid to that same stride so image and text columns cannot drift
            // apart at fractional display scales.
            for metric in &mut self.metrics {
                *metric = (*metric * density).ceil() / density;
            }
            self.reset_atlas = false;
            self.stats.glyph_resident_bytes = 0;
        }
        let faint = snapshot.faint_palette.as_ref().map(|palette| **palette);
        if self.palette.as_ref() != Some(snapshot.palette.as_ref())
            || self.faint_palette != faint
            || self.reverse != snapshot.modes.reverse
        {
            self.rows.clear();
            self.palette = Some(*snapshot.palette);
            self.faint_palette = faint;
            self.reverse = snapshot.modes.reverse;
        }
        self.row_order.clear();
        for row in &snapshot.lines {
            if !self.rows.contains_key(&row.revision) {
                let prepared = self.prepare_row(context, &row.cells, snapshot, fonts)?;
                self.rows.insert(row.revision, prepared);
                self.stats.prepared_rows += 1;
            }
            self.row_order.push(row.revision);
        }
        // The layout cache retains just the published viewport. A shifted row
        // keeps its revision and layout; old scrollback has no GPU obligation.
        let visible: HashSet<_> = self.row_order.iter().copied().collect();
        self.rows.retain(|revision, _| visible.contains(revision));
        self.prepare_images(context, snapshot)?;
        Ok(())
    }
    fn prepare_row(
        &mut self,
        context: &mut Context,
        cells: &[TerminalCell],
        snapshot: &TerminalSnapshot,
        fonts: &TerminalFonts,
    ) -> io::Result<PreparedRow> {
        let [cw, ch, ascent] = self.metrics;
        let mut row = PreparedRow::default();
        let mut contextual = shape_contextual_cells(&mut self.shape, fonts, cells);
        for (column, cell) in cells.iter().enumerate() {
            if cell.width == 0 || cell.mode & ATTR_WDUMMY != 0 {
                continue;
            }
            let x = column as f32 * cw;
            let width = cell.width as f32 * cw;
            let (fg, bg) = colors(cell, snapshot);
            let default = cell.bg == TerminalColor::Indexed(DEFAULT_BG)
                && cell.mode & ATTR_REVERSE == 0
                && !snapshot.modes.reverse;
            if let Some(previous) = row.backgrounds.last_mut()
                && previous.color == bg
                && previous.default == default
                && previous.x1 == x
            {
                previous.x1 = x + width;
            } else {
                row.backgrounds.push(Background {
                    x0: x,
                    x1: x + width,
                    color: bg,
                    default,
                });
            }
            let blink = cell.mode & ATTR_BLINK != 0;
            if cell.mode & ATTR_INVISIBLE == 0
                && !cell.text.is_empty()
                && cell.text.as_ref() != " "
                // Rio retains horizontal tabs in the grid for copying. They
                // occupy blank column spacing, not a font's missing glyph.
                && cell.text.as_ref() != "\t"
                && !fonts.shape_fonts.is_empty()
            {
                let style = usize::from(cell.mode & ATTR_BOLD != 0 && cell.mode & ATTR_FAINT == 0)
                    + 2 * usize::from(cell.mode & ATTR_ITALIC != 0);
                let context_shape = contextual.remove(&column);
                let font_index = context_shape
                    .as_ref()
                    .map_or_else(|| choose_font(fonts, style, &cell.text), |shape| shape.font);
                let font = fonts.shape_fonts[font_index].font();
                let mut positioned = Vec::new();
                let mut end_column = column + cell.width as usize;
                if let Some(shape) = context_shape {
                    positioned = shape.glyphs;
                    end_column = shape.end_column;
                } else if cell.text.len() == 1 && cell.character.is_ascii() {
                    positioned.push((font.charmap().map(cell.character), 0.0, 0.0));
                } else {
                    let script = script_for(&cell.text);
                    let mut shaper = self
                        .shape
                        .builder(font)
                        .size(fonts.shape_fonts[font_index].shaping_size(fonts.size))
                        .script(script)
                        .direction(Direction::LeftToRight)
                        // Each terminal grapheme is an item, so optional code
                        // ligatures cannot cross cell boundaries. Keep the
                        // standard features used by emoji and complex scripts.
                        .features(&[("dlig", 0)])
                        .build();
                    shaper.add_str(&cell.text);
                    let mut advance = 0.0;
                    shaper.shape_with(|cluster| {
                        for glyph in cluster.glyphs {
                            positioned.push((glyph.id, advance + glyph.x, glyph.y));
                            advance += glyph.advance;
                        }
                    });
                }
                let start = row.quads.len();
                let mut color_cluster = false;
                for (glyph, gx, gy) in positioned {
                    if let Some(image) = self.glyph(
                        context,
                        fonts,
                        GlyphKey {
                            font: font_index,
                            glyph,
                        },
                    )? {
                        color_cluster |= image.color;
                        row.quads.push(Quad {
                            page: image.page,
                            p0: [x + gx + image.left, ascent - gy - image.top],
                            p1: [
                                x + gx + image.left + image.width,
                                ascent - gy - image.top + image.height,
                            ],
                            uv0: image.uv_min,
                            uv1: image.uv_max,
                            color: if image.color { u32::MAX } else { fg },
                            blink,
                            column,
                            end_column,
                        });
                    }
                }
                if color_cluster && row.quads.len() > start {
                    // Color bitmap strikes use their own baseline. Fit and
                    // center the complete cluster within its terminal cells.
                    let quads = &mut row.quads[start..];
                    let min_x = quads.iter().map(|q| q.p0[0]).fold(f32::INFINITY, f32::min);
                    let min_y = quads.iter().map(|q| q.p0[1]).fold(f32::INFINITY, f32::min);
                    let max_x = quads
                        .iter()
                        .map(|q| q.p1[0])
                        .fold(f32::NEG_INFINITY, f32::max);
                    let max_y = quads
                        .iter()
                        .map(|q| q.p1[1])
                        .fold(f32::NEG_INFINITY, f32::max);
                    let factor = (width / (max_x - min_x)).min(ch / (max_y - min_y)).min(1.0);
                    let dx = x + (width - (max_x - min_x) * factor) * 0.5;
                    let dy = (ch - (max_y - min_y) * factor) * 0.5;
                    for q in quads {
                        q.p0 = [
                            dx + (q.p0[0] - min_x) * factor,
                            dy + (q.p0[1] - min_y) * factor,
                        ];
                        q.p1 = [
                            dx + (q.p1[0] - min_x) * factor,
                            dy + (q.p1[1] - min_y) * factor,
                        ];
                    }
                }
                for q in &mut row.quads[start..] {
                    // Keep each rasterized pixel on a framebuffer pixel. Move
                    // the quad without rescaling its mask (or fitted emoji).
                    let size = [q.p1[0] - q.p0[0], q.p1[1] - q.p0[1]];
                    q.p0 =
                        q.p0.map(|value| (value * self.density).round() / self.density);
                    q.p1 = [q.p0[0] + size[0], q.p0[1] + size[1]];
                    clip_quad(q, [x, 0.0], [end_column as f32 * cw, ch]);
                }
            }
            let underline_color = cell
                .underline_color
                .map(|c| resolve(c, &snapshot.palette))
                .unwrap_or(fg);
            decorations(
                &mut row.decorations,
                cell,
                x,
                width,
                ch,
                ascent,
                underline_color,
                fg,
                blink,
            );
        }
        row.quads.retain(|q| q.p1[0] > q.p0[0] && q.p1[1] > q.p0[1]);
        Ok(row)
    }
    fn glyph(
        &mut self,
        context: &mut Context,
        fonts: &TerminalFonts,
        key: GlyphKey,
    ) -> io::Result<Option<AtlasGlyph>> {
        if let Some(image) = self.glyphs.get(&key) {
            return Ok(*image);
        }
        if self.glyphs.len() >= MAX_GLYPH_ENTRIES {
            self.reset_atlas = true;
            return Ok(None);
        }
        let shape_font = &fonts.shape_fonts[key.font];
        let image = if shape_font.color {
            // Retain Swash's layered color-outline and bitmap-strike support.
            let mut scaler = self
                .scale
                .builder(shape_font.font())
                .size(shape_font.shaping_size(fonts.size) * self.density)
                .hint(true)
                .build();
            let Some(image) = Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .render(&mut scaler, key.glyph) else {
                self.glyphs.insert(key, None);
                return Ok(None);
            };
            image
        } else {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.rasterizers.entry(key.font)
            {
                entry.insert(TerminalFontRasterizer::new(
                    shape_font,
                    fonts.size * self.density,
                )?);
            }
            self.rasterizers
                .get_mut(&key.font)
                .expect("outline font has a FreeType face")
                .rasterize(key.glyph)?
        };
        self.stats.rasterized_glyphs += 1;
        let width = image.placement.width;
        let height = image.placement.height;
        if width == 0 || height == 0 {
            self.glyphs.insert(key, None);
            return Ok(None);
        }
        let mut allocation = None;
        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some(pos) = page.allocate(width, height) {
                allocation = Some((index, pos));
                break;
            }
        }
        if allocation.is_none()
            && self.pages.len() < MAX_GLYPH_PAGES
            && width + 2 <= PAGE_SIZE
            && height + 2 <= PAGE_SIZE
        {
            let pixels = vec![0; PAGE_SIZE as usize * PAGE_SIZE as usize * 4];
            let texture = context.register_texture(
                OwnedTextureData::from_pixels(TextureFormat::RGBA32, PAGE_SIZE, PAGE_SIZE, &pixels)
                    .map_err(io::Error::other)?,
            );
            let mut page = GlyphPage {
                texture,
                x: 0,
                y: 0,
                shelf_height: 0,
            };
            let pos = page
                .allocate(width, height)
                .expect("glyph fits a new atlas page");
            allocation = Some((self.pages.len(), pos));
            self.pages.push(page);
            self.stats.glyph_resident_bytes =
                self.pages.len() * PAGE_SIZE as usize * PAGE_SIZE as usize * 4;
        }
        let Some((page, pos)) = allocation else {
            // Reclaim only at the next preparation boundary. Quads already
            // emitted in this pass keep valid managed texture references.
            self.reset_atlas = true;
            self.warning = Some(
                "Terminal glyph atlas reached its 32 MiB budget; reclaiming cached glyphs".into(),
            );
            return Ok(None);
        };
        let color = image.content == Content::Color;
        let pixels = if color {
            image.data
        } else {
            let channels = if image.content == Content::SubpixelMask {
                4
            } else {
                1
            };
            image
                .data
                .chunks_exact(channels)
                .flat_map(|value| [255, 255, 255, value[0]])
                .collect()
        };
        let region = TextureRegion::new(pos[0], pos[1], width, height).map_err(io::Error::other)?;
        context
            .try_with_texture_mut(self.pages[page].texture, |mut texture| {
                texture.update_subresource(TextureSubresource::new(
                    region,
                    width as usize * 4,
                    &pixels,
                ))
            })
            .map_err(io::Error::other)?;
        self.stats.glyph_upload_bytes += pixels.len() as u64;
        let glyph = AtlasGlyph {
            page,
            uv_min: [
                pos[0] as f32 / PAGE_SIZE as f32,
                pos[1] as f32 / PAGE_SIZE as f32,
            ],
            uv_max: [
                (pos[0] + width) as f32 / PAGE_SIZE as f32,
                (pos[1] + height) as f32 / PAGE_SIZE as f32,
            ],
            left: image.placement.left as f32 / self.density,
            top: image.placement.top as f32 / self.density,
            width: width as f32 / self.density,
            height: height as f32 / self.density,
            color,
        };
        self.glyphs.insert(key, Some(glyph));
        Ok(Some(glyph))
    }
    fn prepare_images(
        &mut self,
        context: &mut Context,
        snapshot: &TerminalSnapshot,
    ) -> io::Result<()> {
        if self.reset_images {
            for (_, image) in self.images.drain() {
                for tile in image.tiles {
                    context
                        .remove_texture(tile.texture)
                        .map_err(io::Error::other)?;
                }
            }
            self.stats.image_resident_bytes = 0;
            self.stats.image_resident_textures = 0;
            self.reset_images = false;
        }
        let current: HashSet<_> = snapshot.images.iter().map(|p| p.image.id).collect();
        self.rejected_images.retain(|(id, revision)| {
            snapshot
                .images
                .iter()
                .any(|placement| placement.image.id == *id && placement.image.revision == *revision)
        });
        let stale: Vec<_> = self
            .images
            .iter()
            .filter(|(id, image)| {
                !current.contains(id)
                    || snapshot
                        .images
                        .iter()
                        .any(|p| p.image.id == **id && p.image.revision != image.revision)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            let image = self.images.remove(&id).unwrap();
            self.stats.image_resident_bytes -= image.bytes;
            self.stats.image_resident_textures -= image.tiles.len();
            for tile in image.tiles {
                context
                    .remove_texture(tile.texture)
                    .map_err(io::Error::other)?;
            }
        }
        // Reserve an image's full tile count at admission, including tiles
        // awaiting upload. Tiny images must obey the same resource bound as
        // large images, and deferred uploads cannot overcommit texture slots.
        let mut reserved_textures: usize = self.images.values().map(|image| image.tile_count).sum();
        let mut upload_budget = IMAGE_UPLOAD_BYTES_PER_FRAME;
        for placement in &snapshot.images {
            let image = &placement.image;
            if !self.images.contains_key(&image.id) {
                let Some(bytes) = image
                    .width
                    .checked_mul(image.height)
                    .and_then(|n| n.checked_mul(4))
                else {
                    continue;
                };
                let tile_count = image.width.div_ceil(PAGE_SIZE as usize)
                    * image.height.div_ceil(PAGE_SIZE as usize);
                if bytes != image.rgba.len()
                    || bytes == 0
                    || bytes > MAX_IMAGE_BYTES
                    || self.stats.image_resident_bytes.saturating_add(bytes) > IMAGE_RESIDENT_BYTES
                    || reserved_textures.saturating_add(tile_count) > MAX_IMAGE_TEXTURES
                {
                    if self.rejected_images.len() < MAX_IMAGE_TEXTURES
                        && self.rejected_images.insert((image.id, image.revision))
                    {
                        self.stats.rejected_images += 1;
                    }
                    self.warning = Some(
                        "Terminal image exceeded the 64 MiB image, 128 MiB presentation, or 256 texture budget"
                            .into(),
                    );
                    continue;
                }
                self.images.insert(
                    image.id,
                    PreparedImage {
                        revision: image.revision,
                        width: image.width,
                        height: image.height,
                        bytes,
                        tile_count,
                        tiles: Vec::new(),
                        next_tile: 0,
                    },
                );
                self.stats.image_resident_bytes += bytes;
                reserved_textures += tile_count;
            }
            let cached = self.images.get_mut(&image.id).unwrap();
            let columns = image.width.div_ceil(PAGE_SIZE as usize);
            let total = cached.tile_count;
            while cached.next_tile < total {
                let x = cached.next_tile % columns * PAGE_SIZE as usize;
                let y = cached.next_tile / columns * PAGE_SIZE as usize;
                let width = (image.width - x).min(PAGE_SIZE as usize);
                let height = (image.height - y).min(PAGE_SIZE as usize);
                let bytes = width * height * 4;
                if bytes > upload_budget {
                    break;
                }
                let mut pixels = Vec::with_capacity(bytes);
                for row in y..y + height {
                    let start = (row * image.width + x) * 4;
                    pixels.extend_from_slice(&image.rgba[start..start + width * 4]);
                }
                let texture = context.register_texture(
                    OwnedTextureData::from_pixels(
                        TextureFormat::RGBA32,
                        width as u32,
                        height as u32,
                        &pixels,
                    )
                    .map_err(io::Error::other)?,
                );
                cached.tiles.push(ImageTile {
                    texture,
                    x,
                    y,
                    width,
                    height,
                });
                cached.next_tile += 1;
                self.stats.image_resident_textures += 1;
                upload_budget -= bytes;
                self.stats.image_upload_bytes += bytes as u64;
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        ui: &Ui,
        snapshot: &TerminalSnapshot,
        origin: [f32; 2],
        clip_max: [f32; 2],
        focused: bool,
        cursor_on: bool,
        blink: bool,
        transparent: bool,
    ) {
        let draw = ui.get_window_draw_list();
        let viewport_origin = ui.window_viewport().pos();
        let origin = std::array::from_fn(|axis| {
            viewport_origin[axis]
                + ((origin[axis] - viewport_origin[axis]) * self.density).round() / self.density
        });
        let _clip = draw.push_clip_rect(origin, clip_max, true);
        let [cw, ch, _] = self.metrics;
        self.draw_images(&draw, snapshot, origin, 0);
        for (index, revision) in self.row_order.iter().enumerate() {
            let Some(row) = self.rows.get(revision) else {
                continue;
            };
            let y = origin[1] + index as f32 * ch;
            for bg in &row.backgrounds {
                if !transparent || !bg.default {
                    draw.add_rect(
                        [origin[0] + bg.x0, y],
                        [origin[0] + bg.x1, y + ch],
                        bg.color,
                    )
                    .filled(true)
                    .build();
                }
            }
        }
        self.draw_images(&draw, snapshot, origin, 1);
        for (index, revision) in self.row_order.iter().enumerate() {
            let Some(row) = self.rows.get(revision) else {
                continue;
            };
            let offset = [origin[0], origin[1] + index as f32 * ch];
            self.draw_quads(ui, &row.quads, offset, blink, None);
            for d in &row.decorations {
                if !d.blink || !blink {
                    draw.add_rect(
                        [offset[0] + d.p0[0], offset[1] + d.p0[1]],
                        [offset[0] + d.p1[0], offset[1] + d.p1[1]],
                        d.color,
                    )
                    .filled(true)
                    .build();
                }
            }
        }
        self.draw_images(&draw, snapshot, origin, 2);
        for span in &snapshot.selection {
            draw.add_rect(
                [
                    origin[0] + span.start as f32 * cw,
                    origin[1] + span.row as f32 * ch,
                ],
                [
                    origin[0] + span.end as f32 * cw,
                    origin[1] + (span.row + 1) as f32 * ch,
                ],
                rgba(115, 160, 235, 100),
            )
            .filled(true)
            .build();
        }
        let cursor = snapshot.cursor;
        if snapshot.display_offset > 0
            || !cursor.visible
            || cursor.row >= self.row_order.len()
            || !cursor_on && cursor.blinking
        {
            return;
        }
        let column = snapshot
            .lines
            .get(cursor.row)
            .and_then(|row| row.cells.get(cursor.col))
            .filter(|cell| cell.width == 0)
            .map_or(cursor.col, |_| cursor.col.saturating_sub(1));
        let width = snapshot
            .lines
            .get(cursor.row)
            .and_then(|row| row.cells.get(column))
            .map_or(1, |cell| cell.width.max(1)) as f32
            * cw;
        let p0 = [
            origin[0] + column as f32 * cw,
            origin[1] + cursor.row as f32 * ch,
        ];
        let p1 = [p0[0] + width, p0[1] + ch];
        let color = resolve(TerminalColor::Indexed(256), &snapshot.palette);
        if !focused {
            draw.add_rect(p0, p1, color).build();
            return;
        }
        match cursor.shape {
            3 | 4 => draw
                .add_rect([p0[0], p1[1] - 2.0], p1, color)
                .filled(true)
                .build(),
            5 | 6 => draw
                .add_rect(p0, [p0[0] + 2.0, p1[1]], color)
                .filled(true)
                .build(),
            _ => {
                draw.add_rect(p0, p1, color).filled(true).build();
                if let Some(row) = self.rows.get(&self.row_order[cursor.row]) {
                    let mut quads: Vec<_> = row
                        .quads
                        .iter()
                        .filter(|q| q.column <= column && q.end_column > column)
                        .copied()
                        .collect();
                    for quad in &mut quads {
                        clip_quad(
                            quad,
                            [column as f32 * cw, 0.0],
                            [column as f32 * cw + width, ch],
                        );
                    }
                    self.draw_quads(
                        ui,
                        &quads,
                        [origin[0], origin[1] + cursor.row as f32 * ch],
                        false,
                        Some(resolve(
                            TerminalColor::Indexed(DEFAULT_BG),
                            &snapshot.palette,
                        )),
                    );
                }
            }
        }
    }
    fn draw_quads(
        &self,
        ui: &Ui,
        quads: &[Quad],
        offset: [f32; 2],
        blink: bool,
        tint: Option<u32>,
    ) {
        let mut start = 0;
        while start < quads.len() {
            let page = quads[start].page;
            let mut end = start + 1;
            while end < quads.len() && quads[end].page == page {
                end += 1;
            }
            let count = quads[start..end]
                .iter()
                .filter(|q| !q.blink || !blink)
                .count();
            if count > 0 {
                // The texture facade records the frame reference; the native
                // primitives stay inside this immediate draw call. Reserve one
                // contiguous run instead of font/texture push-pop per glyph.
                ui.with_bound_context(|| unsafe {
                    let texture = ui
                        .resolve_texture_ref_raw(self.pages[page].texture.into())
                        .expect("prepared atlas texture is active");
                    let list = sys::igGetWindowDrawList();
                    sys::ImDrawList_PushTexture(list, texture);
                    sys::ImDrawList_PrimReserve(list, (count * 6) as i32, (count * 4) as i32);
                    for q in &quads[start..end] {
                        if !q.blink || !blink {
                            sys::ImDrawList_PrimRectUV(
                                list,
                                [offset[0] + q.p0[0], offset[1] + q.p0[1]].into(),
                                [offset[0] + q.p1[0], offset[1] + q.p1[1]].into(),
                                q.uv0.into(),
                                q.uv1.into(),
                                tint.unwrap_or(q.color),
                            );
                        }
                    }
                    sys::ImDrawList_PopTexture(list);
                });
            }
            start = end;
        }
    }
    fn draw_images(
        &self,
        draw: &dear_imgui_rs::DrawListMut<'_>,
        snapshot: &TerminalSnapshot,
        origin: [f32; 2],
        layer: u8,
    ) {
        let mut placements: Vec<_> = snapshot
            .images
            .iter()
            .filter(|p| image_layer(p.z_index) == layer)
            .collect();
        placements.sort_by_key(|p| p.z_index);
        for placement in placements {
            let Some(image) = self.images.get(&placement.image.id) else {
                continue;
            };
            let [u0, v0, u1, v1] = placement.source_rect;
            let sx = u0 * image.width as f32;
            let sy = v0 * image.height as f32;
            let sw = (u1 - u0) * image.width as f32;
            let sh = (v1 - v0) * image.height as f32;
            if sw <= 0.0 || sh <= 0.0 {
                continue;
            }
            for tile in &image.tiles {
                let x0 = sx.max(tile.x as f32);
                let y0 = sy.max(tile.y as f32);
                let x1 = (sx + sw).min((tile.x + tile.width) as f32);
                let y1 = (sy + sh).min((tile.y + tile.height) as f32);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                draw.add_image(
                    tile.texture,
                    [
                        origin[0] + (placement.x + (x0 - sx) * placement.width / sw) / self.density,
                        origin[1]
                            + (placement.y + (y0 - sy) * placement.height / sh) / self.density,
                    ],
                    [
                        origin[0] + (placement.x + (x1 - sx) * placement.width / sw) / self.density,
                        origin[1]
                            + (placement.y + (y1 - sy) * placement.height / sh) / self.density,
                    ],
                    [
                        (x0 - tile.x as f32) / tile.width as f32,
                        (y0 - tile.y as f32) / tile.height as f32,
                    ],
                    [
                        (x1 - tile.x as f32) / tile.width as f32,
                        (y1 - tile.y as f32) / tile.height as f32,
                    ],
                    u32::MAX,
                );
            }
        }
    }
}
fn image_layer(z_index: i32) -> u8 {
    if z_index < -(1 << 30) {
        0
    } else if z_index < 0 {
        1
    } else {
        2
    }
}

/// Preserve terminal order while giving joining scripts their adjacent-cell
/// context. The shaper's source ranges map each output cluster back to its
/// allocated cells; proportional advances never move the next terminal cell.
fn shape_contextual_cells(
    shape: &mut ShapeContext,
    fonts: &TerminalFonts,
    cells: &[TerminalCell],
) -> HashMap<usize, ContextCellShape> {
    let mut result = HashMap::new();
    if fonts.shape_fonts.is_empty() {
        return result;
    }
    let text_style = ATTR_BOLD | ATTR_FAINT | ATTR_ITALIC | ATTR_REVERSE | ATTR_BLINK;
    let mut column = 0;
    while column < cells.len() {
        let first = &cells[column];
        if first.width == 0 || first.text.is_ascii() || first.mode & ATTR_INVISIBLE != 0 {
            column += 1;
            continue;
        }
        let script = script_for(&first.text);
        if !script.is_complex() && !script.is_joined() {
            column += first.width as usize;
            continue;
        }
        let style = usize::from(first.mode & ATTR_BOLD != 0 && first.mode & ATTR_FAINT == 0)
            + 2 * usize::from(first.mode & ATTR_ITALIC != 0);
        let font_index = choose_font(fonts, style, &first.text);
        let mut text = String::new();
        let mut sources = Vec::new();
        let mut end = column;
        while end < cells.len() {
            let cell = &cells[end];
            if cell.width == 0 {
                end += 1;
                continue;
            }
            if cell.text.is_ascii()
                || script_for(&cell.text) != script
                || cell.mode & ATTR_INVISIBLE != 0
                || cell.mode & text_style != first.mode & text_style
                || cell.fg != first.fg
                || cell.bg != first.bg
                || choose_font(fonts, style, &cell.text) != font_index
            {
                break;
            }
            sources.push((text.len() as u32, end, end + cell.width as usize));
            text.push_str(&cell.text);
            result.insert(
                end,
                ContextCellShape {
                    font: font_index,
                    end_column: end + cell.width as usize,
                    glyphs: Vec::new(),
                },
            );
            end += cell.width as usize;
        }
        let font = fonts.shape_fonts[font_index].font();
        let mut shaper = shape
            .builder(font)
            .size(fonts.shape_fonts[font_index].shaping_size(fonts.size))
            .script(script)
            .direction(Direction::LeftToRight)
            // Required ligatures and joining remain active. Optional code
            // ligatures remain disabled in these script-specific runs.
            .features(&[("liga", 0), ("dlig", 0)])
            .build();
        shaper.add_str(&text);
        shaper.shape_with(|cluster| {
            let first_index = sources
                .partition_point(|(offset, _, _)| *offset <= cluster.source.start)
                .saturating_sub(1);
            let last_index = sources
                .partition_point(|(offset, _, _)| *offset < cluster.source.end)
                .saturating_sub(1);
            let (_, column, _) = sources[first_index];
            let last_column = sources[last_index.max(first_index)].2;
            let target = result
                .get_mut(&column)
                .expect("source cluster belongs to this run");
            target.end_column = target.end_column.max(last_column);
            let mut advance = 0.0;
            for glyph in cluster.glyphs {
                target.glyphs.push((glyph.id, advance + glyph.x, glyph.y));
                advance += glyph.advance;
            }
        });
        column = end.max(column + 1);
    }
    result
}
fn script_for(text: &str) -> Script {
    text.chars()
        .map(|ch| ch.properties().script())
        .find(|s| !matches!(s, Script::Common | Script::Inherited | Script::Unknown))
        .unwrap_or(Script::Latin)
}
fn choose_font(fonts: &TerminalFonts, style: usize, text: &str) -> usize {
    let primary = fonts.shape_styles[style];
    if text.is_ascii() {
        return primary;
    }
    let emoji = text.chars().any(|ch| ch.properties().is_emoji()) && !text.contains('\u{fe0e}');
    let mut candidates = vec![primary];
    candidates.extend(4..fonts.shape_fonts.len());
    if emoji {
        candidates.sort_by_key(|index| !fonts.shape_fonts[*index].color);
    }
    let script = script_for(text);
    let mut best = primary;
    let mut best_score = 0;
    for index in candidates {
        let font = fonts.shape_fonts[index].font();
        let charmap = font.charmap();
        let mut parser = Parser::new(
            script,
            text.char_indices().map(|(offset, ch)| Token {
                ch,
                offset: offset as u32,
                len: ch.len_utf8() as u8,
                info: ch.into(),
                data: 0,
            }),
        );
        let mut cluster = CharCluster::new();
        let mut complete = true;
        while parser.next(&mut cluster) {
            complete &= cluster.map(|ch| charmap.map(ch)) == Status::Complete;
        }
        if complete {
            return index;
        }
        let score = text.chars().filter(|&ch| charmap.map(ch) != 0).count();
        if score > best_score {
            best = index;
            best_score = score;
        }
    }
    best
}
fn resolve(color: TerminalColor, palette: &[[u8; 3]; 260]) -> u32 {
    let [r, g, b] = match color {
        TerminalColor::Rgb(rgb) => rgb,
        TerminalColor::Indexed(index) => palette.get(index).copied().unwrap_or(palette[DEFAULT_FG]),
    };
    rgb(r, g, b)
}
fn colors(cell: &TerminalCell, snapshot: &TerminalSnapshot) -> (u32, u32) {
    let mut fg = resolve(cell.fg, &snapshot.palette);
    let mut bg = resolve(cell.bg, &snapshot.palette);
    if cell.mode & ATTR_BOLD != 0
        && cell.mode & ATTR_FAINT == 0
        && let TerminalColor::Indexed(index @ 0..=7) = cell.fg
    {
        fg = resolve(TerminalColor::Indexed(index + 8), &snapshot.palette);
    }
    if cell.mode & ATTR_FAINT != 0 {
        fg = match snapshot.faint_color(cell.fg) {
            Some([r, g, b])
                if cell.bg == TerminalColor::Indexed(DEFAULT_BG) && !snapshot.modes.reverse =>
            {
                rgb(r, g, b)
            }
            _ => {
                let bits = fg;
                rgb(
                    (bits as u8) / 2,
                    ((bits >> 8) as u8) / 2,
                    ((bits >> 16) as u8) / 2,
                )
            }
        };
    }
    if snapshot.modes.reverse ^ (cell.mode & ATTR_REVERSE != 0) {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}
fn clip_quad(q: &mut Quad, min: [f32; 2], max: [f32; 2]) {
    for axis in 0..2 {
        let length = q.p1[axis] - q.p0[axis];
        if length <= 0.0 {
            continue;
        }
        let uv_length = q.uv1[axis] - q.uv0[axis];
        let low = (min[axis] - q.p0[axis]).max(0.0).min(length);
        let high = (q.p1[axis] - max[axis]).max(0.0).min(length);
        q.p0[axis] += low;
        q.p1[axis] -= high;
        q.uv0[axis] += uv_length * low / length;
        q.uv1[axis] -= uv_length * high / length;
    }
}
#[allow(clippy::too_many_arguments)]
fn decorations(
    out: &mut Vec<Decoration>,
    cell: &TerminalCell,
    x: f32,
    width: f32,
    ch: f32,
    ascent: f32,
    color: u32,
    fg: u32,
    blink: bool,
) {
    let y = (ascent + 1.0).min(ch - 1.0);
    let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32, color| {
        out.push(Decoration {
            p0: [x0, y0],
            p1: [x1, y1],
            color,
            blink,
        })
    };
    match cell.underline {
        TerminalUnderline::None => {}
        TerminalUnderline::Single => rect(x, y, x + width, y + 1.0, color),
        TerminalUnderline::Double => {
            rect(x, (y - 1.0).max(0.0), x + width, y, color);
            rect(
                x,
                (y + 1.0).min(ch - 1.0),
                x + width,
                (y + 2.0).min(ch),
                color,
            );
        }
        TerminalUnderline::Curly => {
            let mut dx = 0.0;
            while dx < width {
                let dy = if (dx as usize / 2).is_multiple_of(2) {
                    0.0
                } else {
                    1.0
                };
                rect(
                    x + dx,
                    y + dy,
                    x + (dx + 2.0).min(width),
                    (y + dy + 1.0).min(ch),
                    color,
                );
                dx += 2.0;
            }
        }
        TerminalUnderline::Dotted | TerminalUnderline::Dashed => {
            let length = if cell.underline == TerminalUnderline::Dotted {
                1.0
            } else {
                3.0
            };
            let mut dx = 0.0;
            while dx < width {
                rect(x + dx, y, x + (dx + length).min(width), y + 1.0, color);
                dx += length + 2.0;
            }
        }
    }
    if cell.mode & ATTR_STRUCK != 0 {
        let sy = ascent * 0.65;
        rect(x, sy, x + width, sy + 1.0, fg);
    }
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16) | (u32::from(a) << 24)
}
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    rgba(r, g, b, 255)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    #[test]
    fn text_quads_stay_on_physical_pixels_in_fractionally_positioned_panes() {
        use dear_imgui_rs::{Condition, FramePrepareOptions};
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload(&mut context, root, 20.0).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut term = Terminal::new(30, 3);
        term.feed("ASCII e\u{301} accents".as_bytes());
        let snapshot = term.snapshot();
        let mut renderer = TerminalRenderer::default();
        for density in [1.25, 2.0] {
            renderer
                .prepare(&mut context, &snapshot, &fonts, density)
                .unwrap();
            context.prepare_frame(
                FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0)
                    .framebuffer_scale([density, density]),
            );
            let ui = context.frame();
            ui.window("Pixel alignment")
                .position([0.0, 0.0], Condition::Always)
                .size([800.0, 600.0], Condition::Always)
                .build(|| unsafe {
                    let list = sys::igGetWindowDrawList();
                    let start = (*list).VtxBuffer.Size as usize;
                    renderer.draw(
                        ui,
                        &snapshot,
                        [10.23, 30.17],
                        [790.0, 590.0],
                        true,
                        false,
                        false,
                        true,
                    );
                    let vertices = std::slice::from_raw_parts(
                        (*list).VtxBuffer.Data,
                        (*list).VtxBuffer.Size as usize,
                    );
                    assert!(vertices.len() > start);
                    for vertex in &vertices[start..] {
                        for position in [vertex.pos.x, vertex.pos.y] {
                            assert!(
                                (position * density - (position * density).round()).abs() < 0.0001,
                                "position {position} at density {density}"
                            );
                        }
                    }
                });
            drop(context.render_legacy());
        }
        renderer.clear(&mut context);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn arabic_uses_neighbor_context_without_reordering_terminal_cells() {
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload(&mut context, root, 18.0).unwrap();
        let mut term = Terminal::new(10, 2);
        term.feed("بب".as_bytes());
        let snapshot = term.snapshot();
        let cells = &snapshot.lines[0].cells;
        let mut shape = ShapeContext::new();
        let alone = shape_contextual_cells(&mut shape, &fonts, &cells[..1]);
        let joined = shape_contextual_cells(&mut shape, &fonts, &cells[..2]);
        assert_eq!(joined.len(), 2);
        assert_ne!(
            joined[&0].glyphs[0].0, alone[&0].glyphs[0].0,
            "the first letter uses its joining form"
        );
        assert_ne!(
            joined[&1].glyphs[0].0, alone[&0].glyphs[0].0,
            "the second letter uses its joining form"
        );
        assert_eq!(joined[&0].end_column, 1);
        assert_eq!(joined[&1].end_column, 2);
    }
    #[test]
    fn ls_column_tabs_keep_spacing_without_emitting_missing_glyphs() {
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload(&mut context, root, 18.0).unwrap();
        let mut term = Terminal::new(96, 4);
        // BSD ls uses horizontal tabs, including repeated tabs, to align its
        // filename columns. Captured from /bin/ls -C in this repository, with
        // the PTY's normal newline conversion and directory colors on assets.
        term.feed(
            concat!(
                "AGENTS.md\tLICENSE\t\tREADME.md\texamples\tsrc\t\tvendor\r\n",
                "Cargo.lock\tLICENSES\t\x1b[34massets\x1b[0m\t\tresources\ttarget\r\n",
                "Cargo.toml\tNOTICE\t\tcrates\t\tscripts\t\ttests\r\n",
            )
            .as_bytes(),
        );
        let snapshot = term.snapshot();
        assert_eq!(snapshot.lines[0].cells[9].text.as_ref(), "\t");
        assert_eq!(snapshot.lines[0].cells[16].character, 'L');
        assert_eq!(snapshot.lines[0].cells[32].character, 'R');
        assert_eq!(snapshot.lines[0].cells[80].character, 'v');
        assert_eq!(snapshot.lines[1].cells[32].character, 'a');
        assert_eq!(snapshot.lines[2].cells[64].character, 't');

        let mut renderer = TerminalRenderer::default();
        renderer
            .prepare(&mut context, &snapshot, &fonts, 1.0)
            .unwrap();
        for line in &snapshot.lines[..3] {
            let row = &renderer.rows[&line.revision];
            let visible_columns: Vec<_> = row.quads.iter().map(|quad| quad.column).collect();
            let filename_columns: Vec<_> = line
                .cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.character != ' ' && cell.character != '\t')
                .map(|(column, _)| column)
                .collect();
            assert_eq!(visible_columns, filename_columns);
        }
        assert!(renderer.glyphs.keys().all(|key| key.glyph != 0));
        renderer.clear(&mut context);
    }
    #[test]
    fn whole_graphemes_fit_their_terminal_cells() {
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let mut fonts = TerminalFonts::default();
        fonts.reload(&mut context, root, 18.0).unwrap();
        let mut term = Terminal::new(40, 4);
        term.feed("e\u{301} 中 👩\u{200d}💻".as_bytes());
        let snapshot = term.snapshot();
        let mut renderer = TerminalRenderer::default();
        renderer
            .prepare(&mut context, &snapshot, &fonts, 1.0)
            .unwrap();
        let row = &renderer.rows[&snapshot.lines[0].revision];
        assert!(row.quads.iter().any(|q| q.column == 0));
        let [cw, ch, _] = renderer.metrics;
        for quad in &row.quads {
            let cell = &snapshot.lines[0].cells[quad.column];
            assert!(cell.width > 0);
            assert!(quad.p0[0] >= quad.column as f32 * cw);
            assert!(quad.p1[0] <= (quad.column as f32 + cell.width as f32) * cw);
            assert!(quad.p0[1] >= 0.0 && quad.p1[1] <= ch);
        }
        #[cfg(target_os = "macos")]
        {
            let index = choose_font(&fonts, 0, "👩\u{200d}💻");
            assert!(
                fonts.shape_fonts[index].color,
                "emoji fallback is selected for the complete ZWJ sequence"
            );
            let emoji: Vec<_> = row
                .quads
                .iter()
                .filter(|q| snapshot.lines[0].cells[q.column].text.contains('\u{200d}'))
                .collect();
            assert_eq!(emoji.len(), 1, "ZWJ emoji becomes one colored glyph");
            assert_eq!(emoji[0].color, u32::MAX);
        }
        renderer.clear(&mut context);
    }
}
