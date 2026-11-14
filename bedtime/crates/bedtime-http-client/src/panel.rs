//! The HTTP client panel: the workspace's saved requests down the side, the
//! selected one's editor (method, URL, params, headers, body) and its last
//! response (status, time, size, body and headers).
//!
//! The requests (`HttpRequests`) are saved in the workspace's store. Responses
//! (`Exchanges`) live only in memory. Drawing edits a copy of the requests,
//! written back if it changed; adding, deleting and sending are intents,
//! applied in `Update`.
use crate::{
    Collection, Error, Field, Method, Options, Response, SavedRequest,
    content::{self, Content, Kind},
    json, send,
};
use bedtime_ui::{
    BuiltinIcon, CodeView, UiContext,
    data::{DataTree, Value},
    egui,
    hex::HexViewer,
    image::{Decoded, ImageView},
    theme,
};
use bedtime_workbench::{
    ManagedTask, ManagedWork, Panel, PanelPresentation, WorkStatus,
    persistence::{Persisted, PersistenceAppExt, ReflectPersist, WorkbenchStore},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Open an HTTP client: a new one, in the focused area.
pub struct ShowHttpClient;
impl bedtime_workbench::Action for ShowHttpClient {
    const KEY: &'static str = "http_client.show";
    const LABEL: &'static str = "HTTP Client";
}

pub struct HttpClientPlugin;

impl Plugin for HttpClientPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{ActionAppExt, MenusAppExt, status::StatusAppExt};
        app.register_type::<HttpClientPanel>()
            .add_action_handler::<ShowHttpClient>(bedtime_workbench::Handler::new(
                bedtime_workbench::open_new::<HttpClientPanel>,
            ))
            .add_menu_item::<ShowHttpClient>(bedtime_workbench::StandardMenu::View, "panels")
            .init_resource::<HttpRequests>()
            .persist::<HttpRequests, WorkbenchStore>()
            .init_resource::<Exchanges>()
            .init_resource::<Intents>()
            .add_panel_status::<HttpClientPanel>(crate::status::item)
            .add_systems(
                Update,
                (apply_intents, collect_responses)
                    .chain()
                    .in_set(HttpClientSystems),
            )
            // The requests are the workspace's (saved in its store); the next
            // workspace's are loaded after this.
            .add_systems(
                bedtime_workbench::OnWorkspaceClose,
                |mut commands: Commands| {
                    commands.insert_resource(HttpRequests::default());
                    commands.insert_resource(Exchanges::default());
                },
            );
    }
}

/// Doing what the panels were asked, then collecting responses, in `Update`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpClientSystems;

/// The workspace's requests and variables, saved in its store
/// (`http_requests.ron`).
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Persisted)]
#[persisted(key = "http_requests")]
#[serde(transparent)]
pub struct HttpRequests(pub Collection);

/// The HTTP client panel. Which request it shows is saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
#[require(HttpClientView)]
pub struct HttpClientPanel {
    /// The request shown, by id.
    pub selected: Option<u64>,
    /// The variables show instead of a request.
    pub variables: bool,
}

/// A panel's tabs and prompts. Not saved.
#[derive(Component, Default)]
pub struct HttpClientView {
    request_tab: RequestTab,
    response_tab: ResponseTab,
    /// How a JSON body shows: indented, as it came, or as a tree.
    json: JsonMode,
    /// Don't wrap the body's long lines.
    no_wrap: bool,
    /// The body shown, coloured, kept while it's the same response.
    body: Option<BodyView>,
    /// An image body shown, by response serial.
    image: Option<(u64, ImageView)>,
    /// A JSON body's tree, by response serial.
    tree: Option<(u64, DataTree)>,
    /// A binary body's bytes, by response serial.
    hex: Option<(u64, HexViewer)>,
    /// Asking whether to delete this request.
    confirm_delete: Option<u64>,
    focus_url: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum JsonMode {
    #[default]
    Indented,
    Raw,
    Tree,
}

struct BodyView {
    serial: u64,
    pretty: bool,
    view: CodeView,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RequestTab {
    #[default]
    Params,
    Headers,
    Body,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ResponseTab {
    #[default]
    Body,
    Headers,
}

/// Each request's last exchange, by request id. Not saved.
#[derive(Resource, Default)]
pub struct Exchanges(HashMap<u64, Exchange>);

impl Exchanges {
    pub fn get(&self, request: u64) -> Option<&Exchange> {
        self.0.get(&request)
    }
    fn sending(&self) -> bool {
        self.0
            .values()
            .any(|e| matches!(e, Exchange::Sending { .. }))
    }
}

pub enum Exchange {
    Sending {
        task: ManagedTask<Result<Received, Error>>,
        started: Instant,
    },
    Done(Result<Box<Received>, Error>),
}

/// A response, with its body made ready to show (on the request's thread).
pub struct Received {
    pub response: Response,
    /// What the body is (its text is moved to `text`).
    pub content: Content,
    /// The body as text, if it's text, decoded from its charset.
    pub text: Option<Shown>,
    /// The body indented, if it's JSON.
    pub pretty: Option<Shown>,
    /// A JSON body as a tree.
    pub tree: Option<Arc<Value>>,
    /// An image body's pixels, or why they couldn't be read.
    pub image: Option<Result<Arc<Decoded>, String>>,
    /// Unique to this response, to tell when the one shown changes.
    pub serial: u64,
}

/// Text to show, cut to `SHOWN` bytes.
pub struct Shown {
    pub text: String,
    pub clipped: bool,
}

/// At most this much of a body is shown.
const SHOWN: usize = 1 << 20;
/// Bodies larger than this aren't indented.
const PRETTY_MAX: usize = 8 << 20;

impl Received {
    fn new(response: Response) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        let mut content = content::decode(response.header("content-type"), &response.body);
        let full = content.text.take();
        let text = full.as_deref().map(Shown::new);
        let small = full.filter(|text| text.len() <= PRETTY_MAX);
        let pretty = small
            .as_deref()
            .and_then(|text| json::pretty(text.as_bytes()))
            .map(|pretty| Shown::new(&pretty));
        let tree = small
            .filter(|_| pretty.is_some())
            .and_then(|text| bedtime_ui::data::parse(bedtime_ui::data::Format::Json, &text).ok())
            .map(Arc::new);
        let image = (content.kind == Kind::Image)
            .then(|| bedtime_ui::image::decode(&response.body).map(Arc::new));
        Self {
            response,
            content,
            text,
            pretty,
            tree,
            image,
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl Shown {
    fn new(text: &str) -> Self {
        let mut end = text.len().min(SHOWN);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].to_owned(),
            clipped: end < text.len(),
        }
    }
}

/// What the user asked for while the panel drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Intent {
    New,
    Duplicate(u64),
    Delete(u64),
    Send(u64),
    Cancel(u64),
}

#[derive(Resource, Default)]
struct Intents(Vec<(Entity, Intent)>);

fn apply_intents(world: &mut World) {
    let intents = std::mem::take(&mut world.resource_mut::<Intents>().0);
    for (panel, intent) in intents {
        match intent {
            Intent::New => {
                let id = world
                    .resource_mut::<HttpRequests>()
                    .0
                    .add(SavedRequest::default());
                select(world, panel, id);
                if let Some(mut view) = world.get_mut::<HttpClientView>(panel) {
                    view.focus_url = true;
                }
            }
            Intent::Duplicate(id) => {
                if let Some(copy) = world.resource_mut::<HttpRequests>().0.duplicate(id) {
                    select(world, panel, copy);
                }
            }
            Intent::Delete(id) => {
                let next = world.resource_mut::<HttpRequests>().0.remove(id);
                cancel(world, id);
                world.resource_mut::<Exchanges>().0.remove(&id);
                let mut panels = world.query::<&mut HttpClientPanel>();
                for mut shown in panels.iter_mut(world) {
                    if shown.selected == Some(id) {
                        shown.selected = next;
                    }
                }
            }
            Intent::Send(id) => start(world, panel, id),
            Intent::Cancel(id) => cancel(world, id),
        }
    }
}

fn select(world: &mut World, panel: Entity, id: u64) {
    if let Some(mut shown) = world.get_mut::<HttpClientPanel>(panel) {
        shown.selected = Some(id);
        shown.variables = false;
    }
}

/// Send request `id` (stopping one already on its way), on a task owned by
/// `panel`.
fn start(world: &mut World, panel: Entity, id: u64) {
    cancel(world, id);
    let exchange = match world.resource::<HttpRequests>().0.resolve(id) {
        Err(why) => Exchange::Done(Err(Error::Invalid(why))),
        Ok(request) => {
            let label = format!("{} {}", request.method, request.url);
            let spawned =
                world
                    .resource_mut::<ManagedWork>()
                    .spawn_task(Some(panel), label, move |cancel| {
                        send(&request, &Options::default(), || cancel.is_cancelled())
                            .map(Received::new)
                    });
            match spawned {
                Ok(task) => Exchange::Sending {
                    task,
                    started: Instant::now(),
                },
                Err(why) => Exchange::Done(Err(Error::Failed(why))),
            }
        }
    };
    world.resource_mut::<Exchanges>().0.insert(id, exchange);
}

/// Stop request `id` if it's on its way.
fn cancel(world: &mut World, id: u64) {
    let mut exchanges = world.resource_mut::<Exchanges>();
    if let Some(Exchange::Sending { task, .. }) = exchanges.0.get(&id) {
        task.cancel();
        exchanges
            .0
            .insert(id, Exchange::Done(Err(Error::Cancelled)));
    }
}

fn collect_responses(mut exchanges: ResMut<Exchanges>) {
    if !exchanges.sending() {
        return;
    }
    for exchange in exchanges.0.values_mut() {
        let Exchange::Sending { task, .. } = exchange else {
            continue;
        };
        let result = match task.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => continue,
            // Its panel closed, or the app is closing.
            Err(mpsc::TryRecvError::Disconnected) => match task.status() {
                WorkStatus::Cancelled | WorkStatus::Cancelling => Err(Error::Cancelled),
                WorkStatus::Failed(why) => Err(Error::Failed(why)),
                _ => Err(Error::Failed("The request stopped unexpectedly".into())),
            },
        };
        *exchange = Exchange::Done(result.map(Box::new));
    }
}

impl Panel for HttpClientPanel {
    const LABEL: &'static str = "HTTP Client";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw(ui, world, context, panel);
    }
}

/// What drawing works with.
struct Draw<'a> {
    context: &'a UiContext<'a>,
    panel: Entity,
    collection: &'a mut Collection,
    exchanges: &'a Exchanges,
    shown: &'a mut HttpClientPanel,
    view: &'a mut HttpClientView,
    intents: &'a mut Vec<Intent>,
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let Some(mut shown) = world.entity_mut(panel).take::<HttpClientPanel>() else {
        return;
    };
    let Some(mut view) = world.entity_mut(panel).take::<HttpClientView>() else {
        world.entity_mut(panel).insert(shown);
        return;
    };
    let mut collection = world.resource::<HttpRequests>().0.clone();
    let exchanges = world.resource::<Exchanges>();
    let mut intents = Vec::new();

    // A request deleted elsewhere (or none chosen yet): show the first.
    if !shown.variables && shown.selected.is_none_or(|id| collection.get(id).is_none()) {
        shown.selected = collection.requests.first().map(|r| r.id);
    }
    if exchanges.sending() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    if focused
        && !shown.variables
        && let Some(id) = shown.selected
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
    {
        intents.push(Intent::Send(id));
    }

    let mut d = Draw {
        context,
        panel,
        collection: &mut collection,
        exchanges,
        shown: &mut shown,
        view: &mut view,
        intents: &mut intents,
    };
    let margin = egui::Margin::symmetric(8, 6);
    egui::Panel::left(egui::Id::new((panel, "http-requests")))
        .resizable(true)
        .default_size(190.0)
        .size_range(120.0..=420.0)
        .frame(egui::Frame::NONE.inner_margin(margin))
        .show(ui, |ui| sidebar(ui, &mut d));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| match (d.shown.variables, d.shown.selected) {
            (true, _) => {
                egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
                    variables(ui, &mut d);
                });
            }
            (false, Some(id)) => {
                egui::Panel::top(egui::Id::new((panel, "http-request")))
                    .resizable(true)
                    .default_size(240.0)
                    .min_size(110.0)
                    .frame(egui::Frame::NONE.inner_margin(margin))
                    .show(ui, |ui| request(ui, &mut d, id));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(margin))
                    .show(ui, |ui| exchange(ui, &mut d, id));
            }
            (false, None) => {
                egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
                    ui.weak("No requests yet.");
                    if ui.button("New Request").clicked() {
                        d.intents.push(Intent::New);
                    }
                });
            }
        });

    if collection != world.resource::<HttpRequests>().0 {
        world.resource_mut::<HttpRequests>().0 = collection;
    }
    world.entity_mut(panel).insert((shown, view));
    world
        .resource_mut::<Intents>()
        .0
        .extend(intents.into_iter().map(|intent| (panel, intent)));
}

fn sidebar(ui: &mut egui::Ui, d: &mut Draw) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Requests").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if d.context
                .frame(ui)
                .icon_button(BuiltinIcon::Plus)
                .on_hover_text("New Request")
                .clicked()
            {
                d.intents.push(Intent::New);
            }
        });
    });
    ui.add_space(4.0);
    // The variables, kept at the bottom.
    egui::Panel::bottom(egui::Id::new((d.panel, "http-variables-row")))
        .resizable(false)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            top: 4,
            ..Default::default()
        }))
        .show(ui, |ui| {
            let count = d.collection.variables.iter().filter(|v| v.enabled).count();
            let text = match count {
                0 => "Variables".to_owned(),
                n => format!("Variables ({n})"),
            };
            if row(ui, d.context, d.shown.variables, None, &text, false).clicked() {
                d.shown.variables = true;
            }
        });
    egui::ScrollArea::vertical()
        .id_salt("requests")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for index in 0..d.collection.requests.len() {
                let request = &d.collection.requests[index];
                let id = request.id;
                let selected = !d.shown.variables && d.shown.selected == Some(id);
                let busy = matches!(d.exchanges.get(id), Some(Exchange::Sending { .. }));
                let tag = Some((
                    short(request.method),
                    method_color(d.context, request.method),
                ));
                let response = row(ui, d.context, selected, tag, request.label(), busy);
                if response.clicked() {
                    d.shown.selected = Some(id);
                    d.shown.variables = false;
                }
                response.context_menu(|ui| {
                    if ui.button("Duplicate").clicked() {
                        d.intents.push(Intent::Duplicate(id));
                        ui.close();
                    }
                    if ui.button("Delete…").clicked() {
                        d.view.confirm_delete = Some(id);
                        ui.close();
                    }
                });
                if d.view.confirm_delete == Some(id) {
                    ui.horizontal(|ui| {
                        ui.weak("Delete?");
                        if ui.small_button("Delete").clicked() {
                            d.intents.push(Intent::Delete(id));
                            d.view.confirm_delete = None;
                        }
                        if ui.small_button("Cancel").clicked() {
                            d.view.confirm_delete = None;
                        }
                    });
                }
            }
        });
}

/// A selectable row of the sidebar: an optional method tag, the text cut to
/// fit, and a spinner while `busy`.
fn row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    selected: bool,
    tag: Option<(&str, egui::Color32)>,
    text: &str,
    busy: bool,
) -> egui::Response {
    let p = &context.theme.palette;
    let height = context.metrics.font_size + 10.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, text)
    });
    let painter = ui.painter_at(rect);
    let fill = if selected {
        Some(p.selection)
    } else if response.hovered() {
        Some(theme::mix(p.panel_background, p.text, 0.08))
    } else {
        None
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, context.metrics.rounding, theme::color(fill));
    }
    let ink = theme::color(p.text);
    let mut x = rect.left() + 6.0;
    if let Some((tag, color)) = tag {
        let font = egui::FontId::monospace(context.metrics.font_size * 0.72);
        let width = painter
            .layout_no_wrap("PATCH".into(), font.clone(), color)
            .size()
            .x;
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            tag,
            font,
            color,
        );
        x += width + 6.0;
    }
    let spinner = context.metrics.font_size * 0.8;
    let right = rect.right() - 6.0 - if busy { spinner + 4.0 } else { 0.0 };
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(context.metrics.font_size), ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (right - x).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    if busy {
        let at = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 6.0 - spinner / 2.0, rect.center().y),
            egui::vec2(spinner, spinner),
        );
        egui::Spinner::new().size(spinner).paint_at(ui, at);
    }
    response
}

/// The selected request's editor.
fn request(ui: &mut egui::Ui, d: &mut Draw, id: u64) {
    let sending = matches!(d.exchanges.get(id), Some(Exchange::Sending { .. }));
    let context = d.context;
    let Some(request) = d.collection.get_mut(id) else {
        return;
    };
    ui.add(
        egui::TextEdit::singleline(&mut request.name)
            .hint_text("Untitled")
            .font(egui::FontId::proportional(context.metrics.font_size * 1.1))
            .frame(egui::Frame::NONE)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt((d.panel, "method"))
            .selected_text(
                egui::RichText::new(request.method.as_str())
                    .monospace()
                    .color(method_color(context, request.method)),
            )
            .width(84.0)
            .show_ui(ui, |ui| {
                for method in Method::ALL {
                    ui.selectable_value(&mut request.method, method, method.as_str());
                }
            });
        let button = 72.0;
        let url = ui.add(
            egui::TextEdit::singleline(&mut request.url)
                .hint_text("https://example.com/path  or  {{base}}/path")
                .font(egui::TextStyle::Monospace)
                .desired_width(ui.available_width() - button - ui.spacing().item_spacing.x),
        );
        if d.view.focus_url {
            url.request_focus();
            d.view.focus_url = false;
        }
        let entered = url.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if sending {
            if ui
                .add_sized([button, url.rect.height()], egui::Button::new("Cancel"))
                .clicked()
            {
                d.intents.push(Intent::Cancel(id));
            }
        } else if context.frame(ui).primary_button("Send").clicked() || entered {
            d.intents.push(Intent::Send(id));
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let params = counted("Params", &request.params);
        let headers = counted("Headers", &request.headers);
        let body = if request.body.is_empty() {
            "Body".to_owned()
        } else {
            "Body •".to_owned()
        };
        ui.selectable_value(&mut d.view.request_tab, RequestTab::Params, params);
        ui.selectable_value(&mut d.view.request_tab, RequestTab::Headers, headers);
        ui.selectable_value(&mut d.view.request_tab, RequestTab::Body, body);
    });
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .id_salt("request")
        .auto_shrink([false, false])
        .show(ui, |ui| match d.view.request_tab {
            RequestTab::Params => fields(
                ui,
                context,
                &mut request.params,
                ["Name", "Value"],
                "Add Parameter",
            ),
            RequestTab::Headers => fields(
                ui,
                context,
                &mut request.headers,
                ["Header", "Value"],
                "Add Header",
            ),
            RequestTab::Body => {
                ui.add(
                    egui::TextEdit::multiline(&mut request.body)
                        .code_editor()
                        .hint_text("Request body. Set its Content-Type in Headers.")
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
            }
        });
}

/// "Name (n)", counting the fields that take part.
fn counted(name: &str, fields: &[Field]) -> String {
    match fields
        .iter()
        .filter(|f| f.enabled && !f.name.is_empty())
        .count()
    {
        0 => name.to_owned(),
        n => format!("{name} ({n})"),
    }
}

/// Editable name/value rows, each with a switch and a remove button.
fn fields(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    fields: &mut Vec<Field>,
    hints: [&str; 2],
    add: &str,
) {
    let mut remove = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut field.enabled, "")
                    .on_hover_text("Include it");
                let spacing = ui.spacing().item_spacing.x;
                let remove_width = context.metrics.font_size + ui.spacing().button_padding.x * 2.0;
                let room = (ui.available_width() - remove_width - spacing * 2.0).max(80.0);
                let ink = if field.enabled {
                    None
                } else {
                    Some(theme::color(context.theme.palette.muted))
                };
                let mut name = egui::TextEdit::singleline(&mut field.name)
                    .hint_text(hints[0])
                    .font(egui::TextStyle::Monospace)
                    .desired_width(room * 0.4);
                let mut value = egui::TextEdit::singleline(&mut field.value)
                    .hint_text(hints[1])
                    .font(egui::TextStyle::Monospace)
                    .desired_width(room * 0.6);
                if let Some(ink) = ink {
                    name = name.text_color(ink);
                    value = value.text_color(ink);
                }
                ui.add(name);
                ui.add(value);
                if context
                    .frame(ui)
                    .icon_button(BuiltinIcon::Close)
                    .on_hover_text("Remove")
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        fields.remove(index);
    }
    if ui.button(add).clicked() {
        fields.push(Field::default());
    }
}

fn variables(ui: &mut egui::Ui, d: &mut Draw) {
    ui.label(egui::RichText::new("Variables").strong());
    ui.weak(
        "Write {{name}} in a request's URL, params, headers or body to use a variable's value.",
    );
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .id_salt("variables")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            fields(
                ui,
                d.context,
                &mut d.collection.variables,
                ["Name", "Value"],
                "Add Variable",
            );
        });
}

/// Request `id`'s last exchange: sending, failed, or its response.
fn exchange(ui: &mut egui::Ui, d: &mut Draw, id: u64) {
    let p = &d.context.theme.palette;
    match d.exchanges.get(id) {
        None => {
            let keys = if cfg!(target_os = "macos") {
                "⌘↵"
            } else {
                "Ctrl+Enter"
            };
            ui.weak(format!("Send the request ({keys}) to see its response."));
        }
        Some(Exchange::Sending { started, .. }) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Sending… {:.1} s", started.elapsed().as_secs_f32()));
            });
        }
        Some(Exchange::Done(Err(error))) => {
            let (title, color) = match error {
                Error::Cancelled => ("Cancelled", p.muted),
                Error::TimedOut => ("Timed out", p.error),
                Error::Invalid(_) => ("Couldn't send the request", p.error),
                Error::Failed(_) => ("The request failed", p.error),
            };
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .color(theme::color(color)),
            );
            if let Error::Invalid(why) | Error::Failed(why) = error {
                ui.label(why);
            }
        }
        Some(Exchange::Done(Ok(received))) => response(ui, d, received),
    }
}

fn response(ui: &mut egui::Ui, d: &mut Draw, received: &Received) {
    let p = &d.context.theme.palette;
    let response = &received.response;
    let status = match response.status {
        200..=299 => p.success,
        300..=399 => p.accent,
        400..=499 => p.warning,
        _ => p.error,
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} {}",
                response.status,
                response.reason.unwrap_or("")
            ))
            .strong()
            .color(theme::color(status)),
        );
        ui.weak(format!(
            "{} · {}",
            duration(response.elapsed),
            size(response.body.len())
        ));
        if response.truncated {
            ui.label(egui::RichText::new("Body cut short").color(theme::color(p.warning)))
                .on_hover_text("The body was longer than the client reads (16 MB).");
        }
    });
    ui.add(egui::Label::new(egui::RichText::new(&response.url).small().weak()).truncate());
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut d.view.response_tab, ResponseTab::Body, "Body");
        ui.selectable_value(
            &mut d.view.response_tab,
            ResponseTab::Headers,
            format!("Headers ({})", response.headers.len()),
        );
        if d.view.response_tab == ResponseTab::Body && received.text.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Copy")
                    .on_hover_text("Copy the body shown")
                    .clicked()
                {
                    let indented = d.view.json != JsonMode::Raw;
                    let shown = match (&received.pretty, &received.text) {
                        (Some(pretty), _) if indented => Some(pretty),
                        (_, text) => text.as_ref(),
                    };
                    if let Some(shown) = shown {
                        ui.ctx().copy_text(shown.text.clone());
                    }
                }
                let tree = d.view.json == JsonMode::Tree && received.tree.is_some();
                if !tree {
                    let mut wrap = !d.view.no_wrap;
                    ui.toggle_value(&mut wrap, "Wrap")
                        .on_hover_text("Wrap long lines");
                    d.view.no_wrap = !wrap;
                }
                if received.pretty.is_some() {
                    ui.separator();
                    ui.selectable_value(&mut d.view.json, JsonMode::Raw, "Raw");
                    ui.selectable_value(&mut d.view.json, JsonMode::Indented, "JSON");
                    if received.tree.is_some() {
                        ui.selectable_value(&mut d.view.json, JsonMode::Tree, "Tree");
                    }
                }
            });
        }
    });
    ui.add_space(2.0);
    match d.view.response_tab {
        ResponseTab::Body => body(ui, d, received),
        ResponseTab::Headers => {
            egui::ScrollArea::vertical()
                .id_salt("response-headers")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("response-headers")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            for (name, value) in &response.headers {
                                ui.label(egui::RichText::new(name).monospace().strong());
                                ui.add(
                                    egui::Label::new(egui::RichText::new(value).monospace()).wrap(),
                                );
                                ui.end_row();
                            }
                        });
                });
        }
    }
}

fn body(ui: &mut egui::Ui, d: &mut Draw, received: &Received) {
    let response = &received.response;
    let p = &d.context.theme.palette;
    if response.body.is_empty() {
        ui.weak("No body");
        return;
    }
    if d.view.json == JsonMode::Tree
        && let Some(value) = &received.tree
    {
        let current = d
            .view
            .tree
            .as_ref()
            .is_some_and(|(serial, _)| *serial == received.serial);
        if !current {
            d.view.tree = Some((received.serial, DataTree::new(value.clone())));
        }
        if let Some((_, tree)) = &mut d.view.tree {
            ui.push_id(received.serial, |ui| tree.show(ui, d.context));
        }
        return;
    }
    let pretty = d.view.json != JsonMode::Raw && received.pretty.is_some();
    let shown = match (&received.pretty, &received.text) {
        (Some(indented), _) if pretty => indented,
        (_, Some(text)) => text,
        _ if matches!(received.image, Some(Ok(_))) => {
            let Some(Ok(picture)) = &received.image else {
                return;
            };
            let current = d
                .view
                .image
                .as_ref()
                .is_some_and(|(serial, _)| *serial == received.serial);
            if !current {
                d.view.image = Some((received.serial, ImageView::default()));
            }
            if let Some((_, view)) = &mut d.view.image {
                ui.push_id(received.serial, |ui| {
                    view.show(ui, d.context, picture, false)
                });
            }
            return;
        }
        _ => {
            let kind = match &received.image {
                Some(Err(why)) => format!("Image that couldn't be read ({why})"),
                _ => "Binary body".to_owned(),
            };
            let media = received
                .content
                .media_type
                .as_deref()
                .unwrap_or("unknown type");
            ui.weak(format!("{kind}: {} ({media})", size(response.body.len())));
            let current = d
                .view
                .hex
                .as_ref()
                .is_some_and(|(serial, _)| *serial == received.serial);
            if !current {
                d.view.hex = Some((received.serial, HexViewer::default()));
            }
            if let Some((_, hex)) = &mut d.view.hex {
                ui.push_id(received.serial, |ui| {
                    hex.show(ui, d.context, &response.body, false)
                });
            }
            return;
        }
    };
    let note = |ui: &mut egui::Ui, text: String, color| {
        ui.label(egui::RichText::new(text).small().color(theme::color(color)));
    };
    if shown.clipped {
        note(ui, format!("Showing the first {}", size(SHOWN)), p.warning);
    }
    if let Some(charset) = received.content.charset {
        note(ui, format!("Decoded from {charset}"), p.muted);
    }
    if received.content.replaced {
        note(
            ui,
            "Some bytes weren't valid in its charset and show as \u{fffd}".into(),
            p.warning,
        );
    }
    let language = if pretty {
        Some("json")
    } else {
        match received.content.kind {
            Kind::Text { language } => language,
            _ => None,
        }
    };
    let current = d
        .view
        .body
        .as_ref()
        .is_some_and(|body| body.serial == received.serial && body.pretty == pretty);
    if !current {
        let view = CodeView::new(shown.text.clone());
        d.view.body = Some(BodyView {
            serial: received.serial,
            pretty,
            view: match language {
                Some(language) => view.language(language),
                None => view,
            },
        });
    }
    if let Some(body) = &mut d.view.body {
        body.view.wrap = !d.view.no_wrap;
        ui.push_id(received.serial, |ui| body.view.show(ui, d.context));
    }
}

/// A method's tag in the sidebar: five characters at most.
fn short(method: Method) -> &'static str {
    match method {
        Method::Delete => "DEL",
        Method::Options => "OPT",
        other => other.as_str(),
    }
}

fn method_color(context: &UiContext<'_>, method: Method) -> egui::Color32 {
    let p = &context.theme.palette;
    theme::color(match method {
        Method::Get => p.success,
        Method::Post => p.warning,
        Method::Put | Method::Patch => p.accent,
        Method::Delete => p.error,
        Method::Head | Method::Options => p.muted,
    })
}

pub(crate) fn duration(elapsed: Duration) -> String {
    match elapsed.as_millis() {
        ms @ 0..1000 => format!("{ms} ms"),
        _ => format!("{:.2} s", elapsed.as_secs_f64()),
    }
}

pub(crate) fn size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Run `app` until no request is on its way (or give up).
    pub(crate) fn settle(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if !app.world().resource::<Exchanges>().sending() || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn intend(app: &mut App, panel: Entity, intent: Intent) {
        app.world_mut()
            .resource_mut::<Intents>()
            .0
            .push((panel, intent));
    }

    pub(crate) use super::Intent as TestIntent;
}
