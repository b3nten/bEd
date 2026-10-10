use crate::{
    assets::{AssetManager, AssetState},
    model::{self, Content, Document, Inline, Style},
};
use dear_imgui_rs::{MouseButton, MouseCursor, StyleColor, Ui};
use pulldown_cmark::Alignment;
use std::collections::HashMap;

enum Kind {
    Text {
        text: String,
        style: Style,
        size: f32,
        muted: bool,
    },
    Background {
        header: bool,
    },
    Line,
    Task(bool),
    Image {
        source: String,
        alt: String,
        link: Option<String>,
        uv0: [f32; 2],
        uv1: [f32; 2],
    },
}
struct Primitive {
    pos: [f32; 2],
    size: [f32; 2],
    kind: Kind,
}
pub struct Layout {
    items: Vec<Primitive>,
    maximum_bottom: Vec<f32>,
    decorations: Vec<Primitive>,
    decorations_bottom: Vec<f32>,
    pub size: [f32; 2],
    pub anchors: HashMap<String, f32>,
}
struct Builder<'a> {
    ui: &'a Ui,
    assets: &'a AssetManager,
    base: f32,
    items: Vec<Primitive>,
    anchors: HashMap<String, f32>,
    width: f32,
    budget: usize,
}

impl Builder<'_> {
    fn put(&mut self, pos: [f32; 2], size: [f32; 2], kind: Kind) -> Result<(), String> {
        let bytes = match &kind {
            Kind::Text { text, style, .. } => {
                text.len() + style.link.as_ref().map_or(0, String::len)
            }
            Kind::Image {
                source, alt, link, ..
            } => source.len() + alt.len() + link.as_ref().map_or(0, String::len),
            _ => 0,
        };
        self.budget = self
            .budget
            .saturating_add((bytes + std::mem::size_of::<Primitive>()).saturating_mul(2));
        if self.budget > model::MAX_BYTES {
            return Err("Markdown layout exceeds the 128 MiB preview limit.".into());
        }
        self.width = self.width.max(pos[0] + size[0]);
        self.items.push(Primitive { pos, size, kind });
        Ok(())
    }
    fn measure(&self, text: &str, size: f32) -> f32 {
        self.ui.calc_text_size(text)[0] * size / self.base
    }
    fn text(
        &mut self,
        pos: [f32; 2],
        text: &str,
        style: &Style,
        size: f32,
        muted: bool,
    ) -> Result<f32, String> {
        let width = self.measure(text, size);
        self.put(
            pos,
            [width, size * 1.35],
            Kind::Text {
                text: text.to_owned(),
                style: style.clone(),
                size,
                muted,
            },
        )?;
        Ok(width)
    }
    fn inline(
        &mut self,
        spans: &[Inline],
        left: f32,
        top: f32,
        width: f32,
        size: f32,
        bold: bool,
    ) -> Result<f32, String> {
        let mut x = left;
        let mut y = top;
        let line = size * 1.45;
        for span in spans {
            match span {
                Inline::Break => {
                    x = left;
                    y += line;
                }
                Inline::Text { text, style } => {
                    let mut style = style.clone();
                    style.strong |= bold;
                    for token in text.split_inclusive(char::is_whitespace) {
                        let token = if x == left {
                            token.trim_start_matches(char::is_whitespace)
                        } else {
                            token
                        };
                        if token.is_empty() {
                            continue;
                        }
                        let token_width =
                            self.measure(token, size) + if style.code { size * 0.25 } else { 0.0 };
                        if x > left && x + token_width > left + width {
                            x = left;
                            y += line;
                        }
                        if token_width <= width {
                            if style.code {
                                self.put(
                                    [x - size * 0.08, y],
                                    [token_width + size * 0.16, line * 0.93],
                                    Kind::Background { header: false },
                                )?;
                            }
                            self.text([x, y], token, &style, size, false)?;
                            x += token_width;
                        } else {
                            // Split very long URLs/words at Unicode scalar boundaries.
                            let mut piece = String::new();
                            let mut piece_width = 0.0;
                            for ch in token.chars() {
                                let mut bytes = [0; 4];
                                let ch_width = self.measure(ch.encode_utf8(&mut bytes), size);
                                if !piece.is_empty() && x + piece_width + ch_width > left + width {
                                    if style.code {
                                        self.put(
                                            [x, y],
                                            [piece_width, line * 0.93],
                                            Kind::Background { header: false },
                                        )?;
                                    }
                                    self.text([x, y], &piece, &style, size, false)?;
                                    piece.clear();
                                    piece_width = 0.0;
                                    x = left;
                                    y += line;
                                }
                                piece.push(ch);
                                piece_width += ch_width;
                            }
                            if !piece.is_empty() {
                                if style.code {
                                    self.put(
                                        [x, y],
                                        [piece_width, line * 0.93],
                                        Kind::Background { header: false },
                                    )?;
                                }
                                self.text([x, y], &piece, &style, size, false)?;
                                x += piece_width;
                            }
                        }
                    }
                }
                Inline::Image {
                    destination,
                    alt,
                    link,
                } => {
                    if x > left {
                        y += line;
                    }
                    x = left;
                    match self.assets.get(destination) {
                        AssetState::Ready {
                            size: image,
                            uv0,
                            uv1,
                        } => {
                            let scale = (width / image[0].max(1) as f32).min(1.0);
                            let size = [image[0] as f32 * scale, image[1] as f32 * scale];
                            self.put(
                                [left, y],
                                size,
                                Kind::Image {
                                    source: destination.clone(),
                                    alt: alt.clone(),
                                    link: link.clone(),
                                    uv0,
                                    uv1,
                                },
                            )?;
                            y += size[1] + self.base * 0.5;
                        }
                        state => {
                            let message = match state {
                                AssetState::Loading => "Loading image…",
                                AssetState::Error(error) => error,
                                _ => unreachable!(),
                            };
                            let label = if alt.is_empty() {
                                format!("[Image] {message}")
                            } else {
                                format!("[{alt}] {message}")
                            };
                            let style = Style {
                                link: link.clone(),
                                ..Style::default()
                            };
                            // Placeholders retain alt text and a useful failure reason.
                            let end = self.inline(
                                &[Inline::Text { text: label, style }],
                                left + self.base * 0.6,
                                y + self.base * 0.4,
                                (width - self.base * 1.2).max(self.base),
                                size,
                                false,
                            )?;
                            self.put(
                                [left, y],
                                [width, end - y + self.base * 0.4],
                                Kind::Background { header: false },
                            )?;
                            y = end + self.base * 0.8;
                        }
                    }
                }
            }
        }
        Ok(y + line)
    }
}

pub fn build(
    ui: &Ui,
    document: &Document,
    width: f32,
    assets: &AssetManager,
) -> Result<Layout, String> {
    let base = ui.current_font_size();
    let margin = base * 1.0;
    let width = width.max(base * 5.0);
    let mut builder = Builder {
        ui,
        assets,
        base,
        items: Vec::new(),
        anchors: HashMap::new(),
        width: 0.0,
        budget: 0,
    };
    let mut y = margin;
    for block in &document.blocks {
        let quote_indent = block.quotes as f32 * base * 1.1;
        let left = margin + block.indent as f32 * base * 1.8 + quote_indent;
        let available = (width - left - margin).max(base * 3.0);
        let top = y;
        let marker_x = left - base * 1.35;
        if let Some(checked) = block.task {
            builder.put(
                [marker_x, y + base * 0.15],
                [base * 0.85; 2],
                Kind::Task(checked),
            )?;
        } else if let Some(marker) = &block.marker {
            builder.text([marker_x, y], marker, &Style::default(), base, false)?;
        }
        match &block.content {
            Content::Prose(spans) => {
                y = builder.inline(spans, left, y, available, base, false)?;
            }
            Content::Heading {
                level,
                anchor,
                spans,
            } => {
                let size = base
                    * match level {
                        1 => 1.9,
                        2 => 1.55,
                        3 => 1.3,
                        4 => 1.15,
                        _ => 1.0,
                    };
                y += base * 0.35;
                builder.anchors.insert(anchor.clone(), y);
                y = builder.inline(spans, left, y, available, size, true)?;
                if *level <= 2 {
                    builder.put([left, y + base * 0.1], [available, 1.0], Kind::Line)?;
                    y += base * 0.45;
                }
            }
            Content::Rule => {
                y += base * 0.4;
                builder.put([left, y], [available, 1.0], Kind::Line)?;
                y += base * 0.6;
            }
            Content::Code { language, text } => {
                let start = y;
                let padding = base * 0.65;
                let mut code_width = available;
                y += padding;
                if !language.is_empty() {
                    builder.text(
                        [left + padding, y],
                        language,
                        &Style::default(),
                        base * 0.8,
                        true,
                    )?;
                    y += base * 1.35;
                }
                for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
                    let mut x = left + padding;
                    // Bounded pieces also make horizontal clipping cheap for huge code lines.
                    let expanded = line.replace('\t', "    ");
                    let mut start = 0;
                    while start < expanded.len() {
                        let mut end = (start + 1024).min(expanded.len());
                        while !expanded.is_char_boundary(end) {
                            end -= 1;
                        }
                        x += builder.text(
                            [x, y],
                            &expanded[start..end],
                            &Style::default(),
                            base,
                            false,
                        )?;
                        start = end;
                    }
                    code_width = code_width.max(x - left + padding);
                    y += base * 1.45;
                }
                y += padding;
                builder.put(
                    [left, start],
                    [code_width, y - start],
                    Kind::Background { header: false },
                )?;
            }
            Content::Table { alignments, rows } => {
                let columns = alignments
                    .len()
                    .max(rows.iter().map(Vec::len).max().unwrap_or(0));
                let padding = base * 0.6;
                let widths: Vec<_> = (0..columns)
                    .map(|col| {
                        rows.iter()
                            .filter_map(|row| row.get(col))
                            .map(|cell| builder.measure(&model::plain_text(cell), base))
                            .fold(base * 7.0, f32::max)
                            .min(base * 28.0)
                            + padding * 2.0
                    })
                    .collect();
                let total: f32 = widths.iter().sum();
                for (row_index, row) in rows.iter().enumerate() {
                    let row_top = y;
                    let mut row_bottom = y + base * 1.45 + padding * 2.0;
                    let mut x = left;
                    for (col, &column_width) in widths.iter().enumerate() {
                        if let Some(cell) = row.get(col) {
                            let natural = builder.measure(&model::plain_text(cell), base);
                            let extra = (column_width - padding * 2.0 - natural).max(0.0);
                            let align = match alignments.get(col) {
                                Some(Alignment::Center) => extra / 2.0,
                                Some(Alignment::Right) => extra,
                                _ => 0.0,
                            };
                            let bottom = builder.inline(
                                cell,
                                x + padding + align,
                                y + padding,
                                (column_width - padding * 2.0 - align).max(base),
                                base,
                                row_index == 0,
                            )?;
                            row_bottom = row_bottom.max(bottom + padding);
                        }
                        x += column_width;
                    }
                    builder.put(
                        [left, row_top],
                        [total, row_bottom - row_top],
                        Kind::Background {
                            header: row_index == 0,
                        },
                    )?;
                    builder.put([left, row_bottom], [total, 1.0], Kind::Line)?;
                    x = left;
                    for column_width in &widths {
                        builder.put([x, row_top], [1.0, row_bottom - row_top], Kind::Line)?;
                        x += column_width;
                    }
                    builder.put([x, row_top], [1.0, row_bottom - row_top], Kind::Line)?;
                    y = row_bottom;
                }
            }
        }
        for quote in 0..block.quotes {
            let x = left - (block.quotes - quote) as f32 * base * 1.1 + base * 0.3;
            builder.put([x, top], [base * 0.14, (y - top).max(base)], Kind::Line)?;
        }
        y += base * 0.65;
    }
    // Tall code backgrounds and quote rules use a separate visibility index;
    // they must not force scanning every preceding text line while scrolling.
    let (mut decorations, mut items): (Vec<_>, Vec<_>) = builder
        .items
        .into_iter()
        .partition(|item| matches!(item.kind, Kind::Background { .. } | Kind::Line));
    items.sort_by(|a, b| a.pos[1].total_cmp(&b.pos[1]));
    decorations.sort_by(|a, b| a.pos[1].total_cmp(&b.pos[1]));
    let mut maximum = 0.0_f32;
    let maximum_bottom = items
        .iter()
        .map(|item| {
            maximum = maximum.max(item.pos[1] + item.size[1]);
            maximum
        })
        .collect();
    maximum = 0.0;
    let decorations_bottom = decorations
        .iter()
        .map(|item| {
            maximum = maximum.max(item.pos[1] + item.size[1]);
            maximum
        })
        .collect();
    Ok(Layout {
        items,
        maximum_bottom,
        decorations,
        decorations_bottom,
        size: [builder.width + margin, y + margin],
        anchors: builder.anchors,
    })
}

impl Layout {
    /// Draw only intersecting primitives. The monotonic bottom index includes tall
    /// images and backgrounds whose start lies above the visible region.
    pub fn draw(
        &self,
        ui: &Ui,
        origin: [f32; 2],
        assets: &AssetManager,
        host: &bed_plugin::HostContext<'_>,
    ) -> Option<String> {
        let draw = ui.get_window_draw_list();
        let clip_min = draw.clip_rect_min();
        let clip_max = draw.clip_rect_max();
        let top = clip_min[1] - origin[1];
        let bottom = clip_max[1] - origin[1];
        let first = self.maximum_bottom.partition_point(|end| *end < top);
        let last = self.items.partition_point(|item| item.pos[1] <= bottom);
        let text_color = ui.style_color(StyleColor::Text);
        let muted = ui.style_color(StyleColor::TextDisabled);
        let border = ui.style_color(StyleColor::Separator);
        let background = ui.style_color(StyleColor::FrameBg);
        let header = ui.style_color(StyleColor::TableHeaderBg);
        let link_color = ui.style_color(StyleColor::TextLink);
        let visible = &self.items[first.min(last)..last];
        let first = self.decorations_bottom.partition_point(|end| *end < top);
        let last = self
            .decorations
            .partition_point(|item| item.pos[1] <= bottom);
        let decorations = &self.decorations[first.min(last)..last];
        // Paint backgrounds first so wrapped cell text and image placeholders remain visible.
        for item in decorations {
            if let Kind::Background { header: is_header } = &item.kind {
                let pos = [origin[0] + item.pos[0], origin[1] + item.pos[1]];
                draw.add_rect(
                    pos,
                    [pos[0] + item.size[0], pos[1] + item.size[1]],
                    if *is_header { header } else { background },
                )
                .filled(true)
                .rounding(3.0)
                .build();
            }
        }
        for item in decorations {
            if matches!(item.kind, Kind::Line) {
                let pos = [origin[0] + item.pos[0], origin[1] + item.pos[1]];
                draw.add_rect(pos, [pos[0] + item.size[0], pos[1] + item.size[1]], border)
                    .filled(true)
                    .build();
            }
        }
        let mut clicked = None;
        for item in visible {
            let pos = [origin[0] + item.pos[0], origin[1] + item.pos[1]];
            let end = [pos[0] + item.size[0], pos[1] + item.size[1]];
            if end[0] < clip_min[0] || pos[0] > clip_max[0] || end[1] < clip_min[1] {
                continue;
            }
            match &item.kind {
                Kind::Text {
                    text,
                    style,
                    size,
                    muted: is_muted,
                } => {
                    let color = if style.link.is_some() {
                        link_color
                    } else if *is_muted {
                        muted
                    } else {
                        text_color
                    };
                    let raw = unsafe { dear_imgui_rs::sys::igGetWindowDrawList() };
                    // Only newly appended glyph vertices belong to this text operation.
                    let start = unsafe { (*raw).VtxBuffer.Size as usize };
                    draw.add_text_with_font(ui.current_font(), *size, pos, color, text, 0.0, None);
                    if style.strong {
                        draw.add_text_with_font(
                            ui.current_font(),
                            *size,
                            [pos[0] + size * 0.035, pos[1]],
                            color,
                            text,
                            0.0,
                            None,
                        );
                    }
                    if style.emphasis {
                        // A small synthetic italic shear works with the host's chosen font.
                        unsafe {
                            let buffer = &mut (*raw).VtxBuffer;
                            if buffer.Size as usize > start {
                                for vertex in std::slice::from_raw_parts_mut(
                                    buffer.Data.add(start),
                                    buffer.Size as usize - start,
                                ) {
                                    vertex.pos.x += (pos[1] + size - vertex.pos.y) * 0.16;
                                }
                            }
                        }
                    }
                    if style.strike {
                        draw.add_line(
                            [pos[0], pos[1] + size * 0.55],
                            [end[0], pos[1] + size * 0.55],
                            color,
                        )
                        .build();
                    }
                    if let Some(target) = &style.link {
                        draw.add_line(
                            [pos[0], pos[1] + size * 1.05],
                            [end[0], pos[1] + size * 1.05],
                            color,
                        )
                        .build();
                        if ui.is_window_hovered() && ui.is_mouse_hovering_rect(pos, end) {
                            ui.set_mouse_cursor(Some(MouseCursor::Hand));
                            ui.tooltip_text(target);
                            if ui.is_mouse_clicked(MouseButton::Left) {
                                clicked = Some(target.clone());
                            }
                        }
                    }
                }
                Kind::Line => {
                    draw.add_rect(pos, end, border).filled(true).build();
                }
                Kind::Task(checked) => {
                    draw.add_rect(pos, end, border).rounding(2.0).build();
                    if *checked {
                        draw.add_line(
                            [pos[0] + item.size[0] * 0.2, pos[1] + item.size[1] * 0.5],
                            [pos[0] + item.size[0] * 0.45, pos[1] + item.size[1] * 0.75],
                            text_color,
                        )
                        .thickness(1.5)
                        .build();
                        draw.add_line(
                            [pos[0] + item.size[0] * 0.45, pos[1] + item.size[1] * 0.75],
                            [pos[0] + item.size[0] * 0.85, pos[1] + item.size[1] * 0.2],
                            text_color,
                        )
                        .thickness(1.5)
                        .build();
                    }
                }
                Kind::Image {
                    source,
                    alt,
                    link,
                    uv0,
                    uv1,
                } => {
                    if let Some(texture) = host.texture(assets.texture_handle()) {
                        draw.add_image(texture, pos, end, *uv0, *uv1, [1.0; 4]);
                    }
                    if ui.is_window_hovered() && ui.is_mouse_hovering_rect(pos, end) {
                        ui.tooltip_text(if alt.is_empty() { source } else { alt });
                        if let Some(target) = link {
                            ui.set_mouse_cursor(Some(MouseCursor::Hand));
                            if ui.is_mouse_clicked(MouseButton::Left) {
                                clicked = Some(target.clone());
                            }
                        }
                    }
                }
                Kind::Background { .. } => {}
            }
        }
        clicked
    }
}
