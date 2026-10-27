//! Optional Tree-sitter contributions. Hosts own sessions and explicitly attach
//! matching observations; editor views never create their own parser workers.
use crate::*;
use bedtime_treesitter::{AnalysisSession, Observation};
use std::sync::Arc;

/// Submit a cheap immutable root. Positional journal gaps force a fresh parse.
pub fn update_analysis(
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

/// Syntax colors affect glyph color only; font and layout metrics stay fixed.
#[derive(Clone)]
pub struct SyntaxPalette {
    pub comment: egui::Color32,
    pub string: egui::Color32,
    pub keyword: egui::Color32,
    pub number: egui::Color32,
    pub function: egui::Color32,
    pub r#type: egui::Color32,
    pub variable: egui::Color32,
    pub punctuation: egui::Color32,
}
impl Default for SyntaxPalette {
    fn default() -> Self {
        Self {
            comment: egui::Color32::from_rgb(133, 146, 137),
            string: egui::Color32::from_rgb(167, 192, 128),
            keyword: egui::Color32::from_rgb(230, 126, 128),
            number: egui::Color32::from_rgb(211, 153, 182),
            function: egui::Color32::from_rgb(219, 188, 127),
            r#type: egui::Color32::from_rgb(131, 192, 146),
            variable: egui::Color32::from_rgb(127, 187, 179),
            punctuation: egui::Color32::from_rgb(157, 169, 160),
        }
    }
}
impl SyntaxPalette {
    pub fn from_theme(theme: &bedtime_ui::Theme) -> Self {
        let colors = &theme.palette.terminal;
        let c = bedtime_ui::theme::color;
        Self {
            comment: c(theme.palette.muted),
            string: c(colors[2]),
            keyword: c(colors[1]),
            number: c(colors[5]),
            function: c(colors[3]),
            r#type: c(colors[6]),
            variable: c(colors[4]),
            punctuation: c(theme.palette.text),
        }
    }
    /// Resolve the shared capture palette for editor and documentation syntax.
    pub fn color(&self, capture: &str, normal: egui::Color32) -> egui::Color32 {
        match capture.split('.').next().unwrap_or(capture) {
            "comment" => self.comment,
            "string" | "character" | "text" => self.string,
            "keyword" | "operator" | "conditional" | "repeat" | "exception" | "include"
            | "preproc" => self.keyword,
            "number" | "float" | "boolean" | "constant" => self.number,
            "function" | "method" | "constructor" => self.function,
            "type" | "namespace" | "module" | "tag" | "attribute" => self.r#type,
            "variable" | "property" | "field" | "parameter" => self.variable,
            "punctuation" => self.punctuation,
            _ => normal,
        }
    }
}
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
        if session.status() == &bedtime_treesitter::AnalysisStatus::Pending {
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
) -> Vec<bedtime_treesitter::Highlight> {
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
                || bedtime_treesitter::Highlight {
                    range,
                    capture: span.capture.clone(),
                },
            )
        })
        .collect()
}
