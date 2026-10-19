use crate::*;

impl EditorView {
    pub fn open_search(&mut self, replace: bool) {
        self.search.open = true;
        self.search.replace = replace;
        self.search.focus = true;
    }
    /// Optional host toolbar; shortcuts also work when no toolbar is installed.
    pub fn editing_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Editing", |ui| {
            ui.checkbox(&mut self.settings.wrap, "Wrap lines");
            ui.checkbox(&mut self.settings.smart_indent, "Smart indentation");
            ui.label("Indentation");
            for style in [
                Indentation::Tabs,
                Indentation::TwoSpaces,
                Indentation::FourSpaces,
                Indentation::EightSpaces,
            ] {
                ui.radio_value(&mut self.settings.indentation, style, style.label());
            }
            ui.separator();
            if ui.button("Find…").clicked() {
                self.open_search(false);
                ui.close();
            }
            if ui.button("Replace…").clicked() {
                self.open_search(true);
                ui.close();
            }
        });
    }
    pub(super) fn search_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
    ) -> Result<(), EditorError> {
        let starting_revision = snapshot.revision;
        let focused = ui.memory(|m| m.focused());
        let owns_focus = focused
            .is_some_and(|focused| [id, id.with("find"), id.with("replace")].contains(&focused));
        if owns_focus && !self.is_composing() {
            let not_mac = input_os_not_mac(ui);
            let mut open = None;
            let mut navigate = None;
            let mut close = false;
            ui.input_mut(|input| {
                input.events.retain(|event| {
                    if let egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    } = event
                    {
                        if modifiers.command && *key == egui::Key::F {
                            open = Some(modifiers.alt);
                            return false;
                        }
                        if modifiers.command && *key == egui::Key::H && not_mac {
                            open = Some(true);
                            return false;
                        }
                        if self.search.open && focused != Some(id) && *key == egui::Key::Enter {
                            navigate = Some(!modifiers.shift);
                            return false;
                        }
                        if self.search.open
                            && (*key == egui::Key::F3
                                || (*key == egui::Key::G && modifiers.command))
                        {
                            navigate = Some(!modifiers.shift);
                            return false;
                        }
                        if self.search.open && *key == egui::Key::Escape {
                            close = true;
                            return false;
                        }
                    }
                    true
                })
            });
            if let Some(replace) = open {
                let range = self.selection.range();
                if !range.is_empty() && range.len() <= 4096 {
                    self.search.query.text = snapshot.text.range_text(range);
                }
                self.open_search(replace);
                session.end_undo_group();
            }
            if let Some(forward) = navigate {
                self.search.navigation = Some(forward);
            }
            if close {
                self.close_search(ui, id);
            }
        }
        if !self.search.open {
            return Ok(());
        }
        let mut close = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let mut query_changed = false;
        let enabled = !self.settings.read_only
            && !self.is_composing()
            && self.search.replacement_job.is_none();
        ui.horizontal_wrapped(|ui| {
            ui.label("Find");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search.query.text)
                    .id(id.with("find"))
                    .desired_width(180.0)
                    .hint_text("Find text")
                    .char_limit(4096)
                    .event_filter(egui::EventFilter {
                        escape: true,
                        horizontal_arrows: true,
                        ..Default::default()
                    }),
            );
            query_changed |= response.changed();
            if self.search.focus {
                response.request_focus();
                self.search.focus = false;
            }
            if response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            {
                self.search.navigation = Some(true);
            }
            if response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter))
            {
                self.search.navigation = Some(false);
            }
            query_changed |= ui
                .checkbox(&mut self.search.query.match_case, "Match case")
                .changed();
            query_changed |= ui
                .checkbox(&mut self.search.query.whole_word, "Whole word")
                .changed();
            if ui.button("Previous").clicked() {
                self.search.navigation = Some(false);
            }
            if ui.button("Next").clicked() {
                self.search.navigation = Some(true);
            }
            ui.checkbox(&mut self.search.replace, "Replace");
            close = ui.button("Close find").clicked();
            if let Some(result) = &self.search.results {
                ui.weak(format!("{} matches", result.count));
            }
            if self.search.job.is_some() {
                ui.spinner();
            }
        });
        if self.search.replace {
            ui.horizontal_wrapped(|ui| {
                ui.label("With");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search.replacement)
                        .id(id.with("replace"))
                        .desired_width(180.0)
                        .hint_text("Replacement")
                        .event_filter(egui::EventFilter {
                            escape: true,
                            horizontal_arrows: true,
                            ..Default::default()
                        }),
                );
                replace_one = ui
                    .add_enabled(enabled, egui::Button::new("Replace next"))
                    .clicked();
                replace_all = ui
                    .add_enabled(
                        enabled && !self.search.query.text.is_empty(),
                        egui::Button::new("Replace all"),
                    )
                    .clicked();
                if self.search.replacement_job.is_some() {
                    ui.spinner();
                    if ui.button("Cancel replace").clicked() {
                        self.search.replacement_job = None;
                        self.search.message = Some("Replacement cancelled".into());
                    }
                }
            });
        }
        if query_changed {
            self.search.navigation = Some(true);
            self.search.message = None;
            self.search.requested = None;
            self.search.replacement_job = None;
        }
        let anchor = if self.search.navigation == Some(false) {
            self.selection.range().start
        } else {
            self.selection.range().end
        };
        self.search.refresh(ui.ctx(), snapshot, anchor);
        if let Some(forward) = self.search.navigation {
            if let Some(result) = &self.search.results {
                if let Some(range) = result.at(anchor, forward) {
                    self.selection = TextSelection {
                        anchor: range.start,
                        head: range.end,
                    };
                    self.navigation = Some(Navigation {
                        revision: snapshot.revision,
                        range,
                        selection: Some(self.selection),
                        reveal: Reveal::Minimal,
                        focus: false,
                    });
                    self.search.navigation = None;
                    session.end_undo_group();
                } else if result.count == 0 {
                    self.search.navigation = None;
                }
            }
        }
        if replace_one && !self.search.query.text.is_empty() {
            if let Some(result) = &self.search.results {
                let selected = self.selection.range();
                // The current selection must be an observed exact match. An
                // arbitrary selection is never silently replaced by Find.
                let current = result.ranges.iter().any(|r| *r == selected)
                    || result.next.as_ref() == Some(&selected)
                    || result.previous.as_ref() == Some(&selected);
                if current {
                    session.end_undo_group();
                    self.replace_selection(
                        id,
                        session,
                        snapshot,
                        self.search.replacement.clone(),
                        ui.input(|i| i.time),
                    )?;
                    session.end_undo_group();
                }
                self.search.navigation = Some(true);
            }
        }
        if replace_all {
            let source = snapshot.clone();
            let query = self.search.query.clone();
            let replacement = self.search.replacement.clone();
            self.search.replacement_job = Some((
                source.revision,
                background::Job::start(ui.ctx(), move |cancel| {
                    search::replace_all(&source, &query, &replacement, cancel)
                }),
            ));
            self.search.message = None;
        }
        if let Some((revision, job)) = &self.search.replacement_job {
            let revision = *revision;
            match job.poll() {
                Ok(Some(result)) => {
                    self.search.replacement_job = None;
                    if revision != snapshot.revision {
                        self.search.message = Some(
                            "Document changed; replace all was not applied. Try again.".into(),
                        );
                    } else if !enabled && self.is_composing() {
                        self.search.message =
                            Some("Finish composition, then try replace all again.".into());
                    } else if self.settings.read_only {
                        self.search.message =
                            Some("Document is read-only; replacement was not applied.".into());
                    } else {
                        match result {
                            Ok((replacement, count)) => {
                                session.end_undo_group();
                                session.set_edit_owner(id);
                                let result = session.edit_buffer(
                                    revision,
                                    BufferEdit::new(0..snapshot.text.len(), replacement),
                                );
                                session.end_undo_group();
                                match result {
                                    Ok(_) => {
                                        *snapshot = session.snapshot()?;
                                        self.selection = TextSelection::caret(
                                            &snapshot.text,
                                            snapshot.text.clamp_boundary(self.selection.head),
                                        )?;
                                        self.search.message =
                                            Some(format!("Replaced {count} matches"));
                                    }
                                    Err(error) => {
                                        self.search.message =
                                            Some(format!("Replacement was not applied: {error}"))
                                    }
                                }
                            }
                            Err(error) => self.search.message = Some(error.to_string()),
                        }
                    }
                }
                Err(()) => {
                    self.search.replacement_job = None;
                    self.search.message =
                        Some("Replacement worker stopped; no changes applied".into());
                }
                _ => {}
            }
        }
        if let Some(message) = &self.search.message {
            ui.weak(message);
        }
        if snapshot.revision != starting_revision {
            // A Find action may have queued a reveal before this replacement
            // committed. Observations and requests from that revision expire.
            self.navigation = None;
            self.search.results = None;
            self.search.requested = None;
            self.search.job = None;
        }
        if close {
            self.close_search(ui, id);
        }
        Ok(())
    }
    fn close_search(&mut self, ui: &egui::Ui, id: egui::Id) {
        self.search.open = false;
        self.search.job = None;
        self.search.replacement_job = None;
        self.search.navigation = None;
        self.search.requested = None;
        self.search.results = None;
        self.request_focus = true;
        ui.memory_mut(|m| m.request_focus(id));
    }
}
fn input_os_not_mac(ui: &egui::Ui) -> bool {
    ui.ctx().os() != egui::os::OperatingSystem::Mac
}
