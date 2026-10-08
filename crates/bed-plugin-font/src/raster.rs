use crate::font::{FontEngine, GlyphBitmap, GlyphDetails};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Preview,
    Glyphs,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RasterRequest {
    pub mode: Mode,
    pub pixels: [u32; 2],
    pub logical: [f32; 2],
    pub label_height: f32,
    pub size: f32,
    pub sample: String,
    pub ligatures: bool,
    pub kerning: bool,
    pub direction: usize,
    pub pan: [f32; 2],
    pub glyph_scroll: f32,
    pub selected: u32,
    pub background: [f32; 4],
    pub ink: [f32; 4],
}

#[derive(Clone, Copy)]
pub(crate) struct GridLayout {
    pub columns: usize,
    pub cell: [f32; 2],
    pub viewport: [f32; 2],
    pub detail: [f32; 4],
}
impl GridLayout {
    pub fn new(logical: [f32; 2], label_height: f32, size: f32) -> Self {
        let [width, height] = logical;
        let (grid, detail) = if width >= 640.0 {
            let detail_width = (width * 0.28).clamp(180.0, 260.0);
            (
                [width - detail_width, height],
                [width - detail_width, 0.0, detail_width, height],
            )
        } else {
            let detail_height = (height * 0.35).clamp(80.0, 180.0).min(height * 0.5);
            (
                [width, height - detail_height],
                [0.0, height - detail_height, width, detail_height],
            )
        };
        let columns = ((grid[0] / (size.clamp(24.0, 64.0) + 36.0)).floor() as usize).clamp(1, 16);
        Self {
            columns,
            cell: [
                grid[0] / columns as f32,
                size.clamp(24.0, 64.0) + label_height + 32.0,
            ],
            viewport: grid,
            detail,
        }
    }
    pub fn scroll_max(&self, glyphs: usize) -> f32 {
        (glyphs.div_ceil(self.columns) as f32 * self.cell[1] - self.viewport[1]).max(0.0)
    }
    pub fn scroll_to(&self, id: usize, glyphs: usize) -> f32 {
        ((id / self.columns) as f32 * self.cell[1]).min(self.scroll_max(glyphs))
    }
    pub fn visible(&self, scroll: f32, glyphs: usize) -> std::ops::Range<usize> {
        let scroll = scroll.clamp(0.0, self.scroll_max(glyphs));
        let first = (scroll / self.cell[1]).floor() as usize * self.columns;
        let end = ((scroll + self.viewport[1]) / self.cell[1]).ceil() as usize * self.columns;
        first.min(glyphs)..end.min(glyphs)
    }
    pub fn cell_origin(&self, id: usize, scroll: f32) -> [f32; 2] {
        [
            (id % self.columns) as f32 * self.cell[0],
            (id / self.columns) as f32 * self.cell[1] - scroll,
        ]
    }
    pub fn glyph_at(&self, point: [f32; 2], scroll: f32, glyphs: usize) -> Option<usize> {
        if point[0] < 0.0
            || point[1] < 0.0
            || point[0] >= self.viewport[0]
            || point[1] >= self.viewport[1]
        {
            return None;
        }
        let col = (point[0] / self.cell[0]) as usize;
        let row = ((point[1] + scroll) / self.cell[1]) as usize;
        let id = row * self.columns + col;
        (id < glyphs).then_some(id)
    }
}

pub(crate) struct Raster {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
    pub content: [f32; 2],
    pub shaped_glyphs: usize,
    pub missing_glyphs: usize,
    pub selected: GlyphDetails,
}

struct Surface {
    size: [u32; 2],
    clip: [u32; 2],
    pixels: Vec<u8>,
    ink: [u8; 4],
}
impl Surface {
    fn new(request: &RasterRequest) -> Result<Self, String> {
        let [w, h] = request.pixels;
        if w == 0
            || h == 0
            || w > 8192
            || h > 8192
            || u64::from(w) * u64::from(h) > 64 * 1024 * 1024 / 4
        {
            return Err("Font preview exceeds the 64 MiB canvas limit".into());
        }
        let mut background = request.background.map(byte);
        background[3] = 255;
        let pixels = background.repeat(w as usize * h as usize);
        Ok(Self {
            size: request.pixels,
            clip: request.pixels,
            pixels,
            ink: request.ink.map(byte),
        })
    }
    fn glyph(&mut self, bitmap: &GlyphBitmap, baseline: [f32; 2]) {
        let origin = [
            baseline[0].round() as i64 + i64::from(bitmap.left),
            baseline[1].round() as i64 - i64::from(bitmap.top),
        ];
        let left = origin[0].max(0);
        let top = origin[1].max(0);
        let right = (origin[0] + i64::from(bitmap.size[0])).min(i64::from(self.clip[0]));
        let bottom = (origin[1] + i64::from(bitmap.size[1])).min(i64::from(self.clip[1]));
        for y in top..bottom {
            for x in left..right {
                let source_index = ((y - origin[1]) as usize * bitmap.size[0] as usize
                    + (x - origin[0]) as usize)
                    * 4;
                let source = &bitmap.rgba[source_index..][..4];
                let alpha = u32::from(source[3])
                    * u32::from(if bitmap.color { 255 } else { self.ink[3] })
                    / 255;
                let dest_index = (y as usize * self.size[0] as usize + x as usize) * 4;
                let dest = &mut self.pixels[dest_index..][..4];
                for c in 0..3 {
                    let color = if bitmap.color { source[c] } else { self.ink[c] };
                    dest[c] =
                        ((u32::from(color) * alpha + u32::from(dest[c]) * (255 - alpha) + 127)
                            / 255) as u8;
                }
            }
        }
    }
    fn centered(&mut self, bitmap: &GlyphBitmap, rect: [f32; 4], scale: f32) {
        let baseline = [
            (rect[0] + rect[2] * 0.5) * scale - bitmap.size[0] as f32 * 0.5 - bitmap.left as f32,
            (rect[1] + rect[3] * 0.5) * scale - bitmap.size[1] as f32 * 0.5 + bitmap.top as f32,
        ];
        self.glyph(bitmap, baseline);
    }
}
fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn render(engine: &mut FontEngine, request: &RasterRequest) -> Result<Raster, String> {
    let mut surface = Surface::new(request)?;
    let scale = (request.pixels[1] as f32 / request.logical[1].max(1.0)).clamp(0.01, 8.0);
    let mut content = request.logical;
    let mut shaped_glyphs = 0;
    let mut missing_glyphs = 0;
    let selected = engine.details(request.selected.min(engine.info.glyphs.len() as u32 - 1))?;
    match request.mode {
        Mode::Preview => {
            let pixels = (request.size * scale).round().clamp(1.0, 1024.0) as u32;
            let em = f32::from(engine.info.units_per_em.max(1));
            let line_height =
                (f32::from(engine.info.line_height) / em * request.size).max(request.size * 1.35);
            let ascender = (f32::from(engine.info.ascender) / em * request.size).max(request.size);
            let mut widest = 0.0f32;
            let mut lines = 0;
            for (line_index, line) in request.sample.split('\n').take(256).enumerate() {
                let shaped = engine.shape(
                    line.trim_end_matches('\r'),
                    pixels as f32,
                    request.ligatures,
                    request.kerning,
                    request.direction,
                )?;
                shaped_glyphs += shaped.glyphs.len();
                missing_glyphs += shaped.glyphs.iter().filter(|g| g.id == 0).count();
                let width = shaped.width / scale;
                widest = widest.max(width);
                lines += 1;
                let y =
                    (24.0 + ascender + line_index as f32 * line_height - request.pan[1]) * scale;
                if y + (pixels as f32) < 0.0 || y - (pixels as f32) > request.pixels[1] as f32 {
                    continue;
                }
                let x = if shaped.rtl {
                    (request.logical[0] - 24.0 - width).max(24.0)
                } else {
                    24.0
                };
                for glyph in shaped.glyphs {
                    let bitmap = engine.bitmap(glyph.id, pixels)?;
                    surface.glyph(
                        &bitmap,
                        [
                            (x - request.pan[0]) * scale + glyph.position[0],
                            y + glyph.position[1],
                        ],
                    );
                }
            }
            content = [widest + 48.0, lines as f32 * line_height + 48.0];
        }
        Mode::Glyphs => {
            let grid = GridLayout::new(request.logical, request.label_height, request.size);
            let scroll = request
                .glyph_scroll
                .clamp(0.0, grid.scroll_max(engine.info.glyphs.len()));
            let pixels = (request.size.clamp(16.0, 64.0) * scale).round().max(1.0) as u32;
            surface.clip = [
                ((grid.viewport[0] * scale).round() as u32).min(surface.size[0]),
                ((grid.viewport[1] * scale).round() as u32).min(surface.size[1]),
            ];
            for id in grid.visible(scroll, engine.info.glyphs.len()) {
                let bitmap = engine.bitmap(id as u32, pixels)?;
                let origin = grid.cell_origin(id, scroll);
                let rect = [
                    origin[0],
                    origin[1],
                    grid.cell[0],
                    (grid.cell[1] - request.label_height - 10.0).max(1.0),
                ];
                surface.centered(&bitmap, rect, scale);
            }
            surface.clip = surface.size;
            let mut rect = grid.detail;
            rect[1] += request.label_height * 2.0 + 8.0;
            rect[3] = (rect[3] - request.label_height * 2.0 - 16.0).max(1.0);
            let pixels = (rect[2].min(rect[3]) * 0.75 * scale)
                .round()
                .clamp(1.0, 512.0) as u32;
            let bitmap = engine.bitmap(selected.id, pixels)?;
            surface.centered(&bitmap, rect, scale);
        }
    }
    Ok(Raster {
        size: request.pixels,
        rgba: surface.pixels.into(),
        content,
        shaped_glyphs,
        missing_glyphs,
        selected,
    })
}
