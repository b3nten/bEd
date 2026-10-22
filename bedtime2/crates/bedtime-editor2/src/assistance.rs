//! Optional host-supplied completion presentation and atomic edit acceptance.
//! There is no protocol knowledge or second text owner in this module.
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct CompletionEdit {
    pub primary: TextEdit,
    pub additional: Vec<TextEdit>,
    pub snippet: bool,
}
pub struct PreparedCompletion {
    pub edit: BufferEdit,
    pub stops: Vec<(u32, Range<usize>)>,
    pub caret: usize,
}
impl CompletionEdit {
    pub fn prepare(&self, source: &TextBuffer) -> Result<PreparedCompletion, String> {
        let (inserted, mut stops) = if self.snippet {
            parse_snippet(&self.primary.replacement)?
        } else {
            (self.primary.replacement.clone(), Vec::new())
        };
        let primary = TextEdit {
            range: self.primary.range.clone(),
            replacement: inserted.clone(),
        };
        let mut edits: Vec<_> = self.additional.iter().cloned().chain([primary]).collect();
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        for edit in &edits {
            if edit.range.start > edit.range.end
                || !source.is_char_boundary(edit.range.start)
                || !source.is_char_boundary(edit.range.end)
            {
                return Err("Completion has an invalid UTF-8 range".into());
            }
        }
        for pair in edits.windows(2) {
            if pair[0].range.end > pair[1].range.start || pair[0].range.start == pair[1].range.start
            {
                return Err("Completion edits overlap".into());
            }
        }
        if edits.iter().map(|e| e.replacement.len()).sum::<usize>() > 8 * 1024 * 1024 {
            return Err("Completion replacement exceeds 8 MiB".into());
        }
        let start = edits.first().unwrap().range.start;
        let end = edits.last().unwrap().range.end;
        let mut result = source.slice(start..end);
        let mut new_end = end;
        let mut primary_start = self.primary.range.start;
        for edit in edits.iter().rev() {
            BufferEdit::new(
                edit.range.start - start..edit.range.end - start,
                edit.replacement.as_str(),
            )
            .apply(&mut result)?;
            new_end = new_end - edit.range.len() + edit.replacement.len();
        }
        for edit in &self.additional {
            if edit.range.end <= self.primary.range.start {
                primary_start = primary_start - edit.range.len() + edit.replacement.len();
            }
        }
        for (_, range) in &mut stops {
            range.start += primary_start;
            range.end += primary_start;
        }
        if !stops.iter().any(|(n, _)| *n == 0) {
            stops.push((
                0,
                primary_start + inserted.len()..primary_start + inserted.len(),
            ));
        }
        stops.sort_by_key(|(n, r)| (if *n == 0 { u32::MAX } else { *n }, r.start));
        let caret = stops
            .first()
            .map_or(primary_start + inserted.len(), |(_, r)| r.end);
        debug_assert_eq!(start + result.len(), new_end);
        Ok(PreparedCompletion {
            edit: BufferEdit::new(start..end, result),
            stops,
            caret,
        })
    }
}

/// LSP snippets: tabstops, nested defaults, choices, escapes and variable
/// defaults. Transform expressions are rejected instead of inserted literally.
pub fn parse_snippet(input: &str) -> Result<(String, Vec<(u32, Range<usize>)>), String> {
    struct Parser<'a> {
        input: &'a str,
        at: usize,
        out: String,
        stops: Vec<(u32, Range<usize>)>,
        values: BTreeMap<u32, String>,
    }
    impl Parser<'_> {
        fn part(&mut self, nested: bool, depth: usize) -> Result<(), String> {
            if depth > 32 {
                return Err("Snippet nesting exceeds 32".into());
            }
            while self.at < self.input.len() {
                let c = self.input[self.at..].chars().next().unwrap();
                self.at += c.len_utf8();
                if nested && c == '}' {
                    return Ok(());
                }
                if c == '\\' {
                    let next = self.input[self.at..]
                        .chars()
                        .next()
                        .ok_or("Unterminated snippet escape")?;
                    if matches!(next, '$' | '}' | '\\' | ',' | '|') {
                        self.at += next.len_utf8();
                        self.out.push(next);
                    } else {
                        self.out.push(c);
                    }
                    continue;
                }
                if c != '$' {
                    self.out.push(c);
                    continue;
                }
                let braced = self.input[self.at..].starts_with('{');
                if braced {
                    self.at += 1;
                }
                let begin = self.at;
                while self.at < self.input.len() && self.input.as_bytes()[self.at].is_ascii_digit()
                {
                    self.at += 1;
                }
                let number = if self.at > begin {
                    Some(
                        self.input[begin..self.at]
                            .parse::<u32>()
                            .map_err(|_| "Snippet tabstop is too large")?,
                    )
                } else {
                    None
                };
                if number.is_none() {
                    while self.at < self.input.len()
                        && (self.input.as_bytes()[self.at].is_ascii_alphanumeric()
                            || self.input.as_bytes()[self.at] == b'_')
                    {
                        self.at += 1;
                    }
                    if self.at == begin {
                        return Err("Invalid snippet variable".into());
                    }
                }
                let start = self.out.len();
                if braced {
                    let token = *self
                        .input
                        .as_bytes()
                        .get(self.at)
                        .ok_or("Unterminated snippet placeholder")?;
                    self.at += 1;
                    match token {
                        b'}' => {
                            if let Some(n) = number {
                                if let Some(v) = self.values.get(&n) {
                                    self.out.push_str(v);
                                }
                            }
                        }
                        b':' => self.part(true, depth + 1)?,
                        b'|' if number.is_some() => {
                            let mut first = String::new();
                            let mut first_choice = true;
                            let mut ended = false;
                            while self.at < self.input.len() {
                                let ch = self.input[self.at..].chars().next().unwrap();
                                self.at += ch.len_utf8();
                                if ch == '|' && self.input[self.at..].starts_with('}') {
                                    self.at += 1;
                                    ended = true;
                                    break;
                                }
                                if ch == ',' {
                                    first_choice = false;
                                    continue;
                                }
                                if ch == '\\' {
                                    let ch = self.input[self.at..]
                                        .chars()
                                        .next()
                                        .ok_or("Invalid snippet choice")?;
                                    self.at += ch.len_utf8();
                                    if first_choice {
                                        first.push(ch);
                                    }
                                } else if first_choice {
                                    first.push(ch);
                                }
                            }
                            if !ended {
                                return Err("Unterminated snippet choice".into());
                            }
                            self.out.push_str(&first);
                        }
                        _ => {
                            return Err(
                                "Unsupported snippet transform or invalid placeholder".into()
                            );
                        }
                    }
                } else if let Some(n) = number {
                    if let Some(v) = self.values.get(&n) {
                        self.out.push_str(v);
                    }
                }
                if let Some(n) = number {
                    if self.stops.len() >= 256 {
                        return Err("Too many snippet placeholders".into());
                    }
                    self.values
                        .entry(n)
                        .or_insert_with(|| self.out[start..].to_owned());
                    self.stops.push((n, start..self.out.len()));
                }
            }
            if nested {
                Err("Unterminated snippet placeholder".into())
            } else {
                Ok(())
            }
        }
    }
    let mut parser = Parser {
        input,
        at: 0,
        out: String::new(),
        stops: vec![],
        values: BTreeMap::new(),
    };
    parser.part(false, 0)?;
    Ok((parser.out, parser.stops))
}
#[derive(Clone, Debug)]
pub struct CompletionChoice {
    pub label: std::sync::Arc<str>,
    pub detail: std::sync::Arc<str>,
}
pub(super) struct Menu {
    revision: u64,
    choices: std::sync::Arc<[CompletionChoice]>,
    selected: usize,
    reveal: bool,
}
pub(super) struct Snippet {
    revision: u64,
    stops: Vec<(u32, Range<usize>)>,
    active: usize,
}
#[derive(Default)]
pub(crate) struct Assistance {
    pub(super) menu: Option<Menu>,
    pending: Option<(u64, CompletionEdit)>,
    pub(super) snippet: Option<Snippet>,
    pub popup_rect: Option<egui::Rect>,
    pub accepted: Option<usize>,
    pub requested: bool,
    pub refresh: bool,
    pub dismissed: bool,
    pub typed: Option<String>,
}
impl EditorView {
    pub fn set_completions(
        &mut self,
        revision: u64,
        choices: impl Into<std::sync::Arc<[CompletionChoice]>>,
    ) {
        let choices = choices.into();
        self.assistance.menu = (!choices.is_empty()).then_some(Menu {
            revision,
            choices,
            selected: 0,
            reveal: true,
        });
    }
    pub fn clear_completions(&mut self) {
        self.assistance.menu = None;
    }
    pub fn selected_completion(&self) -> Option<usize> {
        self.assistance.menu.as_ref().map(|m| m.selected)
    }
    pub fn queue_completion(&mut self, revision: u64, edit: CompletionEdit) {
        self.assistance.pending = Some((revision, edit));
        self.assistance.menu = None;
    }
    pub(crate) fn prepare_assistance(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
    ) -> Result<(), EditorError> {
        self.assistance.accepted = None;
        self.assistance.requested = false;
        self.assistance.refresh = false;
        self.assistance.dismissed = false;
        self.assistance.typed = None;
        if self
            .assistance
            .menu
            .as_ref()
            .is_some_and(|m| m.revision != snapshot.revision)
        {
            self.assistance.menu = None;
        }
        if self
            .assistance
            .snippet
            .as_ref()
            .is_some_and(|s| s.revision != snapshot.revision)
        {
            self.assistance.snippet = None;
        }
        if let Some((revision, edit)) = self.assistance.pending.take() {
            if revision == snapshot.revision && !self.settings.read_only && !self.is_composing() {
                let prepared = edit.prepare(&snapshot.text).map_err(EditorError)?;
                let range = prepared
                    .stops
                    .first()
                    .map(|(_, r)| r.clone())
                    .unwrap_or(prepared.caret..prepared.caret);
                session.set_edit_owner(id);
                session.end_undo_group();
                session.edit_buffer(revision, prepared.edit)?;
                session.end_undo_group();
                *snapshot = session.snapshot()?;
                self.selection = TextSelection::new(&snapshot.text, range.start, range.end)?;
                self.navigation = Some(Navigation {
                    revision: snapshot.revision,
                    range: range.clone(),
                    selection: Some(self.selection),
                    reveal: Reveal::Minimal,
                    focus: false,
                });
                self.assistance.snippet = (edit.snippet
                    && prepared.stops.iter().any(|(n, _)| *n != 0))
                .then_some(Snippet {
                    revision: snapshot.revision,
                    stops: prepared.stops,
                    active: 0,
                });
            }
        }
        Ok(())
    }
    pub(crate) fn assistance_event(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        event: &egui::Event,
    ) -> Result<bool, EditorError> {
        if self.is_composing() || self.settings.read_only {
            self.assistance.menu = None;
            self.assistance.snippet = None;
            return Ok(false);
        }
        if let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
        {
            if *key == egui::Key::Space && modifiers.ctrl && !modifiers.alt && !modifiers.shift {
                self.assistance.requested = true;
                return Ok(true);
            }
            if self
                .assistance
                .menu
                .as_ref()
                .is_some_and(|m| m.revision == snapshot.revision)
            {
                let menu = self.assistance.menu.as_mut().unwrap();
                match key {
                    egui::Key::ArrowDown => {
                        menu.selected = (menu.selected + 1) % menu.choices.len();
                        menu.reveal = true;
                        return Ok(true);
                    }
                    egui::Key::ArrowUp => {
                        menu.selected =
                            (menu.selected + menu.choices.len() - 1) % menu.choices.len();
                        menu.reveal = true;
                        return Ok(true);
                    }
                    egui::Key::Enter | egui::Key::Tab => {
                        self.assistance.accepted = Some(menu.selected);
                        self.assistance.menu = None;
                        return Ok(true);
                    }
                    egui::Key::Escape => {
                        self.assistance.menu = None;
                        self.assistance.dismissed = true;
                        return Ok(true);
                    }
                    egui::Key::Backspace | egui::Key::Delete => {
                        self.assistance.refresh = true;
                    }
                    _ => {}
                }
            }
            if *key == egui::Key::Tab
                && self.assistance.snippet.is_some()
                && !modifiers.ctrl
                && !modifiers.alt
                && !modifiers.command
            {
                let snippet = self.assistance.snippet.as_mut().unwrap();
                let current = snippet.stops[snippet.active].0;
                if modifiers.shift {
                    if let Some(next) = (0..snippet.active)
                        .rev()
                        .find(|i| snippet.stops[*i].0 != current)
                    {
                        snippet.active = next;
                    }
                } else if let Some(next) = (snippet.active + 1..snippet.stops.len())
                    .find(|i| snippet.stops[*i].0 != current)
                {
                    snippet.active = next;
                }
                let (number, range) = &snippet.stops[snippet.active];
                self.selection = TextSelection::new(&snapshot.text, range.start, range.end)?;
                if *number == 0 {
                    self.assistance.snippet = None;
                }
                session.end_undo_group();
                return Ok(true);
            }
            if *key == egui::Key::Escape
                && (self.assistance.snippet.is_some() || self.language_tools)
            {
                self.assistance.snippet = None;
                self.assistance.dismissed = true;
                return Ok(true);
            }
        }
        // Mirror replacements across repeated tabstops in a single transaction.
        if let Some(mut snippet) = self.assistance.snippet.take() {
            let active = snippet.stops[snippet.active].clone();
            let selected = self.selection.range();
            let mut replacement = match event {
                egui::Event::Text(t) | egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            };
            let mut range = selected.clone();
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if modifiers.is_none() && matches!(key, egui::Key::Backspace | egui::Key::Delete) {
                    replacement = Some(String::new());
                    if range.is_empty() {
                        if *key == egui::Key::Backspace {
                            range.start = snapshot.text.previous_grapheme(range.start);
                        } else {
                            range.end = snapshot.text.next_grapheme(range.end);
                        }
                    }
                }
            }
            if let Some(replacement) =
                replacement.filter(|_| range.start >= active.1.start && range.end <= active.1.end)
            {
                let relative = range.start - active.1.start..range.end - active.1.start;
                let mut edits: Vec<_> = snippet
                    .stops
                    .iter()
                    .filter(|(n, _)| *n == active.0)
                    .map(|(_, r)| TextEdit {
                        range: r.start + relative.start..r.start + relative.end,
                        replacement: replacement.clone(),
                    })
                    .collect();
                edits.sort_by_key(|e| e.range.start);
                // Nested/overlapping placeholders retire their children after a replacement.
                snippet.stops.retain(|(n, r)| {
                    *n == active.0 || !(r.start >= range.start && r.end <= range.end)
                });
                let plan = CompletionEdit {
                    primary: edits.remove(0),
                    additional: edits.clone(),
                    snippet: false,
                }
                .prepare(&snapshot.text)
                .map_err(EditorError)?;
                session.set_edit_owner(id);
                session.edit_buffer(snapshot.revision, plan.edit)?;
                *snapshot = session.snapshot()?;
                // Use all mirror locations rather than the selection when the active copy is not first.
                let mut deltas: Vec<_> = snippet
                    .stops
                    .iter()
                    .filter(|(n, _)| *n == active.0)
                    .map(|(_, r)| TextDelta {
                        range: r.start + relative.start..r.start + relative.end,
                        inserted_bytes: replacement.len(),
                    })
                    .collect();
                deltas.sort_by_key(|d| std::cmp::Reverse(d.range.start));
                let mut head = range.end;
                for delta in &deltas {
                    head = delta.map_position(head);
                    for (_, r) in &mut snippet.stops {
                        // Placeholder boundaries stay around their contents, including inserts at either edge.
                        if r.start >= delta.range.end && r.start != delta.range.start {
                            r.start = delta.map_position(r.start);
                        }
                        if r.end >= delta.range.start {
                            r.end = delta.map_position(r.end);
                        }
                    }
                }
                snippet.active = snippet
                    .stops
                    .iter()
                    .position(|(n, _)| *n == active.0)
                    .unwrap_or(0);
                snippet.revision = snapshot.revision;
                self.selection = TextSelection::caret(&snapshot.text, head)?;
                self.assistance.snippet = Some(snippet);
                return Ok(true);
            }
            if selected.start >= active.1.start && selected.end <= active.1.end {
                self.assistance.snippet = Some(snippet);
            }
        }
        if let egui::Event::Text(text) = event {
            self.assistance.typed = Some(text.clone());
        }
        Ok(false)
    }
    pub(crate) fn completion_popup(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        caret: egui::Rect,
        revision: u64,
        focused: bool,
    ) {
        self.assistance.popup_rect = None;
        if !focused
            || self.is_composing()
            || self
                .assistance
                .menu
                .as_ref()
                .is_some_and(|m| m.revision != revision)
        {
            self.assistance.menu = None;
            return;
        }
        let Some(menu) = self.assistance.menu.as_mut() else {
            return;
        };
        let mut accepted = None;
        let popup = egui::Area::new(id.with("completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(caret.left_bottom())
            .constrain_to(ui.ctx().content_rect().shrink(8.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(360.0);
                    let row_height = ui.text_style_height(&egui::TextStyle::Body);
                    let mut scroll = egui::ScrollArea::vertical().max_height(240.0);
                    if menu.reveal {
                        scroll = scroll.vertical_scroll_offset(
                            (menu.selected.saturating_sub(4)) as f32
                                * (row_height + ui.spacing().item_spacing.y),
                        );
                        menu.reveal = false;
                    }
                    scroll.show_rows(ui, row_height, menu.choices.len(), |ui, rows| {
                        for index in rows {
                            let choice = &menu.choices[index];
                            let response = ui
                                .selectable_label(index == menu.selected, choice.label.as_ref())
                                .on_hover_text(choice.detail.as_ref());
                            if response.clicked() {
                                accepted = Some(index);
                            }
                        }
                    });
                });
            });
        self.assistance.popup_rect = Some(popup.response.rect);
        if let Some(index) = accepted {
            self.assistance.accepted = Some(index);
            self.assistance.menu = None;
            ui.memory_mut(|m| m.request_focus(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{focus, frame, key};
    #[test]
    fn completion_import_and_snippet_are_one_undoable_edit() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("completion");
        let mut doc = TextDocument::new("package main\n\nfunc main() { Pr }\n");
        let mut view = EditorView::default();
        focus(&ctx, &mut view, &mut doc, id);
        let start = doc.text().to_string().find("Pr").unwrap();
        view.queue_completion(
            0,
            CompletionEdit {
                primary: TextEdit {
                    range: start..start + 2,
                    replacement: "Println(${1:message})$0".into(),
                },
                additional: vec![TextEdit {
                    range: 13..13,
                    replacement: "import \"fmt\"\n".into(),
                }],
                snippet: true,
            },
        );
        let output = frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert_eq!(
            doc.text().to_string(),
            "package main\nimport \"fmt\"\n\nfunc main() { Println(message) }\n"
        );
        assert_eq!(
            doc.text().slice(output.selection.range()).to_string(),
            "message"
        );
        assert_eq!(doc.snapshot().unwrap().revision, 1);
        doc.history(HistoryAction::Undo).unwrap();
        assert_eq!(
            doc.text().to_string(),
            "package main\n\nfunc main() { Pr }\n"
        );
    }
    #[test]
    fn stale_completion_and_overlapping_edits_leave_text_unchanged() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("stale");
        let mut doc = TextDocument::new("abc");
        let mut view = EditorView::default();
        focus(&ctx, &mut view, &mut doc, id);
        let edit = CompletionEdit {
            primary: TextEdit {
                range: 0..3,
                replacement: "ok".into(),
            },
            additional: vec![],
            snippet: false,
        };
        view.queue_completion(0, edit.clone());
        doc.edit(
            0,
            TextEdit {
                range: 3..3,
                replacement: "!".into(),
            },
        )
        .unwrap();
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert_eq!(doc.text(), "abc!");
        let mut invalid = edit;
        invalid.additional.push(TextEdit {
            range: 1..2,
            replacement: "oops".into(),
        });
        assert!(invalid.prepare(doc.text()).is_err());
        assert_eq!(doc.text(), "abc!");
    }
    #[test]
    fn snippet_mirrors_and_tab_navigation_follow_edits() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("snippet");
        let mut doc = TextDocument::new("");
        let mut view = EditorView::default();
        focus(&ctx, &mut view, &mut doc, id);
        view.queue_completion(
            0,
            CompletionEdit {
                primary: TextEdit {
                    range: 0..0,
                    replacement: "${1:name} = $1; ${2|first,second|}$0".into(),
                },
                additional: vec![],
                snippet: true,
            },
        );
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![egui::Event::Text("世界".into())],
        );
        assert_eq!(doc.text(), "世界 = 世界; first");
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        assert_eq!(
            doc.text().slice(view.selection.range()).to_string(),
            "first"
        );
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
        );
        assert_eq!(doc.text().slice(view.selection.range()).to_string(), "世界");
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![
                key(egui::Key::Tab, egui::Modifiers::NONE),
                key(egui::Key::Tab, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(view.selection.head(), doc.text().len());
        assert!(view.assistance.snippet.is_none());
    }
    #[test]
    fn completion_keys_select_without_inserting_newlines_or_tabs() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("menu");
        let mut doc = TextDocument::new("f");
        let mut view = EditorView::default();
        focus(&ctx, &mut view, &mut doc, id);
        view.set_completions(
            0,
            vec![
                CompletionChoice {
                    label: "foo".into(),
                    detail: "".into(),
                },
                CompletionChoice {
                    label: "far".into(),
                    detail: "".into(),
                },
            ],
        );
        let output = frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![
                key(egui::Key::ArrowDown, egui::Modifiers::NONE),
                key(egui::Key::Enter, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(output.completion_accepted, Some(1));
        assert_eq!(doc.text(), "f");
        let output = frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![key(egui::Key::Space, egui::Modifiers::CTRL)],
        );
        assert!(output.completion_requested);
        assert_eq!(doc.text(), "f");
    }
    #[test]
    fn snippet_defaults_escapes_and_invalid_transforms_are_explicit() {
        let (text, stops) =
            parse_snippet("${1:hello ${2:world}} \\$ ${3|a\\,b,c|} ${UNKNOWN:fallback}$0").unwrap();
        assert_eq!(text, "hello world $ a,b fallback");
        assert!(
            stops
                .iter()
                .any(|(n, r)| *n == 2 && &text[r.clone()] == "world")
        );
        assert!(parse_snippet("${1/a/b/}").is_err());
        assert!(parse_snippet("${1:unfinished").is_err());
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::*;
    #[test]
    fn command_click_reports_pointer_symbol_without_moving_the_caret() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("link");
        let mut view = EditorView::default();
        view.language_tools = true;
        let mut doc = TextDocument::new("x\t世界 name\n");
        let first = crate::tests::frame(&ctx, &mut view, &mut doc, id, vec![]);
        let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        let cell = ctx.fonts_mut(|f| f.glyph_width(&font, ' '));
        let pos = egui::pos2(
            first.text_rect.left() + 8.0 + cell * 4.2,
            first.text_rect.top() + 6.0 + first.caret_rect.height() * 0.5,
        );
        let mods = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        let mut run = |pressed| {
            let mut result = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 300.0),
                    )),
                    events: vec![
                        egui::Event::ModifiersChanged(mods),
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: mods,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    result = Some(
                        view.show(ui, id, &mut doc, MotionPolicy::default())
                            .unwrap(),
                    )
                },
            );
            result.unwrap()
        };
        let press = run(true);
        assert_eq!(press.selection.head(), 0);
        let click = run(false);
        assert_eq!(click.selection.head(), 0);
        assert_eq!(click.hovered_symbol.unwrap().range, 2..8);
        assert_eq!(click.definition_clicked, Some(2));
    }
}
