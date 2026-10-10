//! Project search controls, live-query scheduling, and flat highlighted results.
use bed_document_session::{DocumentId, EditorSession};
use bed_editing::{
    editor_state::DocumentKind,
    text_search::{CompiledSearch, SearchOptions},
};
use bed_files::content_search::{ContentMatch, ContentSearch as Search, SearchBuffer};
use dear_imgui_rs::{FocusedFlags, Key, ListClipper, StyleColor, Ui};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct ContentSearch {
    /// Captured when the panel opens; terminal directory changes do not retarget it.
    pub root: String,
    search: Search,
    should_focus: bool,
    pub replacement: String,
    pub show_replace: bool,
    pub show_filters: bool,
    observed: Option<(String, String, SearchOptions)>,
    pending: Option<Instant>,
    pub(crate) stale: bool,
    pub(crate) buffer_revisions: HashMap<String, (DocumentId, (u64, u64))>,
    pub(crate) message: Option<String>,
    validation_error: Option<String>,
    scroll_to_selected: bool,
    pub(crate) advance_to: Option<(String, i32, usize)>,
}
impl Default for ContentSearch {
    fn default() -> Self {
        Self::new_lazy()
    }
}
impl Deref for ContentSearch {
    type Target = Search;
    fn deref(&self) -> &Search {
        &self.search
    }
}
impl DerefMut for ContentSearch {
    fn deref_mut(&mut self) -> &mut Search {
        &mut self.search
    }
}
impl ContentSearch {
    pub fn new_lazy() -> Self {
        Self {
            root: String::new(),
            search: Search::new_lazy(),
            should_focus: false,
            replacement: String::new(),
            show_replace: false,
            show_filters: false,
            observed: None,
            pending: None,
            stale: true,
            buffer_revisions: HashMap::new(),
            message: None,
            validation_error: None,
            scroll_to_selected: false,
            advance_to: None,
        }
    }
    pub fn open(&mut self) {
        self.search.open();
        self.should_focus = true;
    }
    pub fn cancel(&mut self) {
        self.pending = None;
        self.search.cancel();
        self.search.canceled = true;
        self.stale = true;
    }
    pub fn needs_tick(&self) -> bool {
        self.pending.is_some() || self.searching
    }
    pub fn invalidate(&mut self) {
        self.search.cancel();
        self.stale = true;
        self.pending = Some(Instant::now() + Duration::from_millis(200));
    }
    fn observe(&mut self, root: &str) {
        let signature = (root.to_owned(), self.query.clone(), self.options());
        if self.observed.as_ref() != Some(&signature) {
            self.validation_error = CompiledSearch::new(self.query.as_bytes(), &signature.2).err();
            self.observed = Some(signature);
            self.invalidate();
            if self.validation_error.is_some() {
                self.pending = None;
                self.search.results.clear();
                return;
            }
            // Clearing a query immediately removes obsolete matches.
            if self.query.is_empty() {
                self.pending = Some(Instant::now());
            }
        }
    }
    pub fn tick(&mut self, root: &str, documents: &EditorSession) -> bool {
        self.observe(root);
        if self
            .pending
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.pending = None;
            self.buffer_revisions.clear();
            let mut buffers = Vec::new();
            let workspace_root = if documents.is_remote() {
                Path::new(root).to_path_buf()
            } else {
                std::fs::canonicalize(root).unwrap_or_else(|_| Path::new(root).to_path_buf())
            };
            for document in documents.document_ids() {
                if documents.is_snapshot_document(document) {
                    continue;
                }
                let eligible = documents
                    .with_document(document, |state| {
                        state.kind == DocumentKind::Text
                            && !state.path.is_empty()
                            && Path::new(&state.path).starts_with(&workspace_root)
                    })
                    .expect("Search snapshot document must still exist");
                if !eligible {
                    continue;
                }
                let snapshot = documents
                    .snapshot(document)
                    .expect("Search snapshot document must still exist");
                let revision = documents
                    .document_revision(document)
                    .expect("Search snapshot document must still exist");
                self.buffer_revisions
                    .insert(snapshot.path.clone(), (document, revision));
                buffers.push(SearchBuffer {
                    path: snapshot.path,
                    bytes: Some(Arc::from(snapshot.bytes)),
                });
            }
            let query = self.query.clone();
            self.search.start_with_buffers(root, &query, buffers);
            self.stale = false;
        }
        let changed = self.search.poll();
        if !self.searching
            && changed
            && let Some((path, row, column)) = self.advance_to.take()
        {
            self.search.selected_index = self
                .results
                .iter()
                .position(|found| {
                    (&found.file.relative_path, found.row, found.range.start)
                        >= (&path, row, column)
                })
                .unwrap_or(0);
            self.scroll_to_selected = true;
        }
        changed || self.pending.is_some()
    }
    pub fn can_replace(&self) -> bool {
        !self.stale
            && self.pending.is_none()
            && !self.searching
            && !self.canceled
            && !self.limit_reached
            && self.error.is_none()
            && !self.results.is_empty()
    }
    fn step(&mut self, direction: isize) -> ContentSearchAction {
        if self.results.is_empty() || self.stale {
            return ContentSearchAction::None;
        }
        self.search.selected_index = (self.selected_index as isize + direction)
            .rem_euclid(self.results.len() as isize) as usize;
        self.scroll_to_selected = true;
        ContentSearchAction::Open(self.results[self.selected_index].clone())
    }
    pub fn draw_body(
        &mut self,
        ui: &Ui,
        project_root: &str,
        replacing: bool,
        icons: Option<&bed_ui::icons::Icons>,
    ) -> ContentSearchAction {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.observe(project_root);
        let mut action = ContentSearchAction::None;
        if self.should_focus {
            ui.set_keyboard_focus_here();
            self.should_focus = false;
        }
        let fs = ui.current_font_size();
        let available = ui.content_region_avail()[0];
        ui.set_next_item_width(if available > fs * 45.0 {
            (available - fs * 36.0).max(fs * 10.0)
        } else {
            available
        });
        let before = self.query.clone();
        let enter = ui
            .input_text("##ProjectQuery", &mut self.search.query)
            .hint("Search all files…")
            .enter_returns_true(true)
            .build();
        same_line_if_room(ui, fs * 5.0);
        if toggle(ui, "Aa", "Match case", &mut self.search.case_sensitive) {
            self.observe(project_root);
        }
        same_line_if_room(ui, fs * 5.0);
        if toggle(ui, "ab", "Match whole words", &mut self.search.whole_words) {
            self.observe(project_root);
        }
        same_line_if_room(ui, fs * 5.0);
        if toggle(ui, ".*", "Use regular expressions", &mut self.search.regex) {
            self.observe(project_root);
        }
        same_line_if_room(ui, fs * 7.0);
        toggle(
            ui,
            "Replace",
            "Show replacement controls",
            &mut self.show_replace,
        );
        same_line_if_room(ui, fs * 7.0);
        let filters_label =
            if self.include.is_empty() && self.exclude.is_empty() && !self.include_ignored {
                "Filters"
            } else {
                "Filters*"
            };
        toggle(
            ui,
            filters_label,
            "Show include/exclude paths and ignored-file options",
            &mut self.show_filters,
        );
        same_line_if_room(ui, fs * 4.0);
        let disabled = ui.begin_disabled_with_cond(self.results.is_empty() || self.stale);
        if bed_ui::icons::icon_button(ui, icons, "chevron-left", "Previous", "<", [0.0; 2]) {
            action = self.step(-1);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text("Previous match (Shift+F3)");
        }
        same_line_if_room(ui, fs * 4.0);
        if bed_ui::icons::icon_button(ui, icons, "chevron-right", "Next", ">", [0.0; 2]) {
            action = self.step(1);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text("Next match (F3)");
        }
        drop(disabled);
        let counter = format!(
            "{} / {}",
            if self.results.is_empty() {
                0
            } else {
                self.selected_index + 1
            },
            self.results.len()
        );
        same_line_if_room(ui, ui.calc_text_size(&counter)[0]);
        ui.text(counter);
        if before != self.query {
            self.observe(project_root);
        }
        if enter {
            self.pending = Some(Instant::now());
            self.stale = true;
        }
        if ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS)
            && ui.is_key_pressed(Key::F3)
        {
            action = self.step(if ui.io().key_shift() { -1 } else { 1 });
        }
        if self.show_replace {
            ui.set_next_item_width(ui.content_region_avail()[0]);
            ui.input_text("##ProjectReplacement", &mut self.replacement)
                .hint("Replace in project…")
                .build();
            if ui.is_item_hovered() {
                ui.tooltip_text(if self.regex { "Regex captures: $1, ${name}; $$ inserts a dollar sign. Empty text deletes matches." }
                    else { "Replacement is literal. Empty text deletes matches." });
            }
            let disabled = ui.begin_disabled_with_cond(replacing || !self.can_replace());
            if ui.button("Replace next") {
                action = ContentSearchAction::ReplaceNext;
            }
            same_line_if_room(ui, fs * 14.0);
            if ui.button("Replace all") {
                action = ContentSearchAction::ReplaceAll;
            }
            drop(disabled);
        }
        if self.show_filters {
            ui.set_next_item_width(ui.content_region_avail()[0]);
            ui.input_text("##ProjectInclude", &mut self.search.include)
                .hint("Include: src/**/*.rs, *.toml")
                .build();
            ui.set_next_item_width(ui.content_region_avail()[0]);
            ui.input_text("##ProjectExclude", &mut self.search.exclude)
                .hint("Exclude: vendor/, *.lock")
                .build();
            ui.checkbox("Include ignored files", &mut self.search.include_ignored);
            self.observe(project_root);
        }
        let status = if self.query.is_empty() {
            "Start typing to search project files".into()
        } else if self.pending.is_some() {
            "Waiting to search…".into()
        } else if self.searching {
            format!(
                "Searching… {}/{} files · {} matches",
                self.progress.scanned_files,
                self.progress.discovered_files,
                self.results.len()
            )
        } else if self.limit_reached {
            format!(
                "{} matches · result limit reached; narrow your search to replace",
                self.results.len()
            )
        } else if self.canceled {
            format!("Canceled · {} matches", self.results.len())
        } else if self.results.is_empty() && self.error.is_none() {
            "No matches".into()
        } else {
            format!(
                "{} matches in {} matching files",
                self.results.len(),
                self.results
                    .iter()
                    .map(|found| &found.file.full_path)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            )
        };
        ui.text_disabled(status);
        if self.needs_tick() {
            same_line_if_room(ui, fs * 8.0);
            if ui.small_button("Cancel") {
                self.cancel();
            }
        }
        if let Some(error) = &self.validation_error {
            ui.text_wrapped(error);
        } else if let Some(error) = &self.error {
            ui.text_wrapped(error);
        }
        if let Some(message) = &self.message {
            if message.contains('\n') {
                ui.child_window("ReplacementReport")
                    .size([
                        0.0,
                        (ui.text_line_height_with_spacing() * 6.0)
                            .min(ui.content_region_avail()[1].max(0.0) * 0.4),
                    ])
                    .build(ui, || ui.text_wrapped(message));
            } else {
                ui.text_wrapped(message);
            }
        }
        if self.skipped_files != 0 {
            ui.text_disabled(format!(
                "{} binary or unreadable files skipped",
                self.skipped_files
            ));
        }
        if self.progress.ignored_paths != 0 {
            ui.text_disabled(format!(
                "{} ignored or Git metadata paths skipped",
                self.progress.ignored_paths
            ));
        }
        ui.separator();
        ui.child_window("ProjectContentResults")
            .size([0.0, 0.0])
            .build(ui, || {
                let mut clipper = ListClipper::new(self.results.len()).begin(ui);
                if self.scroll_to_selected && !self.results.is_empty() {
                    clipper.include_item_by_index(self.selected_index);
                }
                for index in clipper.iter() {
                    let found = &self.search.results[index];
                    let _id = ui.push_id(index as i32);
                    let pos = ui.cursor_screen_pos();
                    let clicked = ui
                        .selectable_config("##Match")
                        .selected(index == self.selected_index)
                        .size([0.0, ui.text_line_height()])
                        .build();
                    draw_result(ui, pos, found);
                    if self.scroll_to_selected && index == self.selected_index {
                        ui.set_scroll_here_y(0.5);
                    }
                    if clicked {
                        action = ContentSearchAction::Open(found.clone());
                        self.search.selected_index = index;
                    }
                }
                self.scroll_to_selected = false;
            });
        action
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContentSearchAction {
    #[default]
    None,
    Close,
    Open(ContentMatch),
    ReplaceNext,
    ReplaceAll,
}
fn same_line_if_room(ui: &Ui, width: f32) {
    let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
    if ui.item_rect_max()[0] + ui.clone_style().item_spacing()[0] + width <= right {
        ui.same_line();
    }
}
fn toggle(ui: &Ui, label: &str, hint: &str, active: &mut bool) -> bool {
    let color = active
        .then(|| ui.push_style_color(StyleColor::Button, ui.style_color(StyleColor::ButtonActive)));
    let clicked = ui.button(label);
    drop(color);
    if ui.is_item_hovered() {
        ui.tooltip_text(hint);
    }
    if clicked {
        *active = !*active;
    }
    clicked
}
fn draw_result(ui: &Ui, pos: [f32; 2], found: &ContentMatch) {
    let start = found.range.start.saturating_sub(48);
    let end = found.range.start.saturating_add(160).min(found.line.len());
    let highlight_end = found.range.end.min(end);
    let prefix = format!(
        "{}:{}:{}  {}{}",
        found.file.relative_path,
        found.source_row + 1,
        found.column + 1,
        if start > 0 { "…" } else { "" },
        String::from_utf8_lossy(&found.line[start..found.range.start])
    );
    let matched = String::from_utf8_lossy(&found.line[found.range.start..highlight_end]);
    let suffix = format!(
        "{}{}",
        String::from_utf8_lossy(&found.line[highlight_end..end]),
        if end < found.line.len() { "…" } else { "" }
    );
    let draw = ui.get_window_draw_list();
    let color = ui.style_color(StyleColor::Text);
    draw.add_text(pos, color, &prefix);
    let x = pos[0] + ui.calc_text_size(&prefix)[0];
    let width = ui.calc_text_size(&matched)[0].max(2.0);
    draw.add_rect(
        [x, pos[1]],
        [x + width, pos[1] + ui.text_line_height()],
        ui.style_color(StyleColor::TextSelectedBg),
    )
    .filled(true)
    .build();
    draw.add_text([x, pos[1]], color, &matched);
    draw.add_text([x + ui.calc_text_size(&matched)[0], pos[1]], color, &suffix);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    #[test]
    fn query_changes_debounce_cancel_and_invalid_patterns_disable_replacement() {
        let temp = TempDir::new();
        temp.write("a", b"old new");
        let session = EditorSession::new();
        let mut search = ContentSearch::new_lazy();
        search.query = "old".into();
        search.tick(temp.root().to_str().unwrap(), &session);
        assert!(search.needs_tick());
        assert!(!search.searching);
        search.cancel();
        search.tick(temp.root().to_str().unwrap(), &session);
        assert!(!search.needs_tick());
        search.query = "[".into();
        search.regex = true;
        search.tick(temp.root().to_str().unwrap(), &session);
        assert!(search.validation_error.is_some());
        assert!(!search.can_replace());
        assert!(!search.needs_tick());
        search.query.clear();
        search.tick(temp.root().to_str().unwrap(), &session);
        assert!(search.results.is_empty());
        assert!(!search.needs_tick());
    }
    #[test]
    fn toolbar_wraps_and_full_panel_draws_in_narrow_and_wide_windows() {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut search = ContentSearch::new_lazy();
        search.show_replace = true;
        search.show_filters = true;
        for width in [160.0, 240.0, 900.0] {
            context.prepare_frame(FramePrepareOptions::new([1000.0, 900.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Search layout")
                .position([0.0; 2], Condition::Always)
                .size([width, 800.0], Condition::Always)
                .build(|| {
                    let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
                    let mut active = false;
                    for label in ["Aa", "ab", ".*", "Replace", "Filters*"] {
                        same_line_if_room(
                            ui,
                            ui.calc_text_size(label)[0] + ui.clone_style().frame_padding()[0] * 2.0,
                        );
                        toggle(ui, label, label, &mut active);
                        assert!(
                            ui.item_rect_max()[0] <= right + 0.1,
                            "{label} clipped at {width}"
                        );
                    }
                    ui.new_line();
                    search.draw_body(ui, "/project", false, None);
                });
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        }
    }
}
