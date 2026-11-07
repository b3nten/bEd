//! Problems at a glance: a mark along the scroll bar for each error and
//! warning, at its place in the whole text, so ones off screen are easy to
//! find (a click on the track scrolls there). Marks are kept per pixel and
//! rebuilt only when the report, the layout or the track changes.
use crate::editor::{DiagnosticSeverity, Diagnostics, layout::Geometry};
use bedtime_ui::egui;
use std::ops::Range;

/// The strip's width at the viewport's right edge, under the scroll bar.
const WIDTH: f32 = 6.0;

#[derive(Default)]
pub(crate) struct Overview {
    key: Option<Key>,
    /// Pixel offsets down the track, each with its most severe problem.
    marks: Vec<(u32, DiagnosticSeverity)>,
}
#[derive(PartialEq)]
struct Key {
    report: usize,
    wrap: Option<(usize, usize)>,
    folds: Vec<Range<usize>>,
    diff: bool,
    content: u32,
    track: u32,
}

impl Overview {
    /// Places each error and warning at its row's share of the content
    /// height, `content` being the scrolled size the rows are drawn in.
    fn marks(
        &mut self,
        report: &std::sync::Arc<Diagnostics>,
        geometry: &Geometry,
        folds: &[Range<usize>],
        content: f32,
        track: f32,
    ) -> &[(u32, DiagnosticSeverity)] {
        let key = Key {
            report: std::sync::Arc::as_ptr(report) as usize,
            wrap: geometry.wrap.map(|w| (w.width, w.rows())),
            folds: folds.to_vec(),
            diff: geometry.diff.is_some(),
            content: content.to_bits(),
            track: track.to_bits(),
        };
        if self.key.as_ref() != Some(&key) {
            self.marks.clear();
            if content > 0.0 && track >= 1.0 {
                for diagnostic in report.items() {
                    if !matches!(
                        diagnostic.severity,
                        DiagnosticSeverity::Error | DiagnosticSeverity::Warning
                    ) {
                        continue;
                    }
                    let row = geometry.visual_row(diagnostic.range.start.min(geometry.text.len()));
                    let y = 6.0 + geometry.row_y(row) * geometry.metrics.height;
                    let pixel = ((y / content).clamp(0.0, 1.0) * (track - 1.0)) as u32;
                    // Items are sorted by start, so rows only move down the
                    // track: a repeat pixel is always the last one.
                    match self.marks.last_mut() {
                        Some((last, severity)) if *last == pixel => {
                            if diagnostic.severity == DiagnosticSeverity::Error {
                                *severity = DiagnosticSeverity::Error;
                            }
                        }
                        _ => self.marks.push((pixel, diagnostic.severity)),
                    }
                }
            }
            self.key = Some(key);
        }
        &self.marks
    }

    /// Paints the marks down the right edge of `viewport` (the scroll area's
    /// visible rect), over everything the scroll area drew.
    pub(crate) fn paint(
        &mut self,
        ui: &egui::Ui,
        viewport: egui::Rect,
        report: &std::sync::Arc<Diagnostics>,
        geometry: &Geometry,
        folds: &[Range<usize>],
        content: f32,
    ) {
        let marks = self.marks(report, geometry, folds, content, viewport.height());
        if marks.is_empty() {
            return;
        }
        let strip = egui::Rect::from_min_max(
            egui::pos2(viewport.right() - WIDTH, viewport.top()),
            viewport.max,
        );
        let painter = ui.painter().with_clip_rect(strip.intersect(ui.clip_rect()));
        let fade = if report.verified { 1.0 } else { 0.6 };
        let height = (geometry.metrics.height * 0.2).clamp(2.0, 3.0);
        for &(pixel, severity) in marks {
            let color = match severity {
                DiagnosticSeverity::Error => ui.visuals().error_fg_color,
                _ => ui.visuals().warn_fg_color,
            }
            .gamma_multiply(fade);
            let top = strip.top() + pixel as f32;
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(strip.left(), top), egui::vec2(WIDTH, height)),
                1.0,
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Diagnostic, TextBuffer, folding::FoldMap, layout::Metrics};

    fn report(starts: &[(usize, DiagnosticSeverity)]) -> std::sync::Arc<Diagnostics> {
        std::sync::Arc::new(Diagnostics::new(
            0,
            true,
            starts
                .iter()
                .map(|&(start, severity)| Diagnostic {
                    range: start..start + 1,
                    severity,
                    message: "problem".into(),
                })
                .collect(),
        ))
    }

    #[test]
    fn marks_follow_rows_keep_the_worst_per_pixel_and_skip_hints() {
        let text = TextBuffer::from("a\n".repeat(100).as_str());
        let metrics = Metrics {
            font: egui::FontId::monospace(10.0),
            cell: 6.0,
            height: 10.0,
        };
        let folds = FoldMap::default();
        let geometry = Geometry {
            text: &text,
            metrics: &metrics,
            wrap: None,
            folds: &folds,
            diff: None,
        };
        // Row 50 starts 506 points down; the track spans pixels 0..=track - 1.
        let content = 1006.0;
        let line = |n: usize| text.line_start(n);
        let report = report(&[
            (line(0), DiagnosticSeverity::Warning),
            (line(0), DiagnosticSeverity::Error),
            (line(10), DiagnosticSeverity::Hint),
            (line(50), DiagnosticSeverity::Warning),
            (line(99), DiagnosticSeverity::Information),
        ]);
        let mut overview = Overview::default();
        let marks = overview
            .marks(&report, &geometry, &[], content, 101.0)
            .to_vec();
        assert_eq!(
            marks,
            [
                (0, DiagnosticSeverity::Error),
                (50, DiagnosticSeverity::Warning)
            ]
        );
        // A different track height is a different layout.
        let marks = overview
            .marks(&report, &geometry, &[], content, 1007.0)
            .to_vec();
        assert!((505..=506).contains(&marks[1].0), "{marks:?}");
    }
}
