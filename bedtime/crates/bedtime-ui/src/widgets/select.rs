use super::{Button, Text, TextField};
use crate::{ambient, motion, picker::fuzzy, theme, tokens::*};

/// One of several options, chosen from a list that drops down.
///
/// ```ignore
/// ui.add(Select::new(&mut method, &Method::ALL));
/// ui.add(Select::with(&mut font, &fonts, |font| font.family.clone()).searchable());
/// ```
///
/// The list takes the arrow keys and Enter; `searchable` adds a field that
/// narrows it as you type.
#[must_use = "a select does nothing until it's added to a Ui"]
pub struct Select<'a, T> {
    value: &'a mut T,
    options: &'a [T],
    labels: Vec<String>,
    placeholder: String,
    size: Size,
    width: Option<f32>,
    fill_width: bool,
    searchable: bool,
    dividers: Vec<usize>,
}

/// What's typed and highlighted in an open list.
#[derive(Clone, Default)]
struct Open {
    query: String,
    highlighted: usize,
    /// The list has been drawn since it opened.
    shown: bool,
    /// The search field has had the keyboard since it opened.
    focused: bool,
}

impl<'a, T: PartialEq + Clone> Select<'a, T> {
    /// The options, named by how they `Display`.
    pub fn new(value: &'a mut T, options: &'a [T]) -> Self
    where
        T: std::fmt::Display,
    {
        Self::with(value, options, T::to_string)
    }

    /// The options, named by `label`.
    pub fn with(value: &'a mut T, options: &'a [T], label: impl Fn(&T) -> String) -> Self {
        Self {
            value,
            options,
            labels: options.iter().map(label).collect(),
            placeholder: "Choose…".into(),
            size: Size::default(),
            width: None,
            fill_width: false,
            searchable: false,
            dividers: Vec::new(),
        }
    }

    /// Shown when the value is none of the options.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    /// This wide, in points (as wide as its longest option, unless set).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    /// As wide as the space it's in.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }
    /// A line before each of these options (by index), to group the list.
    pub fn dividers(mut self, before: impl IntoIterator<Item = usize>) -> Self {
        self.dividers = before.into_iter().collect();
        self
    }
    /// A field over the list that narrows it to what's typed.
    pub fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }
}

impl<T: PartialEq + Clone> egui::Widget for Select<'_, T> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let padding = self.size.padding(ui);
        let height = self.size.height(ui);
        let font = egui::FontId::proportional(font_size);
        let current = self.options.iter().position(|option| option == self.value);
        let shown = current.map_or(&self.placeholder, |index| &self.labels[index]);
        let chevron = font_size;
        let gap = (font_size * 0.4).round();
        let available = ui.available_width();
        let width = if self.fill_width {
            available
        } else if let Some(width) = self.width {
            width.min(available)
        } else {
            // Wide enough for any option, so choosing one doesn't resize it.
            let longest = self
                .labels
                .iter()
                .chain([&self.placeholder])
                .take(200)
                .map(|label| {
                    ui.painter()
                        .layout_no_wrap(label.clone(), font.clone(), egui::Color32::PLACEHOLDER)
                        .size()
                        .x
                })
                .fold(0.0, f32::max);
            (longest + gap + chevron + 2.0 * padding.x).min(available)
        };
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        let enabled = ui.is_enabled();
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, shown));

        // A list of this select's own rows, at their own size: not a menu's
        // compact style.
        let popup = egui::Popup::menu(&response)
            .style(|_: &mut egui::Style| {})
            .gap(Space::Xs.px(ui))
            .width(rect.width())
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup_id = popup.get_id();
        // A click opens or closes it, when the popup's shown (below).
        let is_open = popup.is_open() != response.clicked();

        if ui.is_rect_visible(rect) {
            let animate = |name: &str, on: bool| {
                let target = if on { 1.0 } else { 0.0 };
                motion::scalar(
                    ui.ctx(),
                    response.id.with(name),
                    target,
                    target,
                    services.motion,
                )
            };
            let hover = animate("hover", response.hovered() || response.has_focus());
            let open = animate("open", is_open);
            let p = &services.theme.palette;
            let surface = theme::distinct(p.surface, p.panel_background, p.text);
            let fill = theme::mix(surface, p.accent, 0.2 * hover.max(open));
            let border = theme::mix(p.border, p.accent, open);
            let radius = Radius::Md.px(ui);
            let painter = ui.painter();
            painter.rect(
                rect,
                radius,
                theme::color(fill),
                egui::Stroke::new(1.0, theme::color(border)),
                egui::StrokeKind::Inside,
            );
            if response.has_focus() {
                painter.rect_stroke(
                    rect.expand(2.0),
                    radius + 2.0,
                    egui::Stroke::new(1.5, theme::color(p.accent)),
                    egui::StrokeKind::Inside,
                );
            }
            let ink = if current.is_some() { p.text } else { p.muted };
            let text_width = rect.width() - 2.0 * padding.x - gap - chevron;
            let galley = ui.painter().layout_job(egui::text::LayoutJob {
                wrap: egui::text::TextWrapping::truncate_at_width(text_width.max(0.0)),
                ..egui::text::LayoutJob::simple_singleline(
                    shown.clone(),
                    font.clone(),
                    theme::color(ink),
                )
            });
            let at = egui::pos2(
                rect.left() + padding.x,
                rect.center().y - galley.size().y / 2.0,
            );
            painter.galley(at, galley, theme::color(ink));
            crate::paint_chevron(
                painter,
                egui::pos2(rect.right() - padding.x - chevron / 2.0, rect.center().y),
                1.0,
                theme::color(p.muted),
            );
        }

        let mut state = is_open.then(|| {
            ui.data(|data| data.get_temp::<Open>(popup_id))
                .unwrap_or_else(|| Open {
                    highlighted: current.unwrap_or(0),
                    ..Open::default()
                })
        });
        let mut chosen = None;
        let (labels, searchable, size) = (&self.labels, self.searchable, self.size);
        let dividers = &self.dividers;
        popup.show(|ui| {
            let Some(state) = state.as_mut() else {
                return;
            };
            let matches = |query: &str| -> Vec<usize> {
                if query.is_empty() {
                    return (0..labels.len()).collect();
                }
                let mut scored: Vec<(i32, usize)> = labels
                    .iter()
                    .enumerate()
                    .filter_map(|(index, label)| Some((fuzzy(label, query)?.0, index)))
                    .collect();
                scored.sort_by_key(|(score, index)| (-score, *index));
                scored.into_iter().map(|(_, index)| index).collect()
            };
            if searchable {
                let before = state.query.clone();
                // Ask until it has it: the list's first frame only measures,
                // and a request made then is dropped.
                let field = ui.add(
                    TextField::new(&mut state.query)
                        .hint("Search")
                        .size(size)
                        .fill_width()
                        .focus(!state.focused),
                );
                state.focused |= field.has_focus();
                if state.query != before {
                    // Typed: the best match is highlighted.
                    state.highlighted = matches(&state.query).first().copied().unwrap_or(0);
                }
                ui.add_space(Space::Xs.px(ui));
            }
            let rows = matches(&state.query);
            let (down, up, enter) = ui.input_mut(|input| {
                let mut key = |key| input.consume_key(egui::Modifiers::NONE, key);
                (
                    key(egui::Key::ArrowDown),
                    key(egui::Key::ArrowUp),
                    key(egui::Key::Enter),
                )
            });
            let mut scroll = !state.shown;
            if !rows.is_empty() && (down || up) {
                let at = rows.iter().position(|row| *row == state.highlighted);
                let next = match (at, down) {
                    (Some(at), true) => (at + 1) % rows.len(),
                    (Some(at), false) => (at + rows.len() - 1) % rows.len(),
                    (None, _) => 0,
                };
                state.highlighted = rows[next];
                scroll = true;
            }
            if enter && rows.contains(&state.highlighted) {
                chosen = Some(state.highlighted);
            }
            if rows.is_empty() {
                ui.add(Text::new("No matches").muted());
            }
            egui::ScrollArea::vertical()
                .max_height(size.height(ui) * 9.5)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let grouped = state.query.is_empty();
                    for row in rows {
                        if grouped && row > 0 && dividers.contains(&row) {
                            ui.add(super::Separator::new().space(Space::Xs));
                        }
                        let button = Button::new(&labels[row])
                            .size(size)
                            .ghost()
                            .fill_width()
                            .align(egui::Align::Min)
                            .selected(row == state.highlighted);
                        let response = ui.add(button);
                        if row == state.highlighted && scroll {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            chosen = Some(row);
                        }
                    }
                });
        });
        let Some(mut state) = state else {
            ui.data_mut(|data| data.remove::<Open>(popup_id));
            return response;
        };
        state.shown = true;
        if let Some(chosen) = chosen {
            if *self.value != self.options[chosen] {
                *self.value = self.options[chosen].clone();
                response.mark_changed();
            }
            egui::Popup::close_id(ui.ctx(), popup_id);
            ui.data_mut(|data| data.remove::<Open>(popup_id));
        } else {
            ui.data_mut(|data| data.insert_temp(popup_id, state));
        }
        response
    }
}
