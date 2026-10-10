use crate::{
    Action, Shared,
    model::{self, Auth, KeyValue, Request, ResolvedRequest},
    worker::BodyPresentation,
};
use bed_workbench_api::{HostContext, HostRequest, ModulePanel, ModuleServices};
use dear_imgui_rs::{ImString, ListClipper, MouseCursor, Ui, WindowFlags};
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
        let shared = Rc::clone(&self.shared);
        let data = shared.borrow();
        let mut actions = Vec::new();
        ui.text("HTTP Client");
        ui.same_line();
        ui.text_disabled(if self.persistent {
            "Saved in this workspace · Requests run on this computer"
        } else {
            "Requests run on this computer"
        });
        if let Some(error) = &data.error {
            ui.text_wrapped(error);
        }
        if let Some(active) = &data.active {
            if ui.small_button("Cancel") {
                actions.push(Action::Cancel);
            }
            ui.same_line();
            ui.text_wrapped(format!(
                "Running {} {}…",
                active.request.method, active.request.url
            ));
            requests.push(HostRequest::Invalidate);
        }
        ui.separator();
        let size = ui.content_region_avail();
        let gap = ui.clone_style().item_spacing()[0];
        let list_width = ((size[0] - 6.0 - gap * 2.0) * self.state.list_fraction).max(1.0);
        ui.child_window("http-requests")
            .size([list_width, size[1]])
            .border(true)
            .build(ui, || {
                if ui.button("New") {
                    actions.push(Action::Create);
                }
                ui.same_line();
                {
                    let _disabled = ui.begin_disabled_with_cond(data.selected.is_none());
                    if ui.button("Delete")
                        && let Some(id) = data.selected
                    {
                        actions.push(Action::Delete { id });
                    }
                }
                ui.separator();
                let height = ui.text_line_height_with_spacing();
                for index in ListClipper::new(data.requests.len())
                    .items_height(height)
                    .begin(ui)
                    .iter()
                {
                    let request = &data.requests[index];
                    let name = if request.name.is_empty() {
                        "Untitled request"
                    } else {
                        &request.name
                    };
                    if ui
                        .selectable_config(format!("{name}##http-request-{}", request.id))
                        .selected(data.selected == Some(request.id))
                        .build()
                    {
                        actions.push(Action::Select { id: request.id });
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text(format!("{} {}", request.method, request.url));
                    }
                }
            });
        ui.same_line();
        ui.invisible_button("http-list-splitter", [6.0, size[1].max(1.0)]);
        if ui.is_item_hovered() || ui.is_item_active() {
            ui.set_mouse_cursor(Some(MouseCursor::ResizeEW));
        }
        if ui.is_item_active() {
            self.state.list_fraction = (self.state.list_fraction
                + ui.io().mouse_delta()[0] / size[0].max(1.0))
            .clamp(0.15, 0.6);
        }
        ui.same_line();
        ui.child_window("http-details")
            .size([0.0, size[1]])
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
                let request_height =
                    ((ui.content_region_avail()[1] - 12.0) * self.state.request_fraction).max(1.0);
                ui.child_window("http-request-editor")
                    .size([0.0, request_height])
                    .border(true)
                    .build(ui, || {
                        ui.text_disabled("Name");
                        ui.same_line();
                        ui.set_next_item_width(ui.content_region_avail()[0].max(1.0));
                        changed |= ui
                            .input_text("##request-name", &mut draft.request.name)
                            .build();
                        ui.set_next_item_width(110.0);
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
                        ui.same_line();
                        ui.set_next_item_width(
                            (ui.content_region_avail()[0] - 50.0 - gap).max(40.0),
                        );
                        changed |= ui
                            .input_text("##request-url", &mut draft.request.url)
                            .hint("https://example.com")
                            .build();
                        ui.same_line();
                        {
                            let _disabled = ui.begin_disabled_with_cond(
                                data.active.is_some() || model::resolve(&draft.request).is_err(),
                            );
                            if ui.button("Run") {
                                run = true;
                            }
                        }
                        if draft.custom_method {
                            ui.set_next_item_width(160.0);
                            changed |= ui.input_text("Method", &mut draft.request.method).build();
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
                                ui.same_line();
                            }
                            if ui.radio_button_bool(label, self.state.editor_tab == tab) {
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
                                            ui.content_region_avail()[1].max(40.0),
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
                                if ui.combo_simple_string(
                                    "Authentication",
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
                                    Auth::None => ui.text_disabled(
                                        "Use headers for other authentication schemes.",
                                    ),
                                    Auth::Bearer { token } => {
                                        changed |=
                                            ui.input_text("Token", token).password(true).build()
                                    }
                                    Auth::Basic { username, password } => {
                                        changed |= ui.input_text("Username", username).build();
                                        changed |= ui
                                            .input_text("Password", password)
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
                    [ui.content_region_avail()[0].max(1.0), 6.0],
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
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || {
                        ui.text("Response");
                        if let Err(error) = &resolved {
                            ui.text_wrapped(error);
                        }
                        let Some(result) = data.results.get(&request.id) else {
                            self.response_cache = None;
                            ui.text_disabled("Run this request to inspect its response.");
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
                                ui.text(format!(
                                    "{} {} · {:.0} ms · {} bytes{}",
                                    response.version,
                                    response.status,
                                    response.elapsed.as_secs_f64() * 1000.0,
                                    response.bytes.len(),
                                    if response.truncated {
                                        " collected (truncated at 8 MiB)"
                                    } else {
                                        ""
                                    }
                                ));
                                ui.text_wrapped(&response.final_url);
                                if let BodyPresentation::Text { raw, .. } = &response.presentation {
                                    if ui.small_button("Copy response") {
                                        copy(ui, raw);
                                    }
                                    ui.same_line();
                                }
                                if ui.small_button("Copy headers") {
                                    copy(ui, &header_text(&response.headers));
                                }
                                ui.same_line();
                                if ui.small_button("Copy request") {
                                    copy(ui, &request_text(&result.request));
                                }
                                for (index, (label, tab)) in [
                                    ("Formatted", ResponseTab::Formatted),
                                    ("Raw", ResponseTab::Raw),
                                    ("Response headers", ResponseTab::Headers),
                                ]
                                .into_iter()
                                .enumerate()
                                {
                                    if index != 0 {
                                        ui.same_line();
                                    }
                                    if ui.radio_button_bool(label, self.state.response_tab == tab) {
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
                                    ui.text_disabled("Binary response · first 4 KiB in hex");
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
    let mut changed = false;
    let mut removed = None;
    for (index, row) in rows.iter_mut().enumerate() {
        let _id = ui.push_id(&format!("{label}-{index}"));
        changed |= ui.checkbox("##enabled", &mut row.enabled);
        ui.same_line();
        let width = ((ui.content_region_avail()[0] - 75.0) * 0.4).max(40.0);
        ui.set_next_item_width(width);
        changed |= ui.input_text("##name", &mut row.name).hint("Name").build();
        ui.same_line();
        ui.set_next_item_width((ui.content_region_avail()[0] - 65.0).max(40.0));
        changed |= ui
            .input_text("##value", &mut row.value)
            .hint("Value")
            .build();
        ui.same_line();
        if ui.small_button("Remove") {
            removed = Some(index);
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
        context.prepare_frame(FramePrepareOptions::new([1000.0, 800.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("HTTP fixture")
            .position([0.0, 0.0], Condition::Always)
            .size([1000.0, 800.0], Condition::Always)
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
