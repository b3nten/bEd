//! Immutable byte-range contributions. Preparation/indexing belongs to the
//! producer; drawing only visits ranges intersecting the visible fragments.
use std::{ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub severity: DiagnosticSeverity,
    pub message: Arc<str>,
}
#[derive(Debug)]
pub struct Diagnostics {
    pub revision: u64,
    /// False for observations without a server-supplied version. They are
    /// best-effort presentation, never an edit or navigation precondition.
    pub verified: bool,
    items: Vec<Diagnostic>,
    prefix_end: Vec<usize>,
    counts: [usize; 4],
}
impl Diagnostics {
    pub fn new(revision: u64, verified: bool, mut items: Vec<Diagnostic>) -> Self {
        let mut counts = [0; 4];
        for item in &items {
            counts[match item.severity {
                DiagnosticSeverity::Error => 0,
                DiagnosticSeverity::Warning => 1,
                DiagnosticSeverity::Information => 2,
                DiagnosticSeverity::Hint => 3,
            }] += 1;
        }
        items.sort_by_key(|d| d.range.start);
        let mut end = 0;
        let prefix_end = items
            .iter()
            .map(|d| {
                end = end.max(d.range.end.max(d.range.start.saturating_add(1)));
                end
            })
            .collect();
        Self {
            revision,
            verified,
            items,
            prefix_end,
            counts,
        }
    }
    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }
    pub fn counts(&self) -> [usize; 4] {
        self.counts
    }
    pub fn intersecting(&self, range: Range<usize>) -> impl Iterator<Item = &Diagnostic> {
        let from = self.prefix_end.partition_point(|&end| end < range.start);
        let to = self.items.partition_point(|d| d.range.start <= range.end);
        self.items[from.min(to)..to]
            .iter()
            .filter(move |d| d.range.end >= range.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_lookup_keeps_long_overlaps_and_zero_width_markers() {
        let report = Diagnostics::new(
            7,
            true,
            [0..100, 5..6, 20..20, 80..90]
                .into_iter()
                .map(|range| Diagnostic {
                    range,
                    severity: DiagnosticSeverity::Error,
                    message: "problem".into(),
                })
                .collect(),
        );
        assert_eq!(
            report
                .intersecting(19..21)
                .map(|d| d.range.clone())
                .collect::<Vec<_>>(),
            [0..100, 20..20]
        );
        assert_eq!(report.intersecting(101..110).count(), 0);
    }
}
