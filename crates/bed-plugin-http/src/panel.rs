use crate::{
    Action, Shared,
    model::{self, Auth, KeyValue, Request, ResolvedRequest},
    worker::BodyPresentation,
};
use bed_ui::{
    presentation::{fit_text, readable_color, same_line_if_fits},
    util::popup_style::{controls_style, tooltip, tooltip_text},
};
use bed_workbench_api::{HostContext, HostRequest, ModulePanel, ModuleServices};
use dear_imgui_rs::{ImString, ListClipper, MouseCursor, StyleColor, StyleVar, Ui};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, cell::RefCell, ffi::CString, rc::Rc};

const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
enum EditorTab {
    #[default]
    Params,
    Headers,
    Body,
    Auth,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
enum ResponseTab {
    #[default]
    Formatted,
    Raw,
    Headers,
}

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct ViewState {
    list_fraction: f32,
    request_fraction: f32,
    editor_tab: EditorTab,
    response_tab: ResponseTab,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            list_fraction: 0.25,
            request_fraction: 0.45,
            editor_tab: EditorTab::Params,
            response_tab: ResponseTab::Formatted,
        }
    }
}

struct RequestDraft {
    request: Request,
    body: ImString,
    custom_method: bool,
}

impl RequestDraft {
    fn new(request: &Request) -> Self {
        Self {
            request: request.clone(),
            body: ImString::new(request.body.replace('\0', "�")),
            custom_method: !METHODS.contains(&request.method.as_str()),
        }
    }
}

struct ResponseCache {
    run: u64,
    tab: ResponseTab,
    text: ImString,
}

pub struct HttpPanel {
    persistent: bool,
    shared: Rc<RefCell<Shared>>,
    state: ViewState,
    draft: Option<RequestDraft>,
    response_cache: Option<ResponseCache>,
}

impl HttpPanel {
    pub(crate) fn new(shared: Rc<RefCell<Shared>>, state: &Value) -> Self {
        let mut state: ViewState = serde_json::from_value(state.clone()).unwrap_or_default();
        state.list_fraction = finite_fraction(state.list_fraction, 0.25).min(0.6);
        state.request_fraction = finite_fraction(state.request_fraction, 0.45);
        Self {
            persistent: false,
            shared,
            state,
            draft: None,
            response_cache: None,
        }
    }
}

impl ModulePanel for HttpPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "HTTP Client".into()
    }

    fn save_state(&self) -> Value {
        serde_json::to_value(&self.state).expect("HTTP presentation state is serializable")
    }

    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = controls_style(ui);
        let fs = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.5, fs * 0.3]));
        let _padding = ui.push_style_var(StyleVar::FramePadding([fs * 0.45, fs * 0.2]));
        let _rounding = ui.push_style_var(StyleVar::ChildRounding(fs * 0.35));
        let style = ui.clone_style();
        let gap = style.item_spacing()[0];
        let button_width =
            |label: &str| ui.calc_text_size(label)[0] + style.frame_padding()[0] * 2.0;
        let shared = Rc::clone(&self.shared);
        let data = shared.borrow();
        let mut actions = Vec::new();
        if ui.button("New request") {
            actions.push(Action::Create);
        }
        same_line_if_fits(ui, button_width("Delete"));
        {
            let _disabled = ui.begin_disabled_with_cond(data.selected.is_none());
            if ui.button("Delete")
                && let Some(id) = data.selected
            {
                actions.push(Action::Delete { id });
            }
        }
        let count = format!(
            "{} request{}",
            data.requests.len(),
            if data.requests.len() == 1 { "" } else { "s" }
        );
        same_line_if_fits(ui, ui.calc_text_size(&count)[0]);
        ui.align_text_to_frame_padding();
        ui.text_disabled(count);
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                if self.persistent {
                    "Saved in this workspace. Requests run on this computer."
                } else {
                    "Requests run on this computer."
                },
            );
        }
        if let Some(error) = &data.error {
            ui.text_colored(
                readable_color(ui, [0.95, 0.35, 0.3, 1.0]),
                fit_text(ui, error, ui.content_region_avail()[0]),
            );
            if ui.is_item_hovered() {
                detail_tooltip(ui, error);
            }
        }
        if let Some(active) = &data.active {
            if ui.small_button("Cancel") {
                actions.push(Action::Cancel);
            }
            let running = format!("Running {} {}…", active.request.method, active.request.url);
            same_line_if_fits(ui, fs * 8.0);
            ui.text_disabled(fit_text(ui, &running, ui.content_region_avail()[0]));
            if ui.is_item_hovered() {
                detail_tooltip(ui, &running);
            }
            requests.push(HostRequest::Invalidate);
        }
        ui.separator();
        // A narrow dock gets a picker; the split layout keeps both columns usable.
        let wide = ui.content_region_avail()[0] >= fs * 48.0;
        if !wide {
            let selected = data
                .selected
                .and_then(|id| data.requests.iter().find(|r| r.id == id));
            let width = ui.content_region_avail()[0].max(1.0);
            ui.set_next_item_width(width);
            let preview = selected.map_or("Select a request", request_name);
            if let Some(_combo) = ui.begin_combo(
                "##http-request-picker",
                fit_text(ui, preview, (width - fs * 2.5).max(1.0)),
            ) {
                for request in &data.requests {
                    if request_item(ui, request, data.selected == Some(request.id)) {
                        actions.push(Action::Select { id: request.id });
                    }
                }
            }
        }
        let size = ui.content_region_avail();
        let splitter = fs * 0.35;
        if wide {
            let list_width = (size[0] * self.state.list_fraction)
                .clamp(fs * 12.0, size[0] - fs * 30.0 - splitter - gap * 2.0);
            ui.child_window("http-requests")
                .size([list_width, size[1].max(1.0)])
                .border(true)
                .build(ui, || {
                    ui.text_disabled("Requests");
                    ui.separator();
                    let height = ui.text_line_height_with_spacing();
                    for index in ListClipper::new(data.requests.len())
                        .items_height(height)
                        .begin(ui)
                        .iter()
                    {
                        let request = &data.requests[index];
                        if request_item(ui, request, data.selected == Some(request.id)) {
                            actions.push(Action::Select { id: request.id });
                        }
                    }
                    if data.requests.is_empty() {
                        ui.text_wrapped("Create a request to get started.");
                    }
                });
            ui.same_line();
            ui.invisible_button("http-list-splitter", [splitter, size[1].max(1.0)]);
            if ui.is_item_hovered() || ui.is_item_active() {
                ui.set_mouse_cursor(Some(MouseCursor::ResizeEW));
            }
            if ui.is_item_active() {
                self.state.list_fraction = (self.state.list_fraction
                    + ui.io().mouse_delta()[0] / size[0].max(1.0))
                .clamp(0.15, 0.6);
            }
            ui.same_line();
        }
        ui.child_window("http-details")
            .size([0.0, size[1].max(1.0)])
            .build(ui, || {
                let Some(request) = data
                    .selected
                    .and_then(|id| data.requests.iter().find(|request| request.id == id))
                else {
                    self.draft = None;
                    self.response_cache = None;
                    ui.text_wrapped("Create a request to get started.");
                    return;
                };
                let _request_scope = ui.push_id(&format!("request-{}", request.id));
                if self
                    .draft
                    .as_ref()
                    .is_none_or(|draft| draft.request != *request)
                {
                    self.draft = Some(RequestDraft::new(request));
                }
                let draft = self.draft.as_mut().expect("selected request has a draft");
                let mut changed = false;
                let mut run = false;
                let split_height =
                    (ui.content_region_avail()[1] - splitter - style.item_spacing()[1] * 2.0)
                        .max(1.0);
                let min_request = (fs * 11.0).min(split_height * 0.6);
                let min_response = (fs * 9.0).min(split_height * 0.4);
                let request_height = (split_height * self.state.request_fraction)
                    .clamp(min_request, (split_height - min_response).max(min_request));
                ui.child_window("http-request-editor")
                    .size([0.0, request_height])
                    .border(true)
                    .build(ui, || {
                        ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                        changed |= ui
                            .input_text("##request-name", &mut draft.request.name)
                            .hint("Request name")
                            .build();
                        if ui.is_item_hovered() {
                            tooltip_text(ui, "Request name");
                        }
                        let width = ui.content_region_avail()[0].max(1.0);
                        let method_width = (fs * 7.0).min(width);
                        let inline =
                            width >= method_width + button_width("Run") + gap * 2.0 + fs * 12.0;
                        ui.set_next_item_width(method_width);
                        let method_label = if draft.custom_method {
                            "Custom"
                        } else {
                            &draft.request.method
                        };
                        if let Some(_combo) = ui.begin_combo("##request-method", method_label) {
                            for method in METHODS {
                                if ui
                                    .selectable_config(method)
                                    .selected(
                                        !draft.custom_method && draft.request.method == *method,
                                    )
                                    .build()
                                {
                                    draft.request.method = (*method).into();
                                    draft.custom_method = false;
                                    changed = true;
                                }
                            }
                            if ui
                                .selectable_config("Custom")
                                .selected(draft.custom_method)
                                .build()
                            {
                                draft.custom_method = true;
                            }
                        }
                        if inline {
                            ui.same_line();
                            ui.set_next_item_width(
                                (ui.content_region_avail()[0] - button_width("Run") - gap).max(1.0),
                            );
                            changed |= ui
                                .input_text("##request-url", &mut draft.request.url)
                                .hint("https://example.com")
                                .build();
                        }
                        same_line_if_fits(ui, button_width("Run"));
                        {
                            let _disabled = ui.begin_disabled_with_cond(
                                data.active.is_some() || model::resolve(&draft.request).is_err(),
                            );
                            let _accent = ui.push_style_color(
                                StyleColor::Button,
                                ui.style_color(StyleColor::Header),
                            );
                            if ui.button("Run") {
                                run = true;
                            }
                        }
                        if !inline {
                            ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                            changed |= ui
                                .input_text("##request-url", &mut draft.request.url)
                                .hint("https://example.com")
                                .build();
                        }
                        if draft.custom_method {
                            ui.text_disabled("Custom method");
                            ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                            changed |= ui
                                .input_text("##custom-method", &mut draft.request.method)
                                .build();
                        }
                        ui.separator();
                        for (index, (label, tab)) in [
                            ("Params", EditorTab::Params),
                            ("Headers", EditorTab::Headers),
                            ("Body", EditorTab::Body),
                            ("Auth", EditorTab::Auth),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if index != 0 {
                                same_line_if_fits(ui, button_width(label));
                            }
                            let _selected = (self.state.editor_tab == tab).then(|| {
                                ui.push_style_color(
                                    StyleColor::Button,
                                    ui.style_color(StyleColor::Header),
                                )
                            });
                            if ui.button(label) {
                                self.state.editor_tab = tab;
                            }
                        }
                        ui.separator();
                        match self.state.editor_tab {
                            EditorTab::Params => {
                                changed |= edit_rows(ui, "parameter", &mut draft.request.params)
                            }
                            EditorTab::Headers => {
                                changed |= edit_rows(ui, "header", &mut draft.request.headers)
                            }
                            EditorTab::Body => {
                                ui.text_disabled("Raw request body");
                                if ui
                                    .input_text_multiline_imstr(
                                        "##request-body",
                                        &mut draft.body,
                                        [
                                            ui.content_region_avail()[0].max(1.0),
                                            ui.content_region_avail()[1]
                                                .max(ui.frame_height() * 2.0),
                                        ],
                                    )
                                    .build()
                                {
                                    draft.request.body = draft.body.to_str().to_owned();
                                    changed = true;
                                }
                            }
                            EditorTab::Auth => {
                                let mut mode = match draft.request.auth {
                                    Auth::None => 0,
                                    Auth::Bearer { .. } => 1,
                                    Auth::Basic { .. } => 2,
                                };
                                ui.text_disabled("Authentication");
                                ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                                if ui.combo_simple_string(
                                    "##authentication",
                                    &mut mode,
                                    &["None", "Bearer token", "Basic"],
                                ) {
                                    draft.request.auth = match mode {
                                        1 => Auth::Bearer {
                                            token: String::new(),
                                        },
                                        2 => Auth::Basic {
                                            username: String::new(),
                                            password: String::new(),
                                        },
                                        _ => Auth::None,
                                    };
                                    changed = true;
                                }
                                match &mut draft.request.auth {
                                    Auth::None => ui.text_wrapped(
                                        "Use headers for other authentication schemes.",
                                    ),
                                    Auth::Bearer { token } => {
                                        ui.text_disabled("Token");
                                        ui.set_next_item_width(
                                            ui.content_region_avail()[0].max(1.0),
                                        );
                                        changed |=
                                            ui.input_text("##token", token).password(true).build()
                                    }
                                    Auth::Basic { username, password } => {
                                        ui.text_disabled("Username");
                                        ui.set_next_item_width(
                                            ui.content_region_avail()[0].max(1.0),
                                        );
                                        changed |= ui.input_text("##username", username).build();
                                        ui.text_disabled("Password");
                                        ui.set_next_item_width(
                                            ui.content_region_avail()[0].max(1.0),
                                        );
                                        changed |= ui
                                            .input_text("##password", password)
                                            .password(true)
                                            .build();
                                    }
                                }
                            }
                        }
                    });
                let resolved = model::resolve(&draft.request);
                if changed {
                    actions.push(Action::Update {
                        request: draft.request.clone(),
                    });
                }
                // Preserve ordering when an edit and Run occur in the same frame.
                if run && resolved.is_ok() {
                    actions.push(Action::Run { id: request.id });
                }
                ui.invisible_button(
                    "http-response-splitter",
                    [ui.content_region_avail()[0].max(1.0), splitter],
                );
                if ui.is_item_hovered() || ui.is_item_active() {
                    ui.set_mouse_cursor(Some(MouseCursor::ResizeNS));
                }
                if ui.is_item_active() {
                    self.state.request_fraction = (self.state.request_fraction
                        + ui.io().mouse_delta()[1] / size[1].max(1.0))
                    .clamp(0.15, 0.8);
                }
                ui.child_window("http-response")
                    .size([0.0, 0.0])
                    .border(true)
                    .build(ui, || {
                        ui.text("Response");
                        if let Err(error) = &resolved {
                            ui.text_wrapped(error);
                        }
                        let Some(result) = data.results.get(&request.id) else {
                            self.response_cache = None;
                            ui.text_wrapped("Run this request to inspect its response.");
                            return;
                        };
                        if !matches!(&resolved, Ok(current) if *current == result.request) {
                            ui.text_wrapped("Previous response · request inputs have changed");
                        }
                        match &result.outcome {
                            Err(error) => {
                                self.response_cache = None;
                                ui.text_wrapped(error);
                            }
                            Ok(response) => {
                                if matches!(&resolved, Ok(current) if *current == result.request) {
                                    same_line_if_fits(
                                        ui,
                                        ui.calc_text_size(response.status.to_string())[0],
                                    );
                                }
                                let status_color = match response.status {
                                    200..=299 => [0.4, 0.75, 0.5, 1.0],
                                    300..=399 => [0.4, 0.65, 0.95, 1.0],
                                    400..=499 => [0.95, 0.7, 0.3, 1.0],
                                    _ => [0.95, 0.35, 0.3, 1.0],
                                };
                                ui.text_colored(
                                    readable_color(ui, status_color),
                                    response.status.to_string(),
                                );
                                if ui.is_item_hovered() {
                                    tooltip_text(
                                        ui,
                                        format!("{} {}", response.version, response.status),
                                    );
                                }
                                let timing =
                                    format!("{:.0} ms", response.elapsed.as_secs_f64() * 1000.0);
                                same_line_if_fits(ui, ui.calc_text_size(&timing)[0]);
                                ui.text_disabled(timing);
                                let bytes = format!(
                                    "{} bytes{}",
                                    response.bytes.len(),
                                    if response.truncated {
                                        " · truncated"
                                    } else {
                                        ""
                                    }
                                );
                                same_line_if_fits(ui, ui.calc_text_size(&bytes)[0]);
                                ui.text_disabled(fit_text(
                                    ui,
                                    &bytes,
                                    ui.content_region_avail()[0],
                                ));
                                if response.truncated && ui.is_item_hovered() {
                                    tooltip_text(ui, "Response collection stopped at 8 MiB.");
                                }
                                ui.text_disabled(fit_text(
                                    ui,
                                    &response.final_url,
                                    ui.content_region_avail()[0],
                                ));
                                if ui.is_item_hovered() {
                                    detail_tooltip(ui, &response.final_url);
                                }
                                // Keep copy actions in one menu so the body keeps its space.
                                if ui.small_button("Copy") {
                                    ui.open_popup("http-copy");
                                }
                                if let Some(_popup) = ui.begin_popup("http-copy") {
                                    if let BodyPresentation::Text { raw, .. } =
                                        &response.presentation
                                        && ui.menu_item("Response body")
                                    {
                                        copy(ui, raw);
                                    }
                                    if ui.menu_item("Response headers") {
                                        copy(ui, &header_text(&response.headers));
                                    }
                                    if ui.menu_item("Request") {
                                        copy(ui, &request_text(&result.request));
                                    }
                                }
                                for (label, tab) in [
                                    ("Formatted", ResponseTab::Formatted),
                                    ("Raw", ResponseTab::Raw),
                                    ("Headers", ResponseTab::Headers),
                                ] {
                                    same_line_if_fits(ui, button_width(label));
                                    let _selected = (self.state.response_tab == tab).then(|| {
                                        ui.push_style_color(
                                            StyleColor::Button,
                                            ui.style_color(StyleColor::Header),
                                        )
                                    });
                                    if ui.button(label) {
                                        self.state.response_tab = tab;
                                    }
                                }
                                ui.separator();
                                if self.response_cache.as_ref().is_none_or(|cache| {
                                    cache.run != result.run || cache.tab != self.state.response_tab
                                }) {
                                    let text = if self.state.response_tab == ResponseTab::Headers {
                                        header_text(&response.headers)
                                    } else {
                                        match &response.presentation {
                                            BodyPresentation::Text { raw, pretty_json } => {
                                                if self.state.response_tab == ResponseTab::Formatted
                                                {
                                                    pretty_json.as_deref().unwrap_or(raw).to_owned()
                                                } else {
                                                    raw.clone()
                                                }
                                            }
                                            BodyPresentation::Binary { hex } => hex.clone(),
                                        }
                                    };
                                    self.response_cache = Some(ResponseCache {
                                        run: result.run,
                                        tab: self.state.response_tab,
                                        text: ImString::new(text.replace('\0', "�")),
                                    });
                                }
                                if self.state.response_tab != ResponseTab::Headers
                                    && matches!(
                                        response.presentation,
                                        BodyPresentation::Binary { .. }
                                    )
                                {
                                    ui.text_wrapped("Binary response · first 4 KiB in hex");
                                }
                                let cache = self
                                    .response_cache
                                    .as_mut()
                                    .expect("response text was cached");
                                if cache.text.is_empty() {
                                    ui.text_disabled("Empty response body");
                                }
                                // Zero-copy backing keeps large responses selectable without cloning every frame.
                                ui.input_text_multiline_imstr(
                                    format!(
                                        "##http-response-text-{}-{}",
                                        result.run, self.state.response_tab as u8
                                    ),
                                    &mut cache.text,
                                    [
                                        ui.content_region_avail()[0].max(1.0),
                                        ui.content_region_avail()[1].max(1.0),
                                    ],
                                )
                                .read_only(true)
                                .build();
                            }
                        }
                    });
            });
        drop(data);
        if !actions.is_empty() {
            shared.borrow_mut().actions.extend(actions);
            requests.push(HostRequest::Invalidate);
        }
    }

    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> std::io::Result<()> {
        self.persistent = services.documents.options().project_root.is_some();
        self.draw(ui, host, requests);
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn edit_rows(ui: &Ui, label: &str, rows: &mut Vec<KeyValue>) -> bool {
    let fs = ui.current_font_size();
    let style = ui.clone_style();
    let gap = style.item_spacing()[0];
    let remove_width = ui.calc_text_size("×")[0] + style.frame_padding()[0] * 2.0;
    let mut changed = false;
    let mut removed = None;
    for (index, row) in rows.iter_mut().enumerate() {
        let _id = ui.push_id(&format!("{label}-{index}"));
        let width = ui.content_region_avail()[0].max(1.0);
        let inline = width >= fs * 28.0;
        changed |= ui.checkbox("##enabled", &mut row.enabled);
        if ui.is_item_hovered() {
            tooltip_text(ui, format!("Include this {label}"));
        }
        ui.same_line();
        let fields_width = (ui.content_region_avail()[0] - remove_width - gap).max(1.0);
        ui.set_next_item_width(if inline {
            (fields_width - gap) * 0.4
        } else {
            fields_width
        });
        changed |= ui.input_text("##name", &mut row.name).hint("Name").build();
        if inline {
            ui.same_line();
            ui.set_next_item_width((ui.content_region_avail()[0] - remove_width - gap).max(1.0));
            changed |= ui
                .input_text("##value", &mut row.value)
                .hint("Value")
                .build();
        }
        ui.same_line();
        if ui.button("×") {
            removed = Some(index);
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, format!("Remove {label}"));
        }
        if !inline {
            ui.set_next_item_width(width);
            changed |= ui
                .input_text("##value", &mut row.value)
                .hint("Value")
                .build();
        }
    }
    if let Some(index) = removed {
        rows.remove(index);
        changed = true;
    }
    if ui.small_button(format!("Add {label}")) {
        rows.push(KeyValue::default());
        changed = true;
    }
    changed
}

fn request_name(request: &Request) -> &str {
    if request.name.is_empty() {
        "Untitled request"
    } else {
        &request.name
    }
}

fn request_item(ui: &Ui, request: &Request, selected: bool) -> bool {
    let label = fit_text(ui, request_name(request), ui.content_region_avail()[0]);
    let position = ui.cursor_screen_pos();
    let clicked = ui
        .selectable_config(format!("##http-request-{}", request.id))
        .size([ui.content_region_avail()[0], ui.text_line_height()])
        .selected(selected)
        .build();
    // Draw the name literally, including any ImGui label delimiters in a saved name.
    ui.get_window_draw_list()
        .add_text(position, ui.style_color(StyleColor::Text), label);
    if ui.is_item_hovered() {
        detail_tooltip(
            ui,
            &format!(
                "{}\n{} {}",
                request_name(request),
                request.method,
                request.url
            ),
        );
    }
    clicked
}

fn detail_tooltip(ui: &Ui, text: &str) {
    tooltip(ui, || {
        let _wrap = ui.push_text_wrap_pos(ui.current_font_size() * 36.0);
        ui.text(text);
    });
}

fn finite_fraction(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.15, 0.8)
    } else {
        default
    }
}

fn request_text(request: &ResolvedRequest) -> String {
    let mut text = format!("{} {}\n", request.method, request.url);
    for (name, value) in &request.headers {
        text.push_str(name);
        text.push_str(": ");
        text.push_str(value);
        text.push('\n');
    }
    text.push('\n');
    text.push_str(&request.body);
    text
}

fn header_text(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy(ui: &Ui, text: &str) {
    let text = CString::new(text.replace('\0', "�")).expect("NUL characters were replaced");
    ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igSetClipboardText(text.as_ptr());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunResult, worker::Response};
    use dear_imgui_rs::{
        ClipboardBackend, Condition, Context, FramePrepareOptions, Key, MouseButton,
    };
    use serde_json::json;
    use std::{collections::HashMap, sync::Mutex, time::Duration};

    static IMGUI_LOCK: Mutex<()> = Mutex::new(());
    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, value: &str) {
            *self.0.borrow_mut() = value.to_owned();
        }
    }

    fn fixture() -> Rc<RefCell<Shared>> {
        let mut request = Request::new(1);
        request.name = "Example".into();
        request.url = "http://localhost/test".into();
        request.auth = Auth::Bearer {
            token: "saved-secret".into(),
        };
        Rc::new(RefCell::new(Shared {
            requests: vec![request],
            selected: Some(1),
            ..Shared::default()
        }))
    }

    #[test]
    fn restoration_saves_only_presentation_and_never_creates_or_runs_requests() {
        let shared = fixture();
        let panel = HttpPanel::new(
            shared.clone(),
            &json!({"list_fraction": 0.3, "request_fraction": 0.7, "editor_tab": "Auth", "response_tab": "Headers", "selected": 55, "token": "untrusted-secret"}),
        );
        let saved = panel.save_state();
        let restored = HttpPanel::new(shared.clone(), &saved);
        assert_eq!(restored.save_state(), saved);
        assert_eq!(saved.as_object().unwrap().len(), 4);
        assert!(!saved.to_string().contains("secret"));
        assert_eq!(restored.attached_document(), None);
        assert_eq!(shared.borrow().selected, Some(1));
        assert!(shared.borrow().actions.is_empty());
        assert!(shared.borrow().active.is_none());
        let invalid = HttpPanel::new(
            shared,
            &json!({"list_fraction": -50.0, "request_fraction": 1e30}),
        );
        assert_eq!(invalid.save_state()["list_fraction"], json!(0.15_f32));
        assert_eq!(invalid.save_state()["request_fraction"], json!(0.8_f32));
    }

    fn frame(context: &mut Context, panel: &mut HttpPanel) -> usize {
        frame_at_size(context, panel, [1000.0, 800.0])
    }

    fn frame_at_size(context: &mut Context, panel: &mut HttpPanel, size: [f32; 2]) -> usize {
        context.prepare_frame(FramePrepareOptions::new([1000.0, 800.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("HTTP fixture")
            .position([0.0, 0.0], Condition::Always)
            .size(size, Condition::Always)
            .build(|| {
                panel.draw(
                    ui,
                    &HostContext {
                        documents: &[],
                        active_document: None,
                        settings: &Value::Null,
                        textures: &HashMap::new(),
                        animations: false,
                        remote: false,
                        workspace: 1,
                        diagnostics: &Value::Null,
                        viewer_menu: None,
                        default_viewers: &Value::Null,
                    },
                    &mut Vec::new(),
                );
            });
        context.render_legacy().total_vtx_count()
    }

    #[test]
    fn resizing_keeps_forms_and_response_inside_the_panel() {
        use dear_imgui_rs::sys;
        use std::ffi::CStr;

        let _lock = IMGUI_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let shared = fixture();
        {
            let mut data = shared.borrow_mut();
            let request = &mut data.requests[0];
            request.name =
                "A saved request with a very long name and Unicode characters · 日本語".into();
            request.url =
                "https://example.com/a/very/long/path/that/should/not/expand/the/panel".into();
            request.method = "CUSTOM".into();
            request.auth = Auth::Basic {
                username: "example".into(),
                password: "saved-secret".into(),
            };
            request.params = vec![KeyValue {
                enabled: true,
                name: "expand".into(),
                value: "members,permissions".into(),
            }];
            request.headers = vec![KeyValue {
                enabled: false,
                name: "content-type".into(),
                value: "application/json".into(),
            }];
            let resolved = model::resolve(request).unwrap();
            data.active = Some(crate::ActiveRun {
                run: 2,
                id: 1,
                request: resolved.clone(),
            });
            data.error = Some("An error with a long explanation\nand another line of details that must leave room for the request editor.".into());
            data.results.insert(1, RunResult {
                run: 1,
                request: resolved,
                outcome: Ok(Response {
                    status: 404,
                    version: "HTTP/1.1".into(),
                    final_url: "https://example.com/a/very/long/redirected/path/that/should/not/expand/the/panel".into(),
                    elapsed: Duration::from_millis(125),
                    headers: vec![("content-type".into(), "application/json".into())],
                    bytes: b"{}".to_vec(),
                    truncated: true,
                    presentation: BodyPresentation::Text { raw: "{}".into(), pretty_json: None },
                }),
            });
        }
        let original = shared.borrow().requests.clone();
        let mut panel = HttpPanel::new(shared.clone(), &Value::Null);
        for size in [
            [240.0, 600.0],
            [320.0, 320.0],
            [480.0, 260.0],
            [700.0, 240.0],
            [1000.0, 800.0],
        ] {
            for tab in [
                EditorTab::Params,
                EditorTab::Headers,
                EditorTab::Body,
                EditorTab::Auth,
            ] {
                panel.state.editor_tab = tab;
                for response_tab in [
                    ResponseTab::Formatted,
                    ResponseTab::Raw,
                    ResponseTab::Headers,
                ] {
                    panel.state.response_tab = response_tab;
                    // ImGui computes scroll ranges from the preceding frame's content.
                    for _ in 0..3 {
                        assert!(frame_at_size(&mut context, &mut panel, size) > 0);
                    }
                    context.binding().with_bound_context(|| unsafe {
                        let native = &*sys::igGetCurrentContext();
                        let mut panes = 0;
                        for index in 0..native.Windows.Size as usize {
                            let window = &**native.Windows.Data.add(index);
                            let name = CStr::from_ptr(window.Name).to_string_lossy();
                            let local_name = name.rsplit('/').next().unwrap();
                            let is_pane = [
                                "http-requests_",
                                "http-details_",
                                "http-request-editor_",
                                "http-response_",
                            ]
                            .iter()
                            .any(|prefix| local_name.starts_with(prefix));
                            if !window.Active || (name != "HTTP fixture" && !is_pane) {
                                continue;
                            }
                            assert!(
                                window.ScrollMax.x <= 1.0,
                                "Horizontal overflow in {name}: {}, size {size:?}, tab {}",
                                window.ScrollMax.x,
                                tab as u8
                            );
                            if local_name.starts_with("http-request-editor_")
                                || local_name.starts_with("http-response_")
                            {
                                panes += 1;
                                assert!(
                                    window.Size.y >= 30.0,
                                    "Both panes must retain usable height"
                                );
                                let parent = &*window.ParentWindow;
                                assert!(window.Pos.x >= parent.InnerRect.Min.x);
                                assert!(window.Pos.y >= parent.InnerRect.Min.y);
                                assert!(
                                    window.Pos.x + window.Size.x <= parent.InnerRect.Max.x + 1.0
                                );
                                assert!(
                                    window.Pos.y + window.Size.y <= parent.InnerRect.Max.y + 1.0,
                                    "Pane extends past the panel: {name}, size {size:?}"
                                );
                            }
                        }
                        assert_eq!(panes, 2, "Request and response must remain visible");
                    });
                }
            }
        }
        assert_eq!(shared.borrow().requests, original);
        assert!(shared.borrow().actions.is_empty());
    }

    fn click(context: &mut Context, panel: &mut HttpPanel, point: [f32; 2]) {
        context.io_mut().add_mouse_pos_event(point);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(context, panel);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(context, panel);
    }

    fn key(context: &mut Context, panel: &mut HttpPanel, key: Key) {
        context.io_mut().add_key_event(key, true);
        frame(context, panel);
        context.io_mut().add_key_event(key, false);
        frame(context, panel);
    }

    #[test]
    fn native_draw_keeps_requests_unchanged_and_response_values_selectable() {
        let _lock = IMGUI_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.io_mut().set_config_macosx_behaviors(false);
        let clipboard = Rc::new(RefCell::new(String::new()));
        context.set_clipboard_backend(Clipboard(clipboard.clone()));
        let shared = fixture();
        let original = shared.borrow().requests.clone();
        let resolved = model::resolve(&original[0]).unwrap();
        shared.borrow_mut().results.insert(
            1,
            RunResult {
                run: 1,
                request: resolved,
                outcome: Ok(Response {
                    status: 404,
                    version: "HTTP/1.1".into(),
                    final_url: "http://localhost/test".into(),
                    elapsed: Duration::from_millis(5),
                    headers: vec![("content-type".into(), "application/json".into())],
                    bytes: br#"{"message":"missing"}"#.to_vec(),
                    truncated: false,
                    presentation: BodyPresentation::Text {
                        raw: "{\"message\":\"missing\"}".into(),
                        pretty_json: Some("{\n  \"message\": \"missing\"\n}".into()),
                    },
                }),
            },
        );
        let mut panel = HttpPanel::new(shared.clone(), &Value::Null);
        for tab in [
            EditorTab::Params,
            EditorTab::Headers,
            EditorTab::Body,
            EditorTab::Auth,
        ] {
            panel.state.editor_tab = tab;
            assert!(frame(&mut context, &mut panel) > 0);
        }
        for tab in [
            ResponseTab::Formatted,
            ResponseTab::Headers,
            ResponseTab::Raw,
        ] {
            panel.state.response_tab = tab;
            assert!(frame(&mut context, &mut panel) > 0);
        }
        click(&mut context, &mut panel, [400.0, 650.0]);
        key(&mut context, &mut panel, Key::Home);
        for _ in 0..2 {
            key(&mut context, &mut panel, Key::RightArrow);
        }
        context.io_mut().add_key_event(Key::ModShift, true);
        for _ in 0..7 {
            key(&mut context, &mut panel, Key::RightArrow);
        }
        context.io_mut().add_key_event(Key::ModShift, false);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        key(&mut context, &mut panel, Key::C);
        context.io_mut().add_key_event(Key::ModCtrl, false);
        frame(&mut context, &mut panel);
        assert_eq!(&*clipboard.borrow(), "message");
        context.io_mut().add_input_characters_utf8("overwrite");
        frame(&mut context, &mut panel);
        assert_eq!(
            panel.response_cache.as_ref().unwrap().text.to_str(),
            "{\"message\":\"missing\"}"
        );
        assert_eq!(shared.borrow().requests, original);
        assert!(shared.borrow().actions.is_empty());
        assert!(shared.borrow().active.is_none());
        assert!(!panel.save_state().to_string().contains("saved-secret"));
    }

    #[test]
    fn native_new_and_run_clicks_queue_intentions_without_changing_saved_requests() {
        let _lock = IMGUI_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let shared = fixture();
        let original = shared.borrow().requests.clone();
        let mut panel = HttpPanel::new(shared.clone(), &Value::Null);
        frame(&mut context, &mut panel);
        frame(&mut context, &mut panel);
        'new: for y in (45..90).step_by(8) {
            for x in (20..55).step_by(8) {
                click(&mut context, &mut panel, [x as f32, y as f32]);
                if !shared.borrow().actions.is_empty() {
                    break 'new;
                }
            }
        }
        assert!(matches!(
            shared.borrow().actions.as_slice(),
            [Action::Create]
        ));
        shared.borrow_mut().actions.clear();
        'run: for y in (50..150).step_by(8) {
            for x in (920..975).step_by(8) {
                click(&mut context, &mut panel, [x as f32, y as f32]);
                if !shared.borrow().actions.is_empty() {
                    break 'run;
                }
            }
        }
        assert!(matches!(
            shared.borrow().actions.as_slice(),
            [Action::Run { id: 1 }]
        ));
        assert_eq!(shared.borrow().requests, original);
        assert!(shared.borrow().active.is_none());
    }
}
