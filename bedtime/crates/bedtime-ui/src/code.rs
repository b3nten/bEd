//! A read-only code view: text coloured by syntax role, selectable and
//! copyable, wrapped or not, with find (Cmd/Ctrl+F while the pointer is over
//! it). The caller owns a `CodeView` and draws it with `show`.
//!
//! Colours come from spans the caller gives (`with_spans`), or, with the
//! `syntax` feature, from `bedtime-syntax` analysing the text on a thread of
//! its own (`language`). Without either it's plain text.
use crate::{BuiltinIcon, UiContext, egui, theme::SyntaxPalette, theme::SyntaxRole};
use egui::text::{LayoutJob, TextFormat};
use std::{ops::Range, sync::Arc};

/// At most this many matches are found (and counted).
const MAX_MATCHES: usize = 10_000;

/// A code view's text, colours, wrapping and find state.
pub struct CodeView {
    text: Arc<String>,
    spans: Arc<[(Range<usize>, SyntaxRole)]>,
    /// Bumped when `spans` change, to lay the text out again.
    spans_revision: u64,
    #[cfg(feature = "syntax")]
    analysis: Option<bedtime_syntax::AnalysisSession>,
    /// Lines longer than the view wrap; otherwise it scrolls sideways.
    pub wrap: bool,
    find: Option<Find>,
    layout: Option<(LayoutKey, Arc<egui::Galley>)>,
}

struct Find {
    query: String,
    /// `query`'s matches, as byte ranges, for `matched`.
    matches: Vec<Range<usize>>,
    matched: String,
    current: usize,
    /// Scroll to (and select) the current match when next drawn.
    reveal: bool,
    focus: bool,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    width: u32,
    wrap: bool,
    font: egui::FontId,
    palette: SyntaxPalette,
    spans_revision: u64,
    matched: Option<(String, usize)>,
}

impl CodeView {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Arc::new(text.into()),
            spans: Arc::from([]),
            spans_revision: 0,
            #[cfg(feature = "syntax")]
            analysis: None,
            wrap: true,
            find: None,
            layout: None,
        }
    }

    /// Colour it as `language` (a name or file name: `json`, `page.html`).
    /// Without the `syntax` feature, or for a language it doesn't know, the
    /// text stays plain.
    pub fn language(self, language: &str) -> Self {
        #[cfg(feature = "syntax")]
        {
            let mut this = self;
            let mut session = bedtime_syntax::AnalysisSession::new();
            session.request(this.text.clone(), 0, language, None);
            this.analysis = Some(session);
            this
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = language;
            self
        }
    }

    /// Colour it with `spans`: byte ranges of the text, sorted and disjoint.
    pub fn with_spans(mut self, spans: impl Into<Arc<[(Range<usize>, SyntaxRole)]>>) -> Self {
        self.set_spans(spans);
        self
    }

    pub fn set_spans(&mut self, spans: impl Into<Arc<[(Range<usize>, SyntaxRole)]>>) {
        self.spans = spans.into();
        self.spans_revision += 1;
        #[cfg(feature = "syntax")]
        {
            self.analysis = None;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether colours are still on their way.
    pub fn colouring(&self) -> bool {
        #[cfg(feature = "syntax")]
        if let Some(session) = &self.analysis {
            return session.status() == &bedtime_syntax::AnalysisStatus::Pending;
        }
        false
    }

    /// Open find, with `query` if given.
    pub fn find(&mut self, query: Option<&str>) {
        let find = self.find.get_or_insert_with(|| Find {
            query: String::new(),
            matches: Vec::new(),
            matched: String::new(),
            current: 0,
            reveal: false,
            focus: false,
        });
        if let Some(query) = query {
            find.query = query.to_owned();
        }
        find.focus = true;
    }

    /// Byte ranges of `query`'s matches, if find is open.
    pub fn matches(&self) -> Option<&[Range<usize>]> {
        self.find.as_ref().map(|find| find.matches.as_slice())
    }

    /// Draw it to fill the space left. Returns the text's response.
    pub fn show(&mut self, ui: &mut egui::Ui, context: &UiContext<'_>) -> egui::Response {
        self.poll(ui.ctx());
        let hovered = ui.rect_contains_pointer(ui.max_rect());
        if hovered && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.find(None);
        }
        if let Some(find) = &mut self.find {
            if !find_bar(ui, context, find) {
                self.find = None;
            } else if find.matched != find.query {
                find.matches = find_all(&self.text, &find.query);
                find.matched = find.query.clone();
                find.current = 0;
                find.reveal = true;
            }
        }

        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let palette = SyntaxPalette::from_theme(context.theme);
        let wrap = self.wrap;
        let text = self.text.clone();
        let spans = self.spans.clone();
        let spans_revision = self.spans_revision;
        let find = self.find.as_ref();
        let matched = find.map(|f| (f.matched.clone(), f.current));
        let matches = find.map_or_else(Vec::new, |f| f.matches.clone());
        let current = find.map(|f| f.current);
        let layout = &mut self.layout;
        let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, width: f32| {
            let key = LayoutKey {
                width: if wrap { width as u32 } else { u32::MAX },
                wrap,
                font: font.clone(),
                palette: palette.clone(),
                spans_revision,
                matched: matched.clone(),
            };
            if let Some((cached, galley)) = layout.as_ref()
                && *cached == key
            {
                return galley.clone();
            }
            let mut job = job(&text, &spans, &matches, current, &font, &palette, ui);
            job.wrap.max_width = if wrap { width } else { f32::INFINITY };
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            *layout = Some((key, galley.clone()));
            galley
        };

        let scroll = if wrap {
            egui::ScrollArea::vertical()
        } else {
            egui::ScrollArea::both()
        };
        let mut reveal = None;
        let output = scroll
            .id_salt("code-view")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut shown = self.text.as_str();
                let mut output = egui::TextEdit::multiline(&mut shown)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui);
                if let Some(find) = &mut self.find
                    && std::mem::take(&mut find.reveal)
                    && let Some(range) = find.matches.get(find.current)
                {
                    let start = self.text[..range.start].chars().count();
                    let end = start + self.text[range.clone()].chars().count();
                    let cursor = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(start),
                        egui::text::CCursor::new(end),
                    );
                    output.state.cursor.set_char_range(Some(cursor));
                    output.state.clone().store(ui.ctx(), output.response.id);
                    let rect = output.galley.pos_from_cursor(cursor.primary);
                    reveal = Some(rect.translate(output.galley_pos.to_vec2()));
                }
                if let Some(rect) = reveal {
                    ui.scroll_to_rect(rect.expand(24.0), Some(egui::Align::Center));
                }
                output.response.response
            });
        output.inner
    }

    /// Take colours that arrived; repaint soon while they're on their way.
    fn poll(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "syntax")]
        if let Some(session) = &mut self.analysis {
            session.poll();
            match session.status() {
                bedtime_syntax::AnalysisStatus::Pending => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }
                bedtime_syntax::AnalysisStatus::Ready => {
                    if let Some(observation) = session.observation() {
                        self.spans = observation
                            .highlights
                            .iter()
                            .filter_map(|h| {
                                Some((h.range.clone(), SyntaxRole::of_capture(&h.capture)?))
                            })
                            .collect();
                        self.spans_revision += 1;
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

/// The find row: query, count, previous, next and close. Returns false when
/// closed.
fn find_bar(ui: &mut egui::Ui, context: &UiContext<'_>, find: &mut Find) -> bool {
    let mut open = true;
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut find.query)
                .hint_text("Find")
                .desired_width(180.0),
        );
        if std::mem::take(&mut find.focus) {
            edit.request_focus();
        }
        let count = find.matches.len();
        let step = |find: &mut Find, by: isize| {
            if count > 0 {
                find.current = (find.current as isize + by).rem_euclid(count as isize) as usize;
                find.reveal = true;
            }
        };
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let back = ui.input(|i| i.modifiers.shift);
            step(find, if back { -1 } else { 1 });
            edit.request_focus();
        }
        if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        let label = match count {
            _ if find.query.is_empty() => String::new(),
            0 => "No results".to_owned(),
            MAX_MATCHES => format!("{} of {MAX_MATCHES}+", find.current + 1),
            n => format!("{} of {n}", find.current + 1),
        };
        ui.weak(label);
        let mut frame = context.frame(ui);
        if frame
            .icon_button(BuiltinIcon::ChevronLeft)
            .on_hover_text("Previous (Shift+Enter)")
            .clicked()
        {
            step(find, -1);
        }
        if frame
            .icon_button(BuiltinIcon::ChevronRight)
            .on_hover_text("Next (Enter)")
            .clicked()
        {
            step(find, 1);
        }
        if frame
            .icon_button(BuiltinIcon::Close)
            .on_hover_text("Close (Escape)")
            .clicked()
        {
            open = false;
        }
    });
    open
}

/// Where `query` is in `text`, ignoring ASCII case.
fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let needle = query.as_bytes();
    let haystack = text.as_bytes();
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= haystack.len() && found.len() < MAX_MATCHES {
        if haystack[at..at + needle.len()].eq_ignore_ascii_case(needle)
            && text.is_char_boundary(at)
            && text.is_char_boundary(at + needle.len())
        {
            found.push(at..at + needle.len());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

/// The text as sections: coloured by `spans`, with `matches` behind (the
/// `current` one stronger).
fn job(
    text: &str,
    spans: &[(Range<usize>, SyntaxRole)],
    matches: &[Range<usize>],
    current: Option<usize>,
    font: &egui::FontId,
    palette: &SyntaxPalette,
    ui: &egui::Ui,
) -> LayoutJob {
    let selection = ui.visuals().selection.bg_fill;
    let found = selection.gamma_multiply(0.45);
    let mut job = LayoutJob::default();
    for (range, role, matched) in segments(text, spans, matches) {
        let background = match matched {
            Some(index) if Some(index) == current => selection,
            Some(_) => found,
            None => egui::Color32::TRANSPARENT,
        };
        job.append(
            &text[range],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: role.map_or(palette.text, |role| palette.role(role)),
                background,
                ..Default::default()
            },
        );
    }
    job
}

/// `text` cut where a span or match starts or ends: each piece with its
/// role and the index of the match it's in. Spans and matches are each sorted
/// and disjoint; cuts that would split a character are skipped.
fn segments(
    text: &str,
    spans: &[(Range<usize>, SyntaxRole)],
    matches: &[Range<usize>],
) -> Vec<(Range<usize>, Option<SyntaxRole>, Option<usize>)> {
    let mut cuts: Vec<usize> = spans
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain(matches.iter().flat_map(|r| [r.start, r.end]))
        .chain([0, text.len()])
        .filter(|&at| text.is_char_boundary(at))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let (mut s, mut m) = (0, 0);
    let mut out = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        let range = pair[0]..pair[1];
        while s < spans.len() && spans[s].0.end <= range.start {
            s += 1;
        }
        while m < matches.len() && matches[m].end <= range.start {
            m += 1;
        }
        let role = spans
            .get(s)
            .filter(|(r, _)| r.start <= range.start)
            .map(|(_, role)| *role);
        let matched = matches.get(m).filter(|r| r.start <= range.start).map(|_| m);
        out.push((range, role, matched));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ignores_ascii_case_and_does_not_overlap() {
        assert_eq!(find_all("aAaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(find_all("Content-Type", "type"), vec![8..12]);
        assert!(find_all("abc", "").is_empty());
        assert!(find_all("ab", "abc").is_empty());
        // Matches never split a character.
        assert_eq!(find_all("é e", "e"), vec![3..4]);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn segments_cut_at_spans_and_matches() {
        let spans = [(0..4, SyntaxRole::Keyword), (6..9, SyntaxRole::String)];
        let matches = [3..7];
        let got = segments("0123456789", &spans, &matches);
        assert_eq!(
            got,
            vec![
                (0..3, Some(SyntaxRole::Keyword), None),
                (3..4, Some(SyntaxRole::Keyword), Some(0)),
                (4..6, None, Some(0)),
                (6..7, Some(SyntaxRole::String), Some(0)),
                (7..9, Some(SyntaxRole::String), None),
                (9..10, None, None),
            ]
        );
        assert_eq!(segments("", &[], &[]), vec![]);
        // A span ending inside "é" (bytes 1..3) doesn't cut it.
        assert_eq!(
            segments("aéb", &[(0..2, SyntaxRole::Number)], &[]),
            vec![(0..4, Some(SyntaxRole::Number), None)]
        );
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn a_language_colours_the_text_on_its_own_thread() {
        let mut view = CodeView::new(r#"{"key": 42}"#).language("json");
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while view.colouring() && std::time::Instant::now() < deadline {
            view.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        view.poll(&ctx);
        assert!(!view.colouring());
        assert!(
            view.spans.contains(&(8..10, SyntaxRole::Number)),
            "{:?}",
            view.spans
        );
    }
}
