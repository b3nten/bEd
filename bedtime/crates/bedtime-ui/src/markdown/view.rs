//! `MarkdownView`: Markdown drawn as a document (a readable measure, centred,
//! scrolling) or compact (for tooltips and popups, in the caller's scroll
//! area). Links come back from `show` for the caller to follow; headings can
//! be scrolled to by anchor; images beside `base` show with `egui::Image`.
//!
//! Code blocks are coloured from spans the caller gives (`colour`, from its
//! analysis of the whole source), or with the `syntax` feature by
//! `code_language`, which analyses them on a thread of its own.
use super::{Block, Callout, Code, Item, Markdown, Run, parse};
use crate::{
    UiContext, egui,
    theme::{self, SyntaxPalette, SyntaxRole, Theme},
};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

/// How a `MarkdownView` is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkdownStyle {
    /// A page: its own scroll area, a readable measure centred in it, large
    /// headings with rules under the first two levels.
    #[default]
    Document,
    /// Tight, for a tooltip or popup: no scroll area of its own, smaller
    /// headings, less space.
    Compact,
}

/// Parsed Markdown, ready to draw, and what drawing it needs to remember.
pub struct MarkdownView {
    markdown: Markdown,
    /// The folder image paths are relative to; without one only absolute
    /// paths show.
    pub base: Option<PathBuf>,
    /// Show images (needs bedtime-ui's `image` feature for its loaders).
    pub images: bool,
    pub style: MarkdownStyle,
    /// A heading to scroll to, by anchor, when next drawn.
    pub scroll_to: Option<String>,
    #[cfg(feature = "syntax")]
    analysis: Option<Analysis>,
}

/// Code blocks being coloured: their text, fenced, in one Markdown source
/// (so each is coloured as its own language), and where each block is in it.
#[cfg(feature = "syntax")]
struct Analysis {
    session: bedtime_syntax::AnalysisSession,
    blocks: Vec<Range<usize>>,
}

impl MarkdownView {
    pub fn new(text: &str) -> Self {
        Self::from(parse(text))
    }

    /// `text` as one paragraph, not read as Markdown.
    pub fn plain(text: &str) -> Self {
        Self::from(Markdown {
            blocks: vec![Block::Paragraph(vec![Run {
                text: text.to_owned(),
                style: Default::default(),
                link: None,
                image: None,
            }])],
        })
    }

    pub fn style(mut self, style: MarkdownStyle) -> Self {
        self.style = style;
        self
    }

    pub fn markdown(&self) -> &Markdown {
        &self.markdown
    }

    /// Colour code blocks from `spans` over the source (sorted, disjoint).
    pub fn colour(&mut self, spans: &[(Range<usize>, SyntaxRole)]) {
        self.markdown.colour(spans);
        #[cfg(feature = "syntax")]
        {
            self.analysis = None;
        }
    }

    /// Colour code blocks on a thread of the view's own, each as its fence's
    /// language, or `default` (a name or file name) without one. Without the
    /// `syntax` feature they stay plain.
    pub fn code_language(self, default: &str) -> Self {
        #[cfg(feature = "syntax")]
        {
            let mut this = self;
            let mut source = String::new();
            let mut blocks = Vec::new();
            this.markdown.visit_code(&mut |code| {
                let language = if code.language.is_empty() {
                    default
                } else {
                    code.language.as_str()
                };
                // A fence longer than any run of backticks inside.
                let longest = code
                    .text
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                source.push_str(&format!("{fence}{language}\n"));
                let start = source.len();
                source.push_str(&code.text);
                blocks.push(start..source.len());
                source.push_str(&format!("\n{fence}\n\n"));
            });
            if !blocks.is_empty() {
                let mut session = bedtime_syntax::AnalysisSession::new();
                session.request(std::sync::Arc::new(source), 0, "markdown", None);
                this.analysis = Some(Analysis { session, blocks });
            }
            this
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = default;
            self
        }
    }

    /// Whether code colours are still on their way.
    pub fn colouring(&self) -> bool {
        #[cfg(feature = "syntax")]
        if let Some(analysis) = &self.analysis {
            return analysis.session.status() == &bedtime_syntax::AnalysisStatus::Pending;
        }
        false
    }

    /// Draw it with the theme and rounding of `context`. Returns the link
    /// clicked, if one was.
    pub fn show(&mut self, ui: &mut egui::Ui, context: &UiContext<'_>) -> Option<String> {
        self.show_themed(ui, context.theme, context.metrics.rounding)
    }

    /// Draw it with `theme`, for a caller without a `UiContext`.
    pub fn show_themed(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        rounding: f32,
    ) -> Option<String> {
        self.poll(ui.ctx());
        let mut clicked = None;
        let mut draw = Draw::new(ui, theme, rounding, &mut clicked, self.scroll_to.take());
        draw.base = self.base.clone();
        draw.images = self.images;
        match self.style {
            MarkdownStyle::Document => draw.document(ui, &self.markdown),
            MarkdownStyle::Compact => {
                draw.compact = true;
                let size = draw.body.size;
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(size * 0.35, size * 0.45);
                    draw.blocks(ui, &self.markdown.blocks);
                });
            }
        }
        // Not reached yet (the heading isn't drawn): try again next time.
        self.scroll_to = draw.scroll_to.take();
        clicked
    }

    /// Take code colours that arrived.
    fn poll(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "syntax")]
        if let Some(analysis) = &mut self.analysis {
            analysis.session.poll();
            match analysis.session.status() {
                bedtime_syntax::AnalysisStatus::Pending => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(16));
                }
                bedtime_syntax::AnalysisStatus::Ready => {
                    if let Some(observation) = analysis.session.observation() {
                        let spans: Vec<_> = observation
                            .highlights
                            .iter()
                            .filter_map(|h| {
                                Some((h.range.clone(), SyntaxRole::of_capture(&h.capture)?))
                            })
                            .collect();
                        let blocks = std::mem::take(&mut analysis.blocks);
                        let mut index = 0;
                        self.markdown.visit_code(&mut |code| {
                            if let Some(range) = blocks.get(index) {
                                code.highlights = spans_in(&spans, range.clone());
                            }
                            index += 1;
                        });
                    }
                    self.analysis = None;
                }
                _ => self.analysis = None,
            }
        }
        #[cfg(not(feature = "syntax"))]
        let _ = ctx;
    }
}

impl From<Markdown> for MarkdownView {
    fn from(markdown: Markdown) -> Self {
        Self {
            markdown,
            base: None,
            images: false,
            style: MarkdownStyle::Document,
            scroll_to: None,
            #[cfg(feature = "syntax")]
            analysis: None,
        }
    }
}

/// `spans` within `range`, moved to start at 0.
#[cfg(feature = "syntax")]
fn spans_in(
    spans: &[(Range<usize>, SyntaxRole)],
    range: Range<usize>,
) -> Vec<(Range<usize>, SyntaxRole)> {
    let first = spans.partition_point(|(span, _)| span.end <= range.start);
    spans[first..]
        .iter()
        .take_while(|(span, _)| span.start < range.end)
        .map(|(span, role)| {
            let start = span.start.max(range.start) - range.start;
            let end = span.end.min(range.end) - range.start;
            (start..end, *role)
        })
        .filter(|(span, _)| !span.is_empty())
        .collect()
}

/// The file an image's source names, relative to `base`; `None` for web and
/// inline (`data:`) images.
pub fn source_path(base: Option<&Path>, source: &str) -> Option<PathBuf> {
    if source.contains("://") || source.starts_with("data:") || source.is_empty() {
        return None;
    }
    let file = source.split(['#', '?']).next().unwrap_or(source);
    let file = percent_decode(file);
    let file = Path::new(&file);
    if file.is_absolute() {
        return Some(file.to_owned());
    }
    Some(base?.join(file))
}

/// `%20` and the like decoded, as in a link to a file.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(i + 1).and_then(|&b| hex(b)),
                bytes.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What drawing needs: fonts and colours, and what was clicked.
struct Draw<'a> {
    palette: SyntaxPalette,
    text: egui::Color32,
    muted: egui::Color32,
    accent: egui::Color32,
    code_fill: egui::Color32,
    border: egui::Color32,
    callouts: [egui::Color32; 5],
    body: egui::FontId,
    mono: egui::FontId,
    rounding: f32,
    clicked: &'a mut Option<String>,
    scroll_to: Option<String>,
    /// Numbers code blocks and tables, for their scroll areas' IDs.
    counter: usize,
    /// The Markdown file's folder, which image paths are relative to.
    base: Option<PathBuf>,
    /// Show images (`bedtime_ui::image`'s loaders read and decode them).
    images: bool,
    /// The compact style: tighter, smaller headings, no rules under them.
    compact: bool,
}

impl<'a> Draw<'a> {
    fn new(
        ui: &egui::Ui,
        theme: &Theme,
        rounding: f32,
        clicked: &'a mut Option<String>,
        scroll_to: Option<String>,
    ) -> Self {
        let p = &theme.palette;
        let c = theme::color;
        Self {
            palette: SyntaxPalette::from_theme(theme),
            text: c(p.text),
            muted: c(p.muted),
            accent: c(p.accent),
            code_fill: c(theme::mix(p.panel_background, p.text, 0.06)),
            border: c(theme::mix(p.panel_background, p.text, 0.16)),
            callouts: [
                c(p.accent),
                c(p.success),
                c(theme::mix(p.accent, p.error, 0.5)),
                c(p.warning),
                c(p.error),
            ],
            body: egui::TextStyle::Body.resolve(ui.style()),
            mono: egui::TextStyle::Monospace.resolve(ui.style()),
            rounding,
            clicked,
            scroll_to,
            counter: 0,
            base: None,
            images: false,
            compact: false,
        }
    }

    fn document(&mut self, ui: &mut egui::Ui, markdown: &Markdown) {
        let size = self.body.size;
        egui::ScrollArea::vertical()
            .id_salt("markdown")
            .auto_shrink(false)
            .show(ui, |ui| {
                // A readable measure, centred in wide panels.
                let width = ui.available_width();
                let measure = (size * 52.0).min(width - size * 3.0).max(size * 8.0);
                let side = ((width - measure) / 2.0).max(0.0);
                ui.add_space(size * 1.2);
                let rect = egui::Rect::from_min_size(
                    ui.cursor().min + egui::vec2(side, 0.0),
                    egui::vec2(measure, f32::INFINITY),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(size * 0.4, size * 0.75);
                    self.blocks(ui, &markdown.blocks);
                });
                ui.add_space(size * 3.0);
            });
    }

    fn blocks(&mut self, ui: &mut egui::Ui, blocks: &[Block]) {
        for block in blocks {
            self.block(ui, block);
        }
    }

    fn block(&mut self, ui: &mut egui::Ui, block: &Block) {
        match block {
            Block::Heading {
                level,
                text,
                anchor,
            } => self.heading(ui, *level, text, anchor),
            Block::Paragraph(runs) => {
                self.inline(ui, runs, &self.body.clone(), self.text);
            }
            Block::Code(code) => self.code(ui, code),
            Block::Quote { kind, blocks } => self.quote(ui, *kind, blocks),
            Block::List { start, items } => self.list(ui, *start, items),
            Block::Table {
                alignments,
                head,
                rows,
            } => self.table(ui, alignments, head, rows),
            Block::Rule => {
                ui.add(egui::Separator::default().spacing(self.body.size));
            }
            Block::Html(html) => {
                let job = egui::text::LayoutJob::simple(
                    html.clone(),
                    self.mono.clone(),
                    self.muted,
                    ui.available_width(),
                );
                ui.add(egui::Label::new(job).selectable(true));
            }
        }
    }

    fn heading(&mut self, ui: &mut egui::Ui, level: u8, text: &[Run], anchor: &str) {
        let scales = if self.compact {
            [1.2, 1.12, 1.05, 1.0, 1.0, 0.95]
        } else {
            [1.75, 1.45, 1.25, 1.1, 1.0, 0.95]
        };
        let scale = scales[(level.clamp(1, 6) - 1) as usize];
        if level <= 2 && !self.compact {
            ui.add_space(self.body.size * 0.4);
        }
        let font = crate::fonts::monospace_bold(self.body.size * scale);
        let color = if level >= 6 { self.muted } else { self.text };
        let response = self.inline(ui, text, &font, color);
        if self.scroll_to.as_deref() == Some(anchor) {
            response.scroll_to_me(Some(egui::Align::TOP));
            self.scroll_to = None;
        }
        if level <= 2 && !self.compact {
            let y = ui.cursor().top() - ui.spacing().item_spacing.y * 0.5;
            let x = ui.max_rect().x_range();
            ui.painter()
                .hline(x, y, egui::Stroke::new(1.0, self.border));
        }
    }

    /// Wrapped text with clickable links, and images in place.
    fn inline(
        &mut self,
        ui: &mut egui::Ui,
        runs: &[Run],
        font: &egui::FontId,
        color: egui::Color32,
    ) -> egui::Response {
        if runs.iter().all(|run| run.image.is_none()) {
            return self.text(ui, runs, font, color);
        }
        // Text and images, one after another, wrapping.
        let full = ui.max_rect().width();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut rest = runs;
            while let Some(first) = rest.first() {
                let same = |run: &Run| match &first.image {
                    Some(source) => run.image.as_ref() == Some(source),
                    None => run.image.is_none(),
                };
                let length = rest.iter().take_while(|run| same(run)).count();
                let (piece, after) = rest.split_at(length);
                rest = after;
                match &first.image {
                    Some(source) if self.image(ui, source, piece, full) => {}
                    _ => {
                        self.text(ui, piece, font, color);
                    }
                }
            }
        })
        .response
    }

    /// The image `source`, no wider than `full`; false when it can't show
    /// (not loaded, failed, or on the web), for its description to show.
    fn image(&mut self, ui: &mut egui::Ui, source: &str, runs: &[Run], full: f32) -> bool {
        let Some(path) = source_path(self.base.as_deref(), source) else {
            return false;
        };
        if !self.images {
            return false;
        }
        let description: String = runs.iter().map(|run| run.text.as_str()).collect();
        let uri = format!("file://{}", path.display());
        let size = match ui.ctx().try_load_image(&uri, egui::SizeHint::default()) {
            Ok(egui::load::ImagePoll::Ready { image }) => image.source_size,
            Ok(egui::load::ImagePoll::Pending { .. }) => return false,
            Err(error) => {
                let error = format!("{}: {error}", path.display());
                self.text(ui, runs, &self.body.clone(), self.text)
                    .on_hover_text(error);
                return true;
            }
        };
        let shown = if size.x > full {
            size * (full / size.x)
        } else {
            size
        };
        let response = ui.add(
            egui::Image::new(uri)
                .fit_to_exact_size(shown)
                .alt_text(&description)
                .sense(egui::Sense::click()),
        );
        let link = runs.iter().find_map(|run| run.link.clone());
        if let Some(link) = link {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if response.clicked() {
                *self.clicked = Some(link.clone());
            }
            response.on_hover_text_at_pointer(link);
        } else if !description.is_empty() {
            response.on_hover_text(description);
        }
        true
    }

    /// Wrapped text with clickable links (images as their descriptions).
    fn text(
        &mut self,
        ui: &mut egui::Ui,
        runs: &[Run],
        font: &egui::FontId,
        color: egui::Color32,
    ) -> egui::Response {
        let mut job = egui::text::LayoutJob::default();
        let mut links = Vec::new();
        for run in runs {
            let start = job.text.len();
            let text = if run.style.image {
                format!(
                    "[{}]",
                    if run.text.is_empty() {
                        "image"
                    } else {
                        &run.text
                    }
                )
            } else {
                run.text.clone()
            };
            job.append(&text, 0.0, self.format(run, font, color));
            if let Some(link) = &run.link {
                links.push((start..job.text.len(), link.clone()));
            }
        }
        job.wrap.max_width = ui.available_width();
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let response = ui.add(egui::Label::new(galley.clone()).selectable(true));
        if links.is_empty() {
            return response;
        }
        let link = response.hover_pos().and_then(|pointer| {
            let cursor = galley.cursor_from_pos(pointer - response.rect.min);
            let byte = galley
                .job
                .text
                .char_indices()
                .nth(cursor.index.into())
                .map_or(galley.job.text.len(), |(byte, _)| byte);
            // The cursor sits between characters: the one after it.
            links
                .iter()
                .find(|(range, _)| range.contains(&byte))
                .map(|(_, link)| link.clone())
        });
        if let Some(link) = link {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if response.clicked() {
                *self.clicked = Some(link.clone());
            }
            return response.on_hover_text_at_pointer(link);
        }
        response
    }

    fn format(&self, run: &Run, font: &egui::FontId, color: egui::Color32) -> egui::TextFormat {
        let style = run.style;
        let mut format = egui::TextFormat {
            font_id: if style.code {
                egui::FontId::monospace(font.size * 0.95)
            } else if style.strong && !matches!(font.family, egui::FontFamily::Name(_)) {
                crate::fonts::monospace_bold(font.size)
            } else {
                font.clone()
            },
            color,
            italics: style.emphasis || style.image,
            ..Default::default()
        };
        if style.code {
            format.background = self.code_fill;
        }
        if style.strike {
            format.strikethrough = egui::Stroke::new(1.0, color);
        }
        if style.image {
            format.color = self.muted;
        }
        if run.link.is_some() && !style.image {
            format.color = self.accent;
            format.underline = egui::Stroke::new(1.0, self.accent.gamma_multiply(0.5));
        }
        format
    }

    fn code(&mut self, ui: &mut egui::Ui, code: &Code) {
        self.counter += 1;
        let id = ui.id().with(("code", self.counter));
        let mut job = egui::text::LayoutJob::default();
        let plain = |job: &mut egui::text::LayoutJob, text: &str, color| {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: self.mono.clone(),
                    color,
                    ..Default::default()
                },
            )
        };
        let mut at = 0;
        for (range, role) in &code.highlights {
            if range.start < at {
                continue;
            }
            plain(&mut job, &code.text[at..range.start], self.text);
            plain(
                &mut job,
                &code.text[range.clone()],
                self.palette.role(*role),
            );
            at = range.end;
        }
        plain(&mut job, &code.text[at..], self.text);
        let frame = egui::Frame::NONE
            .fill(self.code_fill)
            .corner_radius(self.rounding)
            .inner_margin(egui::Margin::same(
                (self.body.size * if self.compact { 0.5 } else { 0.8 }) as i8,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
                    ui.add(egui::Label::new(job).extend().selectable(true));
                });
            });
        let rect = frame.response.rect;
        // In a tooltip the language is the file's: no need to say.
        if !code.language.is_empty() && !self.compact {
            ui.painter().text(
                rect.right_top() + egui::vec2(-6.0, 4.0),
                egui::Align2::RIGHT_TOP,
                &code.language,
                egui::FontId::monospace(self.body.size * 0.75),
                self.muted,
            );
        }
        frame
            .response
            .interact(egui::Sense::click())
            .context_menu(|ui| {
                if ui.button("Copy Code").clicked() {
                    ui.ctx().copy_text(code.text.clone());
                }
            });
    }

    fn quote(&mut self, ui: &mut egui::Ui, kind: Option<Callout>, blocks: &[Block]) {
        let bar = match kind {
            None => self.border,
            Some(kind) => self.callouts[kind as usize],
        };
        let size = self.body.size;
        let response = egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: (size * 1.1) as i8,
                ..Default::default()
            })
            .show(ui, |ui| {
                if let Some(kind) = kind {
                    let title = match kind {
                        Callout::Note => "Note",
                        Callout::Tip => "Tip",
                        Callout::Important => "Important",
                        Callout::Warning => "Warning",
                        Callout::Caution => "Caution",
                    };
                    ui.label(
                        egui::RichText::new(title)
                            .font(crate::fonts::monospace_bold(size))
                            .color(bar),
                    );
                }
                let text = self.text;
                self.text = self.muted.lerp_to_gamma(text, 0.5);
                self.blocks(ui, blocks);
                self.text = text;
            })
            .response;
        let rect = response.rect;
        ui.painter().vline(
            rect.left() + 1.5,
            rect.y_range(),
            egui::Stroke::new(3.0, bar),
        );
    }

    fn list(&mut self, ui: &mut egui::Ui, start: Option<u64>, items: &[Item]) {
        let size = self.body.size;
        let gutter = match start {
            Some(start) => {
                let widest = (start + items.len() as u64).to_string().len() as f32;
                size * (widest * 0.6 + 1.2)
            }
            None => size * 1.4,
        };
        let spacing = ui.spacing().item_spacing.y;
        for (i, item) in items.iter().enumerate() {
            let top = ui.cursor().min;
            let row = self.body.size * 1.0 + 2.0;
            ui.scope(|ui| {
                // Tight items sit closer than paragraphs.
                ui.spacing_mut().item_spacing.y = spacing * 0.6;
                ui.spacing_mut().indent = gutter;
                ui.visuals_mut().indent_has_left_vline = false;
                ui.indent(("item", i), |ui| self.blocks(ui, &item.blocks));
            });
            let painter = ui.painter();
            let center_y = top.y + row * 0.55;
            match (item.checked, start) {
                (Some(checked), _) => {
                    let side = size * 0.8;
                    let rect = egui::Rect::from_center_size(
                        egui::pos2(top.x + gutter - size * 0.9, center_y),
                        egui::vec2(side, side),
                    );
                    let stroke =
                        egui::Stroke::new(1.2, if checked { self.accent } else { self.muted });
                    painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                    if checked {
                        painter.add(egui::Shape::line(
                            vec![
                                rect.lerp_inside(egui::vec2(0.22, 0.52)),
                                rect.lerp_inside(egui::vec2(0.42, 0.72)),
                                rect.lerp_inside(egui::vec2(0.78, 0.28)),
                            ],
                            egui::Stroke::new(1.6, self.accent),
                        ));
                    }
                }
                (None, Some(start)) => {
                    painter.text(
                        egui::pos2(top.x + gutter - size * 0.4, top.y),
                        egui::Align2::RIGHT_TOP,
                        format!("{}.", start + i as u64),
                        self.body.clone(),
                        self.muted,
                    );
                }
                (None, None) => {
                    painter.circle_filled(
                        egui::pos2(top.x + gutter - size * 0.8, center_y),
                        size * 0.16,
                        self.muted,
                    );
                }
            }
        }
    }

    fn table(
        &mut self,
        ui: &mut egui::Ui,
        alignments: &[pulldown_cmark::Alignment],
        head: &[Vec<Run>],
        rows: &[Vec<Vec<Run>>],
    ) {
        self.counter += 1;
        let id = ui.id().with(("table", self.counter));
        let bold = crate::fonts::monospace_bold(self.body.size);
        let body = self.body.clone();
        let cell_width = self.body.size * 24.0;
        egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
            egui::Frame::NONE
                .stroke(egui::Stroke::new(1.0, self.border))
                .corner_radius(self.rounding)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    egui::Grid::new(id.with("grid"))
                        .striped(true)
                        .spacing(egui::vec2(self.body.size * 1.2, self.body.size * 0.4))
                        .show(ui, |ui| {
                            let cell =
                                |draw: &mut Self,
                                 ui: &mut egui::Ui,
                                 column: usize,
                                 runs: &[Run],
                                 font: &egui::FontId| {
                                    let align = match alignments.get(column) {
                                        Some(pulldown_cmark::Alignment::Right) => egui::Align::Max,
                                        Some(pulldown_cmark::Alignment::Center) => {
                                            egui::Align::Center
                                        }
                                        _ => egui::Align::Min,
                                    };
                                    ui.with_layout(egui::Layout::top_down(align), |ui| {
                                        ui.set_max_width(cell_width);
                                        draw.inline(ui, runs, font, draw.text);
                                    });
                                };
                            for (column, runs) in head.iter().enumerate() {
                                cell(self, ui, column, runs, &bold);
                            }
                            ui.end_row();
                            for row in rows {
                                for (column, runs) in row.iter().enumerate() {
                                    cell(self, ui, column, runs, &body);
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_files_are_decoded() {
        assert_eq!(percent_decode("My%20Notes.md"), "My Notes.md");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn sources_are_files_beside_the_markdown() {
        let base = Some(Path::new("/docs"));
        assert_eq!(
            source_path(base, "img/My%20Logo.png?raw=true"),
            Some(PathBuf::from("/docs/img/My Logo.png"))
        );
        assert_eq!(
            source_path(base, "/abs.png"),
            Some(PathBuf::from("/abs.png"))
        );
        assert_eq!(source_path(base, "https://x.org/a.png"), None);
        assert_eq!(source_path(None, "a.png"), None);
    }

    #[test]
    fn plain_text_is_one_paragraph_as_written() {
        let view = MarkdownView::plain("a *b*\n# c");
        let [Block::Paragraph(runs)] = view.markdown().blocks.as_slice() else {
            panic!("{:?}", view.markdown().blocks);
        };
        assert_eq!(runs[0].text, "a *b*\n# c");
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn code_blocks_are_coloured_as_their_language_or_the_default() {
        let mut view =
            MarkdownView::new("```json\n{\"a\": 1}\n```\n\nText.\n\n```\nfn main() {}\n```\n")
                .code_language("rust");
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while view.colouring() && std::time::Instant::now() < deadline {
            view.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        view.poll(&ctx);
        let mut codes = Vec::new();
        view.markdown
            .visit_code(&mut |code| codes.push(code.highlights.clone()));
        assert!(
            codes[0].contains(&(6..7, SyntaxRole::Number)),
            "{:?}",
            codes[0]
        );
        assert!(
            codes[1].contains(&(0..2, SyntaxRole::Keyword)),
            "{:?}",
            codes[1]
        );
    }
}
