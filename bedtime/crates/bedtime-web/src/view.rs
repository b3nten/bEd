//! The web panel on macOS: a WKWebView (through wry) as a native view over
//! the page's rectangle, moved with it.
//!
//! A native view isn't part of the app's picture: it sits above everything
//! the app draws, as a plain rectangle, and takes its own input. So, with
//! `WebOptions::draw_pages`, it's kept see-through (still taking the mouse
//! and keyboard) and the panel draws snapshots of it. Otherwise it shows
//! itself, and a snapshot stands in for it only while something would be
//! drawn over it: a menu or popup, or areas being rearranged.
use crate::{
    Navigate, ScreenWarp, ShowWeb, WebOptions, WebPanel, fit_inside,
    native::{self, Shot, Shots},
    with_scheme,
};
use bedtime_ui::{BuiltinIcon, UiContext, UiWaker, egui};
use bedtime_workbench::{LayoutsWorldExt, Panel, PanelInstance, PanelPresentation};
use bevy::{
    ecs::system::NonSendMarker,
    platform::collections::HashMap,
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wry::{
    NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

pub fn install(app: &mut App) {
    use bedtime_workbench::{ActionAppExt, Handler, MenusAppExt, StandardMenu};
    app.add_action_handler::<ShowWeb>(Handler::new(bedtime_workbench::open_new::<WebPanel>))
        .add_menu_item::<ShowWeb>(StandardMenu::View, "panels")
        .insert_non_send(Views::default())
        .add_systems(First, follow_panels);
}

/// What a web panel keeps while it's open, besides its web view.
#[derive(Component, Default)]
pub struct WebBar {
    /// The address bar's text, and whether it's being edited.
    address: String,
    editing: bool,
    /// Drawn since `follow_panels` last looked.
    drawn: bool,
    was_focused: bool,
}

/// Each panel's web view (native views stay on the main thread).
#[derive(Default)]
struct Views(HashMap<Entity, View>);

struct View {
    webview: WebView,
    /// Wakes the app when the page or a snapshot has news: it sleeps
    /// between inputs otherwise, and input to the page isn't the app's.
    waker: UiWaker,
    /// Wakes it at a time, and whether that's been asked since the last
    /// snapshot was.
    alarm: Alarm,
    alarmed: bool,
    page: Arc<Mutex<Page>>,
    /// Where the web view is, in the window's logical pixels: x, y, width,
    /// height.
    bounds: [i32; 4],
    visible: bool,
    see_through: bool,
    /// It had the keyboard when last drawn.
    had_keyboard: bool,
    /// Where a snapshot arrives, and when the last one was asked for (and
    /// whether it's still to come).
    shots: Shots,
    asked: Instant,
    waiting: bool,
    /// The snapshot drawn for the page: its image, egui's name for it, and
    /// its size in pixels.
    picture: Option<(Handle<Image>, egui::TextureId, [usize; 2])>,
    /// The picture's pixels, to tell when the page hasn't changed, and how
    /// many snapshots in a row haven't.
    pixels: Vec<u8>,
    unchanged: u32,
}

/// Wakes the app at the times it's given: a thread that sleeps until the
/// earliest. (A delayed repaint request would keep the app drawing until
/// then instead.)
struct Alarm(std::sync::mpsc::Sender<Instant>);

impl Alarm {
    fn new(waker: UiWaker) -> Self {
        use std::sync::mpsc::RecvTimeoutError;
        let (times, asked) = std::sync::mpsc::channel::<Instant>();
        std::thread::spawn(move || {
            while let Ok(mut at) = asked.recv() {
                loop {
                    let now = Instant::now();
                    if at <= now {
                        break;
                    }
                    match asked.recv_timeout(at - now) {
                        Ok(other) => at = at.min(other),
                        Err(RecvTimeoutError::Timeout) => break,
                        // The view is gone.
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                waker.wake();
            }
        });
        Self(times)
    }

    fn at(&self, at: Instant) {
        let _ = self.0.send(at);
    }
}

/// What the web view's callbacks report.
#[derive(Default)]
struct Page {
    url: String,
    title: String,
    loading: bool,
    /// A link asked for a new window: it opens here instead.
    open: Option<String>,
    /// The link under the pointer.
    hover: String,
}

/// What a new panel shows.
const BLANK: &str = "about:blank";
/// A page that hasn't changed for this many snapshots is looked at less
/// often, until it changes again.
const SETTLED_AFTER: u32 = 20;
const SETTLED_INTERVAL: Duration = Duration::from_millis(50);
/// A stand-in snapshot that hasn't come by now isn't waited for: the web
/// view is hidden without one.
const STAND_IN_PATIENCE: Duration = Duration::from_millis(150);

/// Tells the panel which link the pointer is over, and of presses (the app
/// is woken: the page may have taken the keyboard).
const PAGE_SCRIPT: &str = r#"(() => {
  let last = '';
  const send = href => {
    if (href !== last) { last = href; window.ipc.postMessage('hover:' + href); }
  };
  document.addEventListener('mouseover', event => {
    const link = event.target.closest && event.target.closest('a[href]');
    send(link ? link.href : '');
  }, true);
  document.addEventListener('mouseleave', () => send(''), true);
  document.addEventListener('mousedown', () => window.ipc.postMessage('press'), true);
})();"#;

/// Closed panels drop their web views, and ones that weren't drawn last
/// frame (a hidden tab) hide theirs.
fn follow_panels(
    mut views: NonSendMut<Views>,
    mut panels: Query<&mut WebBar, With<WebPanel>>,
    _: NonSendMarker,
) {
    views.0.retain(|panel, _| panels.contains(*panel));
    for (panel, view) in &mut views.0 {
        if let Ok(mut bar) = panels.get_mut(*panel) {
            if !bar.drawn && view.visible {
                view.visible = false;
                let _ = view.webview.set_visible(false);
            }
            bar.drawn = false;
        }
    }
}

pub fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let panel_focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let Some(saved_url) = world.get::<WebPanel>(panel).map(|web| web.url.clone()) else {
        return;
    };
    let Some(mut bar) = world
        .get_mut::<WebBar>(panel)
        .map(|mut bar| std::mem::take(&mut *bar))
    else {
        return;
    };
    // Out of the map while it's worked on: the world is needed too.
    let mut view = world.non_send_mut::<Views>().0.remove(&panel);
    let (url, title, loading, open, hover) = view.as_ref().map_or_else(default, |view| {
        let mut page = view.page.lock().unwrap();
        (
            page.url.clone(),
            page.title.clone(),
            page.loading,
            page.open.take(),
            page.hover.clone(),
        )
    });
    let url = if url == BLANK { String::new() } else { url };

    // The page, with the address bar under it: that keeps the web view's
    // rectangle clear of the panel's rounded bottom corners (the tab bar
    // squares off the top).
    let all = ui.available_rect_before_wrap();
    ui.allocate_rect(all, egui::Sense::hover());
    let bar_height = ui.spacing().interact_size.y + 8.0;
    let (rect, below) = all.split_top_bottom_at_y((all.max.y - bar_height).max(all.min.y));

    let was_editing = bar.editing;
    if !bar.editing {
        // The link under the pointer shows in place of the address.
        bar.address
            .clone_from(if hover.is_empty() { &url } else { &hover });
    }
    let go = ui
        .scope_builder(egui::UiBuilder::new().max_rect(below), |ui| {
            address_bar(ui, context, &mut bar, loading)
        })
        .inner;
    // An address it was sent to (`open_url`), once its web view exists.
    let sent = view
        .is_some()
        .then(|| world.entity_mut(panel).take::<Navigate>())
        .flatten();
    let go = sent.map(|Navigate(url)| Go::To(url)).or(go);

    // Where the web view goes: what's seen of the page's rectangle (the
    // panel's frame clips its edges), on whole window pixels inside it.
    // Showing itself, it also keeps to where the screen shows the panel
    // (effects may bend the picture). Drawn by the panel, the picture fills
    // the rectangle and is bent with everything else.
    let draw_pages = world.resource::<WebOptions>().draw_pages;
    let seen = rect.intersect(ui.clip_rect());
    let on_screen = match world.resource::<ScreenWarp>().0.clone() {
        Some(warp) if !draw_pages => fit_inside(seen, ui.ctx().content_rect(), &*warp),
        _ => seen,
    };
    let pixels_per_point = ui.ctx().pixels_per_point();
    let window_scale = world
        .get::<Window>(context.surface)
        .map_or(pixels_per_point, Window::scale_factor);
    let to_window = pixels_per_point / window_scale;
    let (left, top) = (
        (on_screen.min.x * to_window).ceil() as i32,
        (on_screen.min.y * to_window).ceil() as i32,
    );
    let bounds = [
        left,
        top,
        ((on_screen.max.x * to_window).floor() as i32 - left).max(1),
        ((on_screen.max.y * to_window).floor() as i32 - top).max(1),
    ];
    // Menus, popups, dragged tabs and moving dividers would be drawn under
    // the web view (or, see-through, have their clicks taken by it).
    let covered = egui::Popup::is_any_open(ui.ctx())
        || ui.ctx().memory(|memory| memory.top_modal_layer().is_some())
        || egui::DragAndDrop::has_any_payload(ui.ctx())
        || world.layouts().rearranging();
    let showable = ui.is_rect_visible(rect);

    if view.is_none() {
        let first_url = if saved_url.is_empty() {
            BLANK
        } else {
            &saved_url
        };
        match create(world, context.surface, bounds, first_url) {
            Ok(created) => view = Some(created),
            Err(error) => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("The web view couldn't be made: {error}"),
                    egui::FontId::proportional(14.0),
                    bedtime_ui::theme::color(context.theme.palette.muted),
                );
            }
        }
    }

    if let Some(view) = &mut view {
        if view.bounds != bounds {
            view.bounds = bounds;
            let _ = view.webview.set_bounds(wry_bounds(bounds));
        }
        if view.see_through != draw_pages {
            view.see_through = draw_pages;
            native::set_transparent(&view.webview, draw_pages);
        }

        // A snapshot that arrived becomes the picture, if it differs.
        let arrived = view.shots.lock().unwrap().take();
        if let Some(shot) = arrived {
            view.waiting = false;
            match shot {
                Some(shot)
                    if view.pixels != shot.pixels
                        || view
                            .picture
                            .as_ref()
                            .is_none_or(|picture| picture.2 != shot.size) =>
                {
                    view.unchanged = 0;
                    view.pixels.clone_from(&shot.pixels);
                    show_shot(world, &mut view.picture, shot);
                }
                // The same picture, or none could be taken: no hurry for
                // the next.
                _ => view.unchanged += 1,
            }
        }

        let visible = if draw_pages {
            // Snapshots one after another (each wakes the app as it
            // arrives), less often once the page has settled. The last one
            // stays while the web view is hidden.
            let visible = showable && !covered;
            if visible && !view.waiting {
                let wait = if view.unchanged < SETTLED_AFTER {
                    Duration::ZERO
                } else {
                    SETTLED_INTERVAL.saturating_sub(view.asked.elapsed())
                };
                if wait.is_zero() {
                    ask(view);
                } else if !view.alarmed {
                    view.alarmed = true;
                    view.alarm.at(Instant::now() + wait);
                }
            }
            visible
        } else if covered {
            // Ask for a stand-in, and keep the web view up until it comes
            // (or takes too long).
            if view.picture.is_none() && !view.waiting {
                ask(view);
            }
            let patient =
                view.picture.is_none() && view.waiting && view.asked.elapsed() < STAND_IN_PATIENCE;
            if patient && !view.alarmed {
                view.alarmed = true;
                view.alarm.at(view.asked + STAND_IN_PATIENCE);
            }
            showable && patient
        } else {
            view.picture = None;
            view.waiting = false;
            showable
        };
        if view.visible != visible {
            view.visible = visible;
            let _ = view.webview.set_visible(visible);
        }
        if (draw_pages || !visible)
            && showable
            && let Some((_, texture, size)) = &view.picture
        {
            // At its own size: it doesn't stretch as the panel does.
            ui.painter().with_clip_rect(seen).image(
                *texture,
                egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(size[0] as f32, size[1] as f32) / pixels_per_point,
                ),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // The web view keeps its own history.
        match go {
            Some(Go::Back) => drop(view.webview.evaluate_script("history.back()")),
            Some(Go::Forward) => drop(view.webview.evaluate_script("history.forward()")),
            Some(Go::Reload) => drop(view.webview.reload()),
            Some(Go::To(url)) => {
                if let Err(error) = view.webview.load_url(&url) {
                    warn!("{url} couldn't be loaded: {error}");
                }
            }
            None => {}
        }
        if let Some(url) = open {
            let _ = view.webview.load_url(&url);
        }
        // The web view keeps the keyboard until it's given back: when
        // another panel takes focus, or the address bar does.
        if (bar.was_focused && !panel_focused) || (bar.editing && !was_editing) {
            let _ = view.webview.focus_parent();
        }
        // A click on the page gives the web view the keyboard without the
        // app hearing of it: the panel takes focus then, as if clicked.
        let has_keyboard = native::has_keyboard(&view.webview);
        if has_keyboard && !view.had_keyboard && !panel_focused {
            // After drawing: the layout isn't changed while it's drawn.
            world.commands().queue(move |world: &mut World| {
                let _ = world.layouts_mut().focus_panel(panel);
            });
        }
        view.had_keyboard = has_keyboard;
    }
    if let Some(view) = view {
        world.non_send_mut::<Views>().0.insert(panel, view);
    }
    bar.was_focused = panel_focused;
    bar.drawn = true;

    // The tab shows the page's title, and the panel remembers its address.
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
        let title = if title.is_empty() {
            WebPanel::LABEL
        } else {
            &title
        };
        if instance.title != title {
            instance.title = title.to_string();
        }
    }
    if !url.is_empty()
        && url != saved_url
        && let Some(mut web) = world.get_mut::<WebPanel>(panel)
    {
        web.url = url;
    }
    if let Some(mut slot) = world.get_mut::<WebBar>(panel) {
        *slot = bar;
    }
}

fn ask(view: &mut View) {
    view.asked = Instant::now();
    view.waiting = true;
    view.alarmed = false;
    let waker = view.waker.clone();
    native::take_snapshot(&view.webview, view.shots.clone(), move || waker.wake());
}

/// What the address bar asked for.
enum Go {
    Back,
    Forward,
    Reload,
    To(String),
}

/// Back, forward, reload and the address.
fn address_bar(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    bar: &mut WebBar,
    loading: bool,
) -> Option<Go> {
    let mut go = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                {
                    let mut frame = context.frame(ui);
                    for (icon, name, action) in [
                        (BuiltinIcon::ChevronLeft, "Back", Go::Back),
                        (BuiltinIcon::ChevronRight, "Forward", Go::Forward),
                        (BuiltinIcon::RefreshCw, "Reload", Go::Reload),
                    ] {
                        if frame.icon_button(icon).on_hover_text(name).clicked() {
                            go = Some(action);
                        }
                    }
                }
                if loading {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut bar.address)
                        .hint_text("Address")
                        .desired_width(f32::INFINITY),
                );
                if edit.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && !bar.address.trim().is_empty()
                {
                    go = Some(Go::To(with_scheme(&bar.address)));
                }
                bar.editing = edit.has_focus();
            });
        });
    go
}

/// Put a snapshot's pixels in the picture: written over the image already
/// there (the same texture, so nothing flickers), or a new one when the size
/// changes.
fn show_shot(
    world: &mut World,
    picture: &mut Option<(Handle<Image>, egui::TextureId, [usize; 2])>,
    shot: Shot,
) {
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };
    if let Some((handle, _, size)) = picture
        && *size == shot.size
        && let Some(mut image) = world.resource_mut::<Assets<Image>>().get_mut(&*handle)
    {
        image.data = Some(shot.pixels);
        return;
    }
    let image = Image::new(
        Extent3d {
            width: shot.size[0] as u32,
            height: shot.size[1] as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        shot.pixels,
        TextureFormat::Rgba8UnormSrgb,
        // Kept in the main world too, to write the next snapshot over.
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = world.resource_mut::<Assets<Image>>().add(image);
    let texture = world
        .resource_mut::<bevy_egui::EguiUserTextures>()
        .add_image(bevy_egui::EguiTextureHandle::Weak(handle.id()));
    *picture = Some((handle, texture, shot.size));
}

fn wry_bounds(bounds: [i32; 4]) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(bounds[0], bounds[1]).into(),
        size: LogicalSize::new(bounds[2] as u32, bounds[3] as u32).into(),
    }
}

/// A web view as a child of the window `surface` is drawn on.
fn create(world: &mut World, surface: Entity, bounds: [i32; 4], url: &str) -> Result<View, String> {
    let handle = match world.get::<RawHandleWrapper>(surface) {
        Some(handle) => handle.clone(),
        None => world
            .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
            .iter(world)
            .next()
            .cloned()
            .ok_or("no window")?,
    };
    // SAFETY: panels are drawn on the main thread, where the window lives.
    let window = unsafe { handle.get_handle() };
    let page = Arc::new(Mutex::new(Page {
        url: url.to_string(),
        loading: true,
        ..default()
    }));
    let (titled, loaded, opened, hovered) =
        (page.clone(), page.clone(), page.clone(), page.clone());
    let waker = world.get_resource::<UiWaker>().cloned().unwrap_or_default();
    let (titled_wake, loaded_wake, opened_wake, hovered_wake) =
        (waker.clone(), waker.clone(), waker.clone(), waker.clone());
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script(PAGE_SCRIPT)
        .with_ipc_handler(move |request| {
            if let Some(link) = request.body().strip_prefix("hover:") {
                hovered.lock().unwrap().hover = link.to_string();
            }
            hovered_wake.wake();
        })
        .with_bounds(wry_bounds(bounds))
        .with_focused(false)
        .with_accept_first_mouse(true)
        .with_document_title_changed_handler(move |title| {
            titled.lock().unwrap().title = title;
            titled_wake.wake();
        })
        .with_on_page_load_handler(move |event, url| {
            let mut page = loaded.lock().unwrap();
            page.loading = matches!(event, PageLoadEvent::Started);
            page.url = url;
            // The new page hasn't said what the pointer is over.
            page.hover.clear();
            loaded_wake.wake();
        })
        .with_new_window_req_handler(move |url, _| {
            opened.lock().unwrap().open = Some(url);
            opened_wake.wake();
            NewWindowResponse::Deny
        })
        .build_as_child(&window)
        .map_err(|error| error.to_string())?;
    Ok(View {
        webview,
        alarm: Alarm::new(waker.clone()),
        alarmed: false,
        waker,
        page,
        bounds,
        visible: true,
        see_through: false,
        had_keyboard: false,
        shots: default(),
        asked: Instant::now(),
        waiting: false,
        picture: None,
        pixels: Vec::new(),
        unchanged: 0,
    })
}
