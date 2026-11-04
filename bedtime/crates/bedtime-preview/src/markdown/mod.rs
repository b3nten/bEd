//! Markdown Preview: the document rendered, following edits. Code blocks are
//! coloured by the document's syntax analysis (shared with its editors), so
//! the preview waits briefly for it after each edit instead of flashing
//! uncoloured code. Links open in the browser, relative files in the app,
//! and `#anchors` scroll to their heading. Images beside the file show in
//! place, at their own size or the text's width.
use bedtime_code::{
    editor::syntax::SyntaxPalette,
    syntax::{AnalysisStatus, DocumentSyntax, SyntaxViewer},
};
use bedtime_document::TextBuffer;
use bedtime_ui::{UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentReference, ManagedWork, Notification, Panel, PathOptions, ViewerFile,
    WorkbenchWorldExt, viewer_document,
};
use bevy::prelude::*;
use std::{path::PathBuf, time::Duration};

mod images;
mod parse;
use images::{Image, Images, source_path};
pub use parse::{Block, Callout, Code, Item, Markdown, Run, Style, parse, slug};

/// How long a new revision waits for its code's colours.
const WAIT_FOR_COLOURS: f64 = 0.3;

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(SyntaxViewer)]
pub struct MarkdownPanel {
    #[reflect(ignore)]
    state: State,
}

#[derive(Default)]
struct State {
    shown: Option<Shown>,
    /// When the document changed past `shown`.
    waiting_since: Option<f64>,
    /// A heading to scroll to, by anchor.
    scroll_to: Option<String>,
    images: Images,
}

struct Shown {
    revision: u64,
    markdown: Markdown,
    coloured: bool,
}

impl Panel for MarkdownPanel {
    const LABEL: &'static str = "Markdown Preview";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<TextBuffer>(ui, world, panel) else {
            return;
        };
        let Some(entity) = world
            .get::<DocumentReference>(panel)
            .map(|r| r.document().entity())
        else {
            return;
        };
        let now = ui.input(|i| i.time);
        let syntax = world.get::<DocumentSyntax>(entity);
        // Colours for `revision` are on their way.
        let colouring = |revision: u64| {
            syntax.is_some_and(|syntax| {
                syntax.at(revision).is_none() && *syntax.session.status() == AnalysisStatus::Pending
            })
        };
        let pending = colouring(document.revision);
        let observation = syntax
            .and_then(|syntax| syntax.at(document.revision))
            .cloned();
        let base = world
            .get::<ViewerFile>(panel)
            .and_then(|file| file.0.parent().map(PathBuf::from));
        let Some(mut preview) = world.get_mut::<MarkdownPanel>(panel) else {
            return;
        };
        let state = &mut preview.state;
        if state.images.poll(ui.ctx()) {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let stale = state
            .shown
            .as_ref()
            .is_none_or(|shown| shown.revision != document.revision);
        if stale {
            let since = *state.waiting_since.get_or_insert(now);
            if state.shown.is_none() || !pending || now - since >= WAIT_FOR_COLOURS {
                let text = document.model.range_text(0..document.model.len());
                state.shown = Some(Shown {
                    revision: document.revision,
                    markdown: parse(&text),
                    coloured: false,
                });
                state.waiting_since = None;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        if let Some(shown) = &mut state.shown
            && !shown.coloured
        {
            if let Some(observation) = observation.filter(|_| shown.revision == document.revision) {
                shown.markdown.color(&observation);
                shown.coloured = true;
            } else if pending {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        let mut clicked = None;
        let scroll_to = state.scroll_to.take();
        if let Some(shown) = &state.shown {
            let mut draw = Draw::new(ui, context, &mut clicked, scroll_to);
            draw.base = base;
            draw.images = Some(&mut state.images);
            draw.document(ui, &shown.markdown);
        }
        world.resource_scope(|world, mut work: Mut<ManagedWork>| {
            if let Some(mut preview) = world.get_mut::<MarkdownPanel>(panel) {
                preview.state.images.load_wanted(&mut work, panel);
            }
        });
        if let Some(link) = clicked {
            follow(world, panel, &link);
        }
    }
}

/// Open a clicked link: an anchor here, a web page outside, a file in the app.
fn follow(world: &mut World, panel: Entity, link: &str) {
    if let Some(anchor) = link.strip_prefix('#') {
        if let Some(mut preview) = world.get_mut::<MarkdownPanel>(panel) {
            preview.state.scroll_to = Some(anchor.to_lowercase());
        }
        return;
    }
    let external = link.contains("://") || link.starts_with("mailto:");
    let result = if external {
        open_externally(link)
    } else {
        let file = link.split('#').next().unwrap_or(link);
        let file = percent_decode(file);
        match world.get::<ViewerFile>(panel) {
            Some(viewer) => {
                let base = viewer.0.parent().unwrap_or(std::path::Path::new(""));
                let path = base.join(&file);
                world
                    .workbench_mut()
                    .open_path(&path, PathOptions::default())
                    .map(|_| ())
                    .map_err(|error| format!("{}: {error}", path.display()))
            }
            None => Err(format!("{file} is relative to a file this isn't")),
        }
    };
    if let Err(error) = result {
        world.workbench_mut().notify(Notification::warning(error));
    }
}

fn percent_decode(text: &str) -> String {
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

fn open_externally(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open {url}: {error}"))
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
    images: Option<&'a mut Images>,
}

impl<'a> Draw<'a> {
    fn new(
        ui: &egui::Ui,
        context: &UiContext<'_>,
        clicked: &'a mut Option<String>,
        scroll_to: Option<String>,
    ) -> Self {
        let p = &context.theme.palette;
        let c = theme::color;
        Self {
            palette: SyntaxPalette::from_theme(context.theme),
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
            rounding: context.metrics.rounding,
            clicked,
            scroll_to,
            counter: 0,
            base: None,
            images: None,
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
        let scale = [1.75, 1.45, 1.25, 1.1, 1.0, 0.95][(level.clamp(1, 6) - 1) as usize];
        if level <= 2 {
            ui.add_space(self.body.size * 0.4);
        }
        let font = bedtime_ui::fonts::monospace_bold(self.body.size * scale);
        let color = if level >= 6 { self.muted } else { self.text };
        let response = self.inline(ui, text, &font, color);
        if self.scroll_to.as_deref() == Some(anchor) {
            response.scroll_to_me(Some(egui::Align::TOP));
            self.scroll_to = None;
        }
        if level <= 2 {
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
        let Some(images) = self.images.as_deref_mut() else {
            return false;
        };
        let description: String = runs.iter().map(|run| run.text.as_str()).collect();
        let (texture, size) = match images.get(&path) {
            Some(Image::Ready { texture, size }) => (texture.id(), *size),
            Some(Image::Failed(error)) => {
                let error = format!("{}: {error}", path.display());
                self.text(ui, runs, &self.body.clone(), self.text)
                    .on_hover_text(error);
                return true;
            }
            _ => return false,
        };
        let shown = if size.x > full {
            size * (full / size.x)
        } else {
            size
        };
        let response = ui.add(
            egui::Image::new((texture, shown))
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
                bedtime_ui::fonts::monospace_bold(font.size)
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
        for (range, capture) in &code.highlights {
            if range.start < at {
                continue;
            }
            plain(&mut job, &code.text[at..range.start], self.text);
            plain(
                &mut job,
                &code.text[range.clone()],
                self.palette.color(capture, self.text),
            );
            at = range.end;
        }
        plain(&mut job, &code.text[at..], self.text);
        let frame = egui::Frame::NONE
            .fill(self.code_fill)
            .corner_radius(self.rounding)
            .inner_margin(egui::Margin::same((self.body.size * 0.8) as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
                    ui.add(egui::Label::new(job).extend().selectable(true));
                });
            });
        let rect = frame.response.rect;
        if !code.language.is_empty() {
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
                            .font(bedtime_ui::fonts::monospace_bold(size))
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
        let bold = bedtime_ui::fonts::monospace_bold(self.body.size);
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
}
