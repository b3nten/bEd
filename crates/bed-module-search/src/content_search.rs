//! Project search presentation around the GUI-free search worker.
use bed_files::content_search::{ContentMatch, ContentSearch as Search};
use dear_imgui_rs::{ListClipper, Ui};
use std::ops::{Deref, DerefMut};

pub struct ContentSearch {
    search: Search,
    should_focus: bool,
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
            search: Search::new_lazy(),
            should_focus: false,
        }
    }
    pub fn open(&mut self) {
        self.search.open();
        self.should_focus = true;
    }
    fn draw_status(&mut self, ui: &Ui) {
        let status = if self.searching && self.progress.discovering {
            format!(
                "Discovering files… {} found",
                self.progress.discovered_files
            )
        } else if self.searching {
            format!(
                "Searching… {}/{} files · {} matches",
                self.progress.scanned_files,
                self.progress.discovered_files,
                self.results.len()
            )
        } else if self.limit_reached {
            format!("Stopped at {} matches (result limit)", self.results.len())
        } else if self.canceled {
            format!(
                "Canceled · {} matches in {} files",
                self.results.len(),
                self.progress.scanned_files
            )
        } else {
            format!(
                "{} matches in {} files",
                self.results.len(),
                self.progress.scanned_files
            )
        };
        ui.text(status);
        if self.searching {
            ui.same_line();
            if ui.button("Cancel") {
                self.cancel();
            }
        }
        if self.progress.ignored_paths != 0 {
            ui.text_disabled(format!(
                "{} ignored or Git metadata paths skipped",
                self.progress.ignored_paths
            ));
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContentSearchAction {
    #[default]
    None,
    Close,
    Open(ContentMatch),
}

impl ContentSearch {
    /// Search controls and results inside a caller-owned docking panel.
    pub fn draw_body(&mut self, ui: &Ui, project_root: &str) -> ContentSearchAction {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.poll();
        let fs = ui.current_font_size();
        let mut action = ContentSearchAction::None;
        if self.should_focus {
            ui.set_keyboard_focus_here();
            self.should_focus = false;
        }
        ui.set_next_item_width((ui.content_region_avail()[0] - fs * 5.0).max(0.0));
        let enter = ui
            .input_text("##ProjectQuery", &mut self.query)
            .hint("Search text")
            .enter_returns_true(true)
            .build();
        ui.same_line();
        if ui.button("Find") || enter {
            let query = self.query.clone();
            self.search
                .start(project_root, &query, self.search.case_sensitive);
        }
        ui.checkbox("Match case", &mut self.case_sensitive);
        ui.same_line();
        ui.checkbox("Include ignored files", &mut self.include_ignored);
        self.draw_status(ui);
        if let Some(error) = &self.error {
            ui.text_wrapped(error);
        }
        if self.skipped_files != 0 {
            ui.text(format!(
                "{} binary or unreadable files skipped",
                self.skipped_files
            ));
        }
        ui.separator();
        ui.child_window("ProjectContentResults")
            .size([0.0, -ui.frame_height_with_spacing()])
            .build(ui, || {
                for index in ListClipper::new(self.results.len()).begin(ui).iter() {
                    let found = self.results[index].clone();
                    let _id = ui.push_id(index as i32);
                    let label = format!(
                        "{}:{}:{}  {}",
                        found.file.relative_path,
                        found.source_row + 1,
                        found.column + 1,
                        result_preview(&found.line, found.column as usize)
                    );
                    if ui
                        .selectable_config(&label)
                        .selected(index == self.selected_index)
                        .build()
                    {
                        self.selected_index = index;
                        action = ContentSearchAction::Open(found.clone());
                    }
                }
            });
        action
    }
}

fn result_preview(line: &[u8], column: usize) -> String {
    let start = column.saturating_sub(48).min(line.len());
    let end = column.saturating_add(160).min(line.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        String::from_utf8_lossy(&line[start..end]),
        if end < line.len() { "…" } else { "" },
    )
}
