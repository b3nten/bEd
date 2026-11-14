//! Drawing analysis in the editor: colours from highlights, kept (mapped
//! through edits) while a newer analysis runs. Folds and outlines are only
//! used at their own revision.
use super::*;
use crate::syntax::{AnalysisSession, AnalysisStatus, Highlight, Observation};
use std::sync::Arc;

/// Submit a session's text for analysis (the workbench does this for
/// documents in `update_syntax`).
#[cfg(test)]
pub(crate) fn update_analysis(
    session: &mut AnalysisSession,
    document: &dyn TextSession,
    language: &str,
) -> Result<(), EditorError> {
    let snapshot = document.snapshot()?;
    let base = session.key().and_then(|key| {
        document
            .changes_since(key.revision)
            .ok()
            .flatten()
            .filter(|commits| commits.iter().all(|c| c.changes.is_some()))
            .map(|_| key.revision)
    });
    session.request(Arc::new(snapshot.text), snapshot.revision, language, base);
    session.poll();
    Ok(())
}

/// The theme's code colours (moved to bedtime-ui, where viewers share them).
pub use bedtime_ui::theme::SyntaxPalette;

impl EditorView {
    pub fn set_syntax(&mut self, observation: Option<Arc<Observation>>) {
        if let Some(observation) = &observation {
            let identity = (observation.key.source, observation.key.language_generation);
            if self.syntax_identity.is_some_and(|old| old != identity) {
                self.unfold_all();
            }
            self.syntax_identity = Some(identity);
        }
        self.syntax_highlights = observation.clone();
        self.syntax = observation;
    }
    /// Attach current analysis while preserving mapped colors during a refresh.
    /// Old observations never become current folding/navigation contributions.
    pub fn set_analysis(&mut self, session: &AnalysisSession) {
        if session.status() == &AnalysisStatus::Pending {
            self.syntax = None;
            let identity = session
                .key()
                .map(|key| (key.source, key.language_generation));
            if identity != self.syntax_identity {
                self.set_syntax(None);
                self.unfold_all();
                self.syntax_identity = identity;
            }
        } else {
            self.set_syntax(session.observation().cloned());
        }
    }

    pub(super) fn highlight_changes(
        &mut self,
        session: &dyn TextSession,
        snapshot: &TextSnapshot,
    ) -> Result<Option<Vec<TextDelta>>, EditorError> {
        let Some(observation) = &self.syntax_highlights else {
            return Ok(None);
        };
        let changes = if observation.key.revision == snapshot.revision {
            Some(Vec::new())
        } else {
            session
                .changes_since(observation.key.revision)?
                .and_then(|commits| {
                    commits
                        .into_iter()
                        .try_fold(Vec::new(), |mut changes, commit| {
                            changes.extend(commit.changes?);
                            Some(changes)
                        })
                })
        };
        let Some(mut changes) = changes else {
            // A journal gap or whole-source replacement removes positional knowledge.
            self.syntax_highlights = None;
            return Ok(None);
        };
        if let Some(composition) = &self.composition {
            changes.push(TextDelta {
                range: composition.selection.range(),
                inserted_bytes: composition.text.len(),
            });
        }
        Ok(Some(changes))
    }

    pub fn syntax(&self) -> Option<&Arc<Observation>> {
        self.syntax.as_ref()
    }
}

/// Query only the old spans covering this visible fragment, then map them forward.
/// Retained colors are presentation only; the observation's semantic ranges stay
/// revision-bound. No full-document span copying or remapping runs while typing.
pub(crate) fn mapped_highlights(
    observation: &Observation,
    changes: &[TextDelta],
    visible: Range<usize>,
) -> Vec<Highlight> {
    let mut original = visible.clone();
    for change in changes.iter().rev() {
        let end = change.range.start + change.inserted_bytes;
        let inverse = |position, right| {
            if position < change.range.start {
                position
            } else if position <= end {
                if right {
                    change.range.end
                } else {
                    change.range.start
                }
            } else {
                position - change.inserted_bytes + change.range.len()
            }
        };
        original = inverse(original.start, false)..inverse(original.end, true);
    }
    observation
        .highlights_in(original)
        .iter()
        .filter_map(|span| {
            let mut range = span.range.clone();
            for change in changes {
                range = change.map_position(range.start)..if range.end <= change.range.start {
                    range.end
                } else {
                    change.map_position(range.end)
                };
            }
            (!range.is_empty() && range.end > visible.start && range.start < visible.end).then(
                || Highlight {
                    range,
                    capture: span.capture.clone(),
                },
            )
        })
        .collect()
}
