//! A video player for workbench apps, on macOS (AVFoundation plays it,
//! with its sound; anything QuickTime plays, plays). The picture fills its
//! panel; a floating bar under it plays, scrubs, loops and sets the volume,
//! and fades while playing. Click or Space plays and pauses, Left and Right
//! skip five seconds. Files aren't read into memory: the player reads them.
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
#[cfg(target_os = "macos")]
use bedtime_ui::{BuiltinIcon, Floating, UiFrame, theme};
use bedtime_ui::{UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
#[cfg(target_os = "macos")]
use bedtime_workbench::{Fit, ViewersAppExt};
use bedtime_workbench::{Panel, PanelPresentation, ViewerFile, viewer_document};
use bevy::{platform::collections::HashMap, prelude::*};
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod player;
#[cfg(target_os = "macos")]
use player::Player;

/// Elsewhere, nothing plays.
#[cfg(not(target_os = "macos"))]
mod player {
    pub struct Player;
}
#[cfg(not(target_os = "macos"))]
use player::Player;

pub const SUPPORTED_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "qt", "3gp"];

pub struct VideoPlugin;

impl Plugin for VideoPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<VideoPanel>()
            .insert_non_send(Players::default())
            .add_systems(Update, drop_players);
        #[cfg(target_os = "macos")]
        app.register_viewer::<VideoFile, VideoPanel>("Video")
            .files(|path, _| {
                let known = path.extension().is_some_and(|ext| {
                    SUPPORTED_EXTENSIONS
                        .iter()
                        .any(|known| ext.eq_ignore_ascii_case(known))
                });
                if known { Fit::Specific } else { Fit::No }
            });
    }
}

/// A video file: its revision is all that's kept (the player reads it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoFile;

impl DocumentModel for VideoFile {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl FileModel for VideoFile {
    const UNREAD: bool = true;
    fn decode(_: &[u8]) -> Result<Self, String> {
        Ok(Self)
    }
    fn encode(&self, _: &mut dyn std::io::Write) -> std::io::Result<()> {
        Err(std::io::Error::other("Videos aren't saved"))
    }
}

#[bedtime_ui::panel]
#[derive(Reflect)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct VideoPanel {
    pub looping: bool,
    pub muted: bool,
    pub volume: f32,
}

impl Default for VideoPanel {
    fn default() -> Self {
        Self {
            looping: false,
            muted: false,
            volume: 1.0,
        }
    }
}

/// Each panel's player (AVFoundation objects stay on the main thread).
#[derive(Default)]
pub struct Players(HashMap<Entity, Playing>);

struct Playing {
    file: PathBuf,
    revision: u64,
    player: Result<Player, String>,
    /// The picture: one image, written over with each new frame (a new
    /// texture each frame would be missing on the GPU for a frame, and flicker).
    picture: Option<(Handle<Image>, egui::TextureId, [u32; 2])>,
    /// While dragging the scrubber: play again after.
    resume: bool,
}

impl Players {
    /// Whether `panel`'s video is playing, and where (in seconds).
    #[cfg(target_os = "macos")]
    pub fn state(&self, panel: Entity) -> Option<(bool, f64)> {
        let player = self.0.get(&panel)?.player.as_ref().ok()?;
        Some((player.playing(), player.position()))
    }
}

fn drop_players(mut players: NonSendMut<Players>, panels: Query<(), With<VideoPanel>>) {
    players.0.retain(|panel, _| panels.contains(*panel));
}

impl Panel for VideoPanel {
    const LABEL: &'static str = "Video";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<VideoFile>(ui, world, panel) else {
            return;
        };
        let Some(file) = world.get::<ViewerFile>(panel).map(|file| file.0.clone()) else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let settings = world
            .get::<VideoPanel>(panel)
            .map(|video| (video.looping, video.muted, video.volume));
        let Some((looping, muted, volume)) = settings else {
            return;
        };
        let mut players = world.non_send_mut::<Players>();
        // A new file, or it changed on disk: open it again, where it was.
        let stale = players
            .0
            .get(&panel)
            .is_none_or(|playing| playing.file != file || playing.revision != document.revision);
        if stale {
            let at = players
                .0
                .get(&panel)
                .and_then(|playing| playing.player.as_ref().ok().map(position));
            let player = open(&file);
            if let (Ok(player), Some(at)) = (&player, at) {
                seek(player, at);
            }
            players.0.insert(
                panel,
                Playing {
                    file,
                    revision: document.revision,
                    player,
                    picture: None,
                    resume: false,
                },
            );
        }
        #[cfg(target_os = "macos")]
        let frame = players
            .0
            .get(&panel)
            .and_then(|playing| playing.player.as_ref().ok())
            .filter(|player| player.ready() == Ok(true))
            .and_then(|player| player.frame());
        #[cfg(not(target_os = "macos"))]
        let frame: Option<(Vec<u8>, [u32; 2])> = None;
        if let Some((pixels, size)) = frame {
            show_frame(world, panel, pixels, size);
        }
        let mut players = world.non_send_mut::<Players>();
        let playing = players.0.get_mut(&panel).unwrap();
        let mut settings = VideoPanel {
            looping,
            muted,
            volume,
        };
        draw(ui, context, panel, playing, &mut settings, focused);
        if let Some(mut video) = world.get_mut::<VideoPanel>(panel) {
            *video = settings;
        }
    }
}

/// Put a frame's pixels in the panel's picture: written over the image
/// already there (the same texture, so nothing flickers), or a new one when
/// the size changes.
fn show_frame(world: &mut World, panel: Entity, pixels: Vec<u8>, size: [u32; 2]) {
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };
    let current = world
        .non_send::<Players>()
        .0
        .get(&panel)
        .and_then(|playing| playing.picture.clone());
    if let Some((handle, _, at)) = &current
        && *at == size
        && let Some(mut image) = world.resource_mut::<Assets<Image>>().get_mut(handle)
    {
        image.data = Some(pixels);
        return;
    }
    let image = Image::new(
        Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        // Kept in the main world too, to write the next frame over.
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = world.resource_mut::<Assets<Image>>().add(image);
    let id = world
        .resource_mut::<bevy_egui::EguiUserTextures>()
        .add_image(bevy_egui::EguiTextureHandle::Weak(handle.id()));
    if let Some(playing) = world.non_send_mut::<Players>().0.get_mut(&panel) {
        playing.picture = Some((handle, id, size));
    }
}

#[cfg(target_os = "macos")]
fn open(file: &std::path::Path) -> Result<Player, String> {
    Player::open(file)
}
#[cfg(not(target_os = "macos"))]
fn open(_: &std::path::Path) -> Result<Player, String> {
    Err("Video plays on macOS".into())
}

#[cfg(target_os = "macos")]
fn position(player: &Player) -> f64 {
    player.position()
}
#[cfg(not(target_os = "macos"))]
fn position(_: &Player) -> f64 {
    0.0
}

#[cfg(target_os = "macos")]
fn seek(player: &Player, at: f64) {
    player.seek(at);
}
#[cfg(not(target_os = "macos"))]
fn seek(_: &Player, _: f64) {}

fn time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(not(target_os = "macos"))]
fn draw(
    ui: &mut egui::Ui,
    _: &UiContext<'_>,
    _: Entity,
    playing: &mut Playing,
    _: &mut VideoPanel,
    _: bool,
) {
    if let Err(error) = &playing.player {
        ui.centered_and_justified(|ui| ui.label(error));
    }
}

#[cfg(target_os = "macos")]
fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    playing: &mut Playing,
    settings: &mut VideoPanel,
    focused: bool,
) {
    let id = egui::Id::new(("bedtime.video", panel));
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::Vec2::splat(1.0)),
        egui::Sense::click(),
    );
    let player = match &playing.player {
        Ok(player) => player,
        Err(error) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("This video can't be played: {error}"),
                egui::FontId::proportional(14.0),
                theme::color(context.theme.palette.muted),
            );
            return;
        }
    };
    match player.ready() {
        Ok(true) => {}
        Ok(false) => {
            ui.ctx().request_repaint();
            ui.put(
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(24.0)),
                egui::Spinner::new(),
            );
            return;
        }
        Err(error) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("This video can't be played: {error}"),
                egui::FontId::proportional(14.0),
                theme::color(context.theme.palette.muted),
            );
            return;
        }
    }
    player.set_volume(settings.volume, settings.muted);
    let duration = player.duration();
    let now = player.position();
    let is_playing = player.playing();
    // At the end: loop, or stay there.
    if !is_playing && duration > 0.0 && now >= duration - 0.05 && settings.looping {
        player.seek(0.0);
        player.play();
    }
    if is_playing {
        ui.ctx().request_repaint();
    }
    // The picture, fit to the panel.
    let size = player.size();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(8));
    if let Some((_, texture, pixels)) = &playing.picture {
        let natural = if size[0] > 0.0 && size[1] > 0.0 {
            egui::vec2(size[0], size[1])
        } else {
            egui::vec2(pixels[0] as f32, pixels[1] as f32)
        };
        let scale = (rect.width() / natural.x).min(rect.height() / natural.y);
        let picture = egui::Rect::from_center_size(rect.center(), natural * scale);
        painter.image(
            *texture,
            picture,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    let mut toggle = response.clicked();
    let mut seek_to = None;
    if response.clicked() || response.secondary_clicked() {
        response.request_focus();
    }
    if focused && !ui.ctx().egui_wants_keyboard_input() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.input_mut(|i| {
            toggle |= i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                seek_to = Some(now + 5.0);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                seek_to = Some(now - 5.0);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                seek_to = Some(0.0);
            }
        });
    }
    response.context_menu(|ui| {
        if ui
            .button(if is_playing { "Pause" } else { "Play" })
            .clicked()
        {
            toggle = true;
        }
        if ui.button("Play from Start").clicked() {
            seek_to = Some(0.0);
            if !is_playing {
                toggle = true;
            }
        }
        ui.separator();
        ui.checkbox(&mut settings.looping, "Loop");
        ui.checkbox(&mut settings.muted, "Mute");
    });

    // The floating controls: shown while paused, and when pointed at.
    let mut frame = context.frame(ui);
    Floating::new(id.with("transport"), rect, egui::Align2::CENTER_BOTTOM)
        .shown(!is_playing)
        .show(&mut frame, |f| {
            let (icon, tip) = if is_playing {
                (BuiltinIcon::Pause, "Pause (Space)")
            } else {
                (BuiltinIcon::Play, "Play (Space)")
            };
            if f.floating_button(icon, tip).clicked() {
                toggle = true;
            }
            let width = (rect.width() * 0.45).clamp(80.0, 520.0);
            if let Some(scrub) = scrubber(f, width, now, duration) {
                match scrub {
                    Scrub::Start(at) => {
                        playing.resume = is_playing;
                        player.pause();
                        seek_to = Some(at);
                    }
                    Scrub::Move(at) => seek_to = Some(at),
                    Scrub::End(at) => {
                        seek_to = Some(at);
                        if playing.resume {
                            player.play();
                        }
                    }
                }
            }
            f.floating_label(format!("{} / {}", time(now), time(duration)));
            if f.floating_toggle(BuiltinIcon::Repeat, "Loop", settings.looping)
                .clicked()
            {
                settings.looping = !settings.looping;
            }
            let icon = if settings.muted || settings.volume <= 0.0 {
                BuiltinIcon::VolumeOff
            } else {
                BuiltinIcon::Volume
            };
            let volume = f.floating_button(icon, "Volume (click the menu to mute)");
            egui::Popup::menu(&volume)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.add(
                        egui::Slider::new(&mut settings.volume, 0.0..=1.0)
                            .show_value(false)
                            .vertical(),
                    );
                    ui.checkbox(&mut settings.muted, "Mute");
                });
        });
    if size[0] > 0.0 {
        Floating::new(id.with("info"), rect, egui::Align2::LEFT_TOP).show(&mut frame, |f| {
            f.floating_label(format!("{} × {} · {}", size[0], size[1], time(duration)));
        });
    }
    if let Some(at) = seek_to {
        let at = at.clamp(0.0, duration.max(0.0));
        player.seek(at);
    }
    if toggle {
        if is_playing {
            player.pause();
        } else {
            if duration > 0.0 && player.position() >= duration - 0.05 {
                player.seek(0.0);
            }
            player.play();
        }
    }
}

enum Scrub {
    Start(f64),
    Move(f64),
    End(f64),
}

/// A thin timeline: where it is, and dragging (or clicking) moves it.
#[cfg(target_os = "macos")]
fn scrubber(f: &mut UiFrame<'_>, width: f32, now: f64, duration: f64) -> Option<Scrub> {
    let height = f.metrics.font_size + 8.0;
    let (rect, response) =
        f.ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    let p = &f.theme.palette;
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(width - 8.0, 4.0));
    let fraction = if duration > 0.0 {
        (now / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let painter = f.ui.painter();
    painter.rect_filled(
        track,
        2.0,
        theme::color(theme::mix(p.border, p.panel_background, 0.2)),
    );
    let played = egui::Rect::from_min_max(
        track.min,
        egui::pos2(track.left() + track.width() * fraction, track.max.y),
    );
    painter.rect_filled(played, 2.0, theme::color(p.accent));
    let hot = response.hovered() || response.dragged();
    painter.circle_filled(
        egui::pos2(played.right(), track.center().y),
        if hot { 6.0 } else { 4.5 },
        theme::color(p.text),
    );
    let at = |response: &egui::Response| {
        response.interact_pointer_pos().map(|pointer| {
            ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * duration
        })
    };
    if response.drag_started() {
        return at(&response).map(Scrub::Start);
    }
    if response.dragged() {
        return at(&response).map(Scrub::Move);
    }
    if response.drag_stopped() {
        return at(&response).map(Scrub::End);
    }
    if response.clicked() {
        return at(&response).map(Scrub::Move);
    }
    None
}
