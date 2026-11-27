//! A box to pick one of many rows by typing: a field over a list narrowed by
//! what's typed, best first. Arrows and Page Up/Down move, Enter or a click
//! chooses, Escape or a click outside dismisses. Go to Symbol and the command
//! palette are pickers.
//!
//! The caller keeps the state (`PickerState`) and the rows (`PickerRows`:
//! filtering, labels, painting); `Picker::show` draws them and says what
//! happened. It doesn't touch the world, so what choosing does is the
//! caller's.
//!
//! A picker can also be the list alone, over a widget the query is typed in
//! (completions above a text area): `Picker::keys` before that widget, then
//! `Picker::show_above`.
use crate::Text;
use crate::UiContext;
use bevy_egui::egui;

/// What's typed and selected in a picker, kept by its owner between frames.
#[derive(Clone, Debug, Default)]
pub struct PickerState {
    pub query: String,
    /// The selected row, into the rows as last filtered. Set it before the
    /// first show to start somewhere other than the first row.
    pub selected: usize,
    /// The query the rows were last filtered with.
    filtered: Option<String>,
    focused: bool,
    /// Scroll the selected row into view next time.
    scroll: bool,
    /// The list's scroll offset last drawn.
    offset: f32,
}

impl PickerState {
    /// Starting with `selected` selected (the caret's entry, say).
    pub fn at(selected: usize) -> Self {
        Self {
            selected,
            ..Self::default()
        }
    }

    /// The rows themselves changed (not the query): filter them again next
    /// time, keeping the selection where it still fits.
    pub fn stale(&mut self) {
        self.filtered = None;
    }

    /// Filter `rows` by the query if it changed since they last were. The
    /// first time the selection stays (clamped); after, the best row is.
    pub fn filter(&mut self, rows: &mut impl PickerRows) {
        if self.filtered.as_deref() == Some(self.query.as_str()) {
            return;
        }
        rows.filter(&self.query);
        let last = rows.len().saturating_sub(1);
        self.selected = match self.filtered {
            None => self.selected.min(last),
            Some(_) => 0,
        };
        self.filtered = Some(self.query.clone());
        self.scroll = true;
    }
}

/// A picker's rows, as the caller has them.
pub trait PickerRows {
    /// Narrow and order the rows for `query`, best first.
    fn filter(&mut self, query: &str);
    /// How many rows there are, as last filtered.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Row `row`'s text, for accessibility.
    fn label(&self, row: usize) -> String;
    /// Paint row `row` in `rect`. The selection and hover fill are drawn.
    fn paint(&self, ui: &mut egui::Ui, context: &UiContext<'_>, rect: egui::Rect, row: usize);
}

/// What happened in a picker this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerEvent {
    /// Enter or a click on a row.
    Chosen(usize),
    /// The selection moved by key to this row.
    Moved(usize),
    /// Escape.
    Escaped,
    /// A press outside the picker.
    ClickedOutside,
}

/// A picker's look and place. `show` draws it floating, `show_inside` in a
/// container of the caller's, `show_above` as a list over another widget.
pub struct Picker {
    id: egui::Id,
    top_center: egui::Pos2,
    width: f32,
    hint: String,
    empty: String,
    max_rows: usize,
}

impl Picker {
    /// A picker `width` wide, hanging from `top_center`.
    pub fn new(id: impl Into<egui::Id>, top_center: egui::Pos2, width: f32) -> Self {
        Self {
            id: id.into(),
            top_center,
            width,
            hint: String::new(),
            empty: "No matches".into(),
            max_rows: 12,
        }
    }

    /// The field's placeholder, also its accessible name.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Shown when no row matches.
    pub fn empty(mut self, empty: impl Into<String>) -> Self {
        self.empty = empty.into();
        self
    }

    /// Rows shown before the list scrolls.
    pub fn max_rows(mut self, rows: usize) -> Self {
        self.max_rows = rows.max(1);
        self
    }

    /// Draw the picker floating over `ui`'s context, taking the keyboard.
    pub fn show(
        self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        state: &mut PickerState,
        rows: &mut impl PickerRows,
    ) -> Option<PickerEvent> {
        let width = self.width;
        let shown = egui::Area::new(self.id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                self.top_center.x - width / 2.0,
                self.top_center.y,
            ))
            .constrain_to(ui.ctx().content_rect().shrink(8.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_width(width);
                        self.show_inside(ui, context, state, rows)
                    })
                    .inner
            });
        let rect = shown.response.rect;
        let clicked_outside = ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pointer| !rect.contains(pointer))
        });
        match shown.inner {
            Some(PickerEvent::Moved(_)) | None if clicked_outside => {
                Some(PickerEvent::ClickedOutside)
            }
            event => event,
        }
    }

    /// Draw the picker's field and list in `ui` (a modal's, say), taking
    /// the keyboard, as wide as `ui` is. Its place and width are the
    /// container's; it never reports `ClickedOutside`.
    pub fn show_inside(
        self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        state: &mut PickerState,
        rows: &mut impl PickerRows,
    ) -> Option<PickerEvent> {
        state.filter(rows);
        let keys = take_keys(ui, state, rows.len(), false);
        let mut chosen = keys.chosen;
        let field = ui.add(
            crate::TextField::new(&mut state.query)
                .id(self.id)
                .hint(self.hint.as_str())
                .label(self.hint.as_str())
                .fill_width(),
        );
        if !state.focused {
            field.request_focus();
            state.focused = true;
        }
        state.filter(rows);
        ui.add_space(4.0);
        if rows.is_empty() {
            ui.add(Text::new(self.empty.as_str()).muted());
        } else {
            self.list(ui, context, state, rows, &mut chosen);
        }
        if let Some(row) = chosen.filter(|&row| row < rows.len()) {
            Some(PickerEvent::Chosen(row))
        } else if keys.escape {
            Some(PickerEvent::Escaped)
        } else {
            keys.moved.then_some(PickerEvent::Moved(state.selected))
        }
    }

    /// The keys of a picker whose query is typed somewhere else (a text
    /// area the list floats over, `show_above`): the arrows and Page
    /// Up/Down move, Enter or Tab chooses, Escape dismisses. Set
    /// `state.query` first, and call this before that widget is added, so
    /// the keys are the picker's and not its.
    pub fn keys(
        ui: &mut egui::Ui,
        state: &mut PickerState,
        rows: &mut impl PickerRows,
    ) -> Option<PickerEvent> {
        state.filter(rows);
        if rows.is_empty() {
            return None;
        }
        let keys = take_keys(ui, state, rows.len(), true);
        if let Some(row) = keys.chosen {
            Some(PickerEvent::Chosen(row))
        } else if keys.escape {
            Some(PickerEvent::Escaped)
        } else {
            keys.moved.then_some(PickerEvent::Moved(state.selected))
        }
    }

    /// Draw the list alone, floating above `anchor` (the widget the query
    /// is typed in), under the hint as a caption. It has no field and
    /// doesn't take the focus or the keyboard (`keys` does, before the
    /// anchor is drawn); a click on a row is `Chosen`. Nothing is drawn
    /// when no row matches. The picker's width is the most it's given.
    pub fn show_above(
        self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        anchor: egui::Rect,
        state: &mut PickerState,
        rows: &mut impl PickerRows,
    ) -> Option<PickerEvent> {
        state.filter(rows);
        if rows.is_empty() {
            return None;
        }
        // The whole popup is as wide as the anchor, its frame included.
        let frame = egui::Frame::popup(ui.style());
        let width = (anchor.width().min(self.width) - frame.total_margin().sum().x).max(0.0);
        let mut chosen = None;
        egui::Area::new(self.id.with("area"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(egui::pos2(anchor.left(), anchor.top() - 4.0))
            .constrain_to(ui.ctx().content_rect().shrink(8.0))
            .show(ui.ctx(), |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width);
                    if !self.hint.is_empty() {
                        ui.add(Text::caption(self.hint.as_str()).muted());
                        ui.add_space(4.0);
                    }
                    self.list(ui, context, state, rows, &mut chosen);
                });
            });
        chosen
            .filter(|&row| row < rows.len())
            .map(PickerEvent::Chosen)
    }

    /// The rows, scrolled to the selected one when it moved. A click on one
    /// sets `chosen`.
    fn list(
        &self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        state: &mut PickerState,
        rows: &impl PickerRows,
        chosen: &mut Option<usize>,
    ) {
        let count = rows.len();
        let row_height = crate::tree::row_height(context);
        ui.spacing_mut().item_spacing.y = 0.0;
        // As tall as its rows (up to `max_rows`), whatever room the
        // container had last frame: a modal's area remembers its size.
        let height = row_height * count.min(self.max_rows) as f32;
        let mut list = egui::ScrollArea::vertical()
            .id_salt(self.id.with("list"))
            .min_scrolled_height(height)
            .max_height(height)
            .auto_shrink([false, true]);
        if std::mem::take(&mut state.scroll) {
            // Just enough to show it.
            let top = state.selected as f32 * row_height;
            let offset = if top < state.offset {
                top
            } else if top + row_height > state.offset + height {
                top + row_height - height
            } else {
                state.offset
            };
            list = list.vertical_scroll_offset(offset);
        }
        let output = list.show_rows(ui, row_height, count, |ui, range| {
            for row in range {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), row_height),
                    egui::Sense::click(),
                );
                let selected = row == state.selected;
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        rows.label(row),
                    )
                });
                if selected || response.hovered() {
                    let fill = if selected {
                        ui.visuals().selection.bg_fill.gamma_multiply(0.6)
                    } else {
                        ui.visuals().widgets.hovered.weak_bg_fill
                    };
                    ui.painter().rect_filled(rect, 4.0, fill);
                }
                if response.clicked() {
                    *chosen = Some(row);
                }
                rows.paint(ui, context, rect, row);
            }
        });
        state.offset = output.state.offset.y;
    }
}

/// What a picker's keys did this frame.
struct Keys {
    /// Enter (or Tab, where it chooses) on this row.
    chosen: Option<usize>,
    escape: bool,
    moved: bool,
}

/// Take the picker's keys from `ui`'s input and move the selection among
/// `count` rows.
fn take_keys(ui: &mut egui::Ui, state: &mut PickerState, count: usize, tab_chooses: bool) -> Keys {
    let (down, up, page_down, page_up, enter, escape) = ui.input_mut(|input| {
        let mut key = |key| input.consume_key(egui::Modifiers::NONE, key);
        (
            key(egui::Key::ArrowDown),
            key(egui::Key::ArrowUp),
            key(egui::Key::PageDown),
            key(egui::Key::PageUp),
            key(egui::Key::Enter) | (tab_chooses && key(egui::Key::Tab)),
            key(egui::Key::Escape),
        )
    });
    let before = state.selected;
    if count > 0 {
        if down {
            state.selected = (state.selected + 1) % count;
        }
        if up {
            state.selected = (state.selected + count - 1) % count;
        }
        if page_down {
            state.selected = (state.selected + 10).min(count - 1);
        }
        if page_up {
            state.selected = state.selected.saturating_sub(10);
        }
    }
    let moved = state.selected != before;
    state.scroll |= moved;
    Keys {
        chosen: enter.then_some(state.selected).filter(|_| count > 0),
        escape,
        moved,
    }
}

/// `label` with the characters at `matched` (char indices, from `fuzzy`) in
/// `accent` and underlined, the rest in `text`.
pub fn highlighted(
    label: &str,
    matched: &[usize],
    font: egui::FontId,
    text: egui::Color32,
    accent: egui::Color32,
) -> egui::text::LayoutJob {
    highlighted_with(label, matched, font, |_| text, accent)
}

/// As `highlighted`, with each unmatched character in `ink(index)` (a
/// quieter prefix, say).
pub fn highlighted_with(
    label: &str,
    matched: &[usize],
    font: egui::FontId,
    ink: impl Fn(usize) -> egui::Color32,
    accent: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (at, c) in label.chars().enumerate() {
        let mut format = egui::TextFormat::simple(font.clone(), ink(at));
        if matched.contains(&at) {
            format.color = accent;
            format.underline = egui::Stroke::new(1.0, accent);
        }
        let mut buffer = [0; 4];
        job.append(c.encode_utf8(&mut buffer), 0.0, format);
    }
    job
}

/// The characters of `label` (as char indices) that `query` matches in order,
/// ignoring case and the query's spaces, with a score: higher for matches at
/// the start, after separators and in runs. `None` if it doesn't match.
pub fn fuzzy(label: &str, query: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = label.chars().collect();
    let mut matched = Vec::with_capacity(query.len());
    let mut score = 0;
    let mut wanted = 0;
    let mut previous: Option<usize> = None;
    for (index, &c) in chars.iter().enumerate() {
        if wanted == query.len() {
            break;
        }
        if !c.to_lowercase().eq(std::iter::once(query[wanted])) {
            continue;
        }
        let boundary = index == 0
            || chars
                .get(index - 1)
                .is_some_and(|p| !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()));
        score += 1;
        if boundary {
            score += 8;
        }
        if index == 0 {
            score += 4;
        }
        if previous.is_some_and(|p| p + 1 == index) {
            score += 5;
        }
        previous = Some(index);
        matched.push(index);
        wanted += 1;
    }
    if wanted < query.len() {
        return None;
    }
    // Shorter names are closer matches.
    score -= (chars.len() as i32 - query.len() as i32).min(20) / 4;
    Some((score, matched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_starts_and_runs() {
        assert!(fuzzy("dog", "cat").is_none());
        let (_, at) = fuzzy("draw_outline", "dro").unwrap();
        assert_eq!(at, vec![0, 1, 5]);
        let (start, _) = fuzzy("outline", "out").unwrap();
        let (middle, _) = fuzzy("draw_outline", "out").unwrap();
        assert!(start > middle);
        let (camel, _) = fuzzy("StructurePanel", "sp").unwrap();
        let (plain, _) = fuzzy("spare", "sp").unwrap();
        assert!(camel > 0 && plain > 0);
        assert_eq!(fuzzy("Cat", "CAT").unwrap().1, vec![0, 1, 2]);
    }

    struct Words(Vec<&'static str>, Vec<usize>);

    impl PickerRows for Words {
        fn filter(&mut self, query: &str) {
            self.1 = (0..self.0.len())
                .filter(|&at| fuzzy(self.0[at], query).is_some())
                .collect();
        }
        fn len(&self) -> usize {
            self.1.len()
        }
        fn label(&self, row: usize) -> String {
            self.0[self.1[row]].into()
        }
        fn paint(&self, _: &mut egui::Ui, _: &UiContext<'_>, _: egui::Rect, _: usize) {}
    }

    #[test]
    fn the_first_filter_keeps_the_selection_and_later_ones_pick_the_best() {
        let mut rows = Words(vec!["alpha", "beta", "gamma"], Vec::new());
        let mut state = PickerState::at(2);
        state.filter(&mut rows);
        assert_eq!((rows.len(), state.selected), (3, 2));
        state.query = "a".into();
        state.filter(&mut rows);
        assert_eq!(state.selected, 0);
        state.selected = 1;
        state.filter(&mut rows);
        assert_eq!(state.selected, 1, "unchanged query, unchanged selection");
        let mut past_the_end = PickerState::at(9);
        past_the_end.filter(&mut rows);
        assert_eq!(past_the_end.selected, 2);
    }
}
