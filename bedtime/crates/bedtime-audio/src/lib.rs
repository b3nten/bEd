//! An audio player for workbench apps. The sound fills its panel as a
//! waveform (click or drag to seek), a live spectrum, or a spectrogram. A
//! floating bar plays, loops and sets the volume; another switches views.
//! Space plays and pauses, Left and Right skip five seconds. The output
//! device opens the first time something plays.
use bedtime_ui::{BuiltinIcon, Floating, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Notification, Panel, PanelPresentation, ViewersAppExt, WorkbenchWorldExt, viewer_document,
};
use bevy::prelude::*;
use std::{sync::Arc, time::Duration};

mod sound;
pub use sound::{Analysis, BINS, FLOOR_DB, PEAK_HOP, SUPPORTED_EXTENSIONS, Sound, decoder};

pub struct AudioPlugin;

/// Audio files: the extensions in `SUPPORTED_EXTENSIONS`.
pub const AUDIO: bedtime_workbench::FormatId = bedtime_workbench::FormatId("audio");

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{Format, FormatsAppExt};
        app.register_type::<AudioPanel>()
            .insert_non_send(Speakers::default())
            .register_format(Format::new(AUDIO).extensions(SUPPORTED_EXTENSIONS.iter().copied()))
            .register_viewer::<Sound, AudioPanel>("Audio")
            .formats([AUDIO]);
    }
}

pub fn has_extension(path: &std::path::Path, extensions: &[&str]) -> bool {
    path.extension().is_some_and(|ext| {
        extensions
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// The output device, opened when something first plays.
#[derive(Default)]
pub enum Speakers {
    #[default]
    Closed,
    Open(rodio::OutputStream),
    Unavailable,
}

impl Speakers {
    /// A new sink on the device, opening it if need be.
    pub fn sink(world: &mut World) -> Result<rodio::Sink, String> {
        let mut speakers = world.non_send_mut::<Speakers>();
        if matches!(*speakers, Speakers::Closed) {
            *speakers = match rodio::OutputStreamBuilder::open_default_stream() {
                Ok(mut stream) => {
                    stream.log_on_drop(false);
                    Speakers::Open(stream)
                }
                Err(_) => Speakers::Unavailable,
            };
        }
        match &*speakers {
            Speakers::Open(stream) => Ok(rodio::Sink::connect_new(stream.mixer())),
            _ => Err("There's no audio output to play to".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum AudioView {
    #[default]
    Waveform,
    Spectrum,
    Spectrogram,
}

#[bedtime_ui::panel]
#[derive(Reflect)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct AudioPanel {
    pub view: AudioView,
    pub looping: bool,
    pub volume: f32,
    #[reflect(ignore)]
    player: Option<Player>,
    /// Where it is when not playing.
    #[reflect(ignore)]
    position: Duration,
    #[reflect(ignore)]
    bars: Vec<[f32; 2]>,
    #[reflect(ignore)]
    spectrogram: Option<(usize, egui::TextureHandle)>,
}

impl Default for AudioPanel {
    fn default() -> Self {
        Self {
            view: AudioView::Waveform,
            looping: false,
            volume: 1.0,
            player: None,
            position: Duration::ZERO,
            bars: Vec::new(),
            spectrogram: None,
        }
    }
}

struct Player {
    sink: rodio::Sink,
    sound: Arc<Analysis>,
}

impl AudioPanel {
    pub fn playing(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|player| !player.sink.is_paused() && !player.sink.empty())
    }

    pub fn position(&self) -> Duration {
        match &self.player {
            Some(player) if !player.sink.empty() => player.sink.get_pos(),
            _ => self.position,
        }
    }
}

/// What the user asked for this frame.
#[derive(Default)]
struct Asked {
    toggle: bool,
    seek: Option<Duration>,
}

impl Panel for AudioPanel {
    const LABEL: &'static str = "Audio";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<Sound>(ui, world, panel) else {
            return;
        };
        let Some(sound) = document.model.0 else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut audio) = world.get_mut::<AudioPanel>(panel) else {
            return;
        };
        // The file changed: play the new one from where it was.
        if audio
            .player
            .as_ref()
            .is_some_and(|player| !Arc::ptr_eq(&player.sound, &sound))
        {
            audio.position = audio.position();
            audio.player = None;
        }
        let asked = draw(ui, context, panel, &mut audio, &sound, focused);
        if asked.toggle
            && let Err(error) = toggle(world, panel, &sound) {
                world.workbench_mut().notify(Notification::warning(error));
            }
        if let Some(at) = asked.seek {
            let mut audio = world.get_mut::<AudioPanel>(panel).unwrap();
            let at = at.min(sound.duration);
            audio.position = at;
            if let Some(player) = &audio.player
                && !player.sink.empty()
            {
                let _ = player.sink.try_seek(at);
            }
        }
    }
}

/// Play or pause; at the end, play from the start.
fn toggle(world: &mut World, panel: Entity, sound: &Arc<Analysis>) -> Result<(), String> {
    let audio = world.get::<AudioPanel>(panel).unwrap();
    if let Some(player) = &audio.player
        && !player.sink.empty()
    {
        if player.sink.is_paused() {
            player.sink.play();
        } else {
            player.sink.pause();
        }
        return Ok(());
    }
    let mut from = audio.position;
    if from + Duration::from_millis(50) >= sound.duration {
        from = Duration::ZERO;
    }
    let volume = audio.volume;
    let sink = Speakers::sink(world)?;
    sink.append(decoder(sound.bytes.clone())?);
    sink.set_volume(volume);
    let _ = sink.try_seek(from);
    sink.play();
    world.get_mut::<AudioPanel>(panel).unwrap().player = Some(Player {
        sink,
        sound: sound.clone(),
    });
    Ok(())
}

fn time(duration: Duration) -> String {
    let seconds = duration.as_secs();
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

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    audio: &mut AudioPanel,
    sound: &Arc<Analysis>,
    focused: bool,
) -> Asked {
    let id = egui::Id::new(("bedtime.audio", panel));
    let mut asked = Asked::default();
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::Vec2::splat(1.0)),
        egui::Sense::click_and_drag(),
    );
    if response.clicked() || response.secondary_clicked() {
        response.request_focus();
    }
    // At the end: loop, or stop there.
    if let Some(player) = &audio.player
        && player.sink.empty()
    {
        audio.position = sound.duration;
        audio.player = None;
        if audio.looping {
            audio.position = Duration::ZERO;
            asked.toggle = true;
        }
    }
    let playing = audio.playing();
    let position = audio.position().min(sound.duration);
    if playing {
        ui.ctx().request_repaint();
    }
    if focused && !ui.ctx().egui_wants_keyboard_input() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Space) {
                asked.toggle = true;
            }
            let skip = Duration::from_secs(5);
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                asked.seek = Some(position + skip);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                asked.seek = Some(position.saturating_sub(skip));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                asked.seek = Some(Duration::ZERO);
            }
        });
    }
    let p = context.theme.palette.clone();
    let content = egui::Rect::from_min_max(
        rect.min + egui::vec2(20.0, 52.0),
        rect.max - egui::vec2(20.0, 64.0),
    );
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let fraction =
        |at: Duration| (at.as_secs_f32() / sound.duration.as_secs_f32().max(1e-6)).clamp(0.0, 1.0);
    match audio.view {
        AudioView::Waveform => waveform(&painter, content, sound, fraction(position), &p),
        AudioView::Spectrum => {
            spectrum(ui, &painter, content, audio, sound, position, playing, &p);
            frequency_marks(&painter, content, sound, true, &p);
        }
        AudioView::Spectrogram => {
            spectrogram(ui, &painter, content, id, audio, sound, &p);
            frequency_marks(&painter, content, sound, false, &p);
            let x = content.left() + content.width() * fraction(position);
            painter.vline(
                x,
                content.y_range(),
                egui::Stroke::new(1.5, theme::color(p.text)),
            );
        }
    }
    // Seeking: click or drag along the time (not in the spectrum).
    if audio.view != AudioView::Spectrum {
        if let Some(pointer) = response.hover_pos()
            && content.x_range().contains(pointer.x)
        {
            let at = sound
                .duration
                .mul_f32((pointer.x - content.left()) / content.width());
            painter.vline(
                pointer.x,
                content.y_range(),
                egui::Stroke::new(
                    1.0,
                    theme::color(theme::mix(p.muted, p.panel_background, 0.3)),
                ),
            );
            painter.text(
                egui::pos2(pointer.x + 6.0, content.top()),
                egui::Align2::LEFT_TOP,
                time(at),
                egui::FontId::monospace(11.0),
                theme::color(p.muted),
            );
        }
        if (response.clicked() || response.dragged_by(egui::PointerButton::Primary))
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let x = ((pointer.x - content.left()) / content.width()).clamp(0.0, 1.0);
            asked.seek = Some(sound.duration.mul_f32(x));
        }
    } else if response.clicked() {
        asked.toggle = true;
    }
    response.context_menu(|ui| {
        if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
            asked.toggle = true;
        }
        if ui.button("Play from Start").clicked() {
            asked.seek = Some(Duration::ZERO);
            if !playing {
                asked.toggle = true;
            }
        }
        ui.checkbox(&mut audio.looping, "Loop");
        ui.separator();
        for (view, label) in [
            (AudioView::Waveform, "Waveform"),
            (AudioView::Spectrum, "Spectrum"),
            (AudioView::Spectrogram, "Spectrogram"),
        ] {
            if ui.radio(audio.view == view, label).clicked() {
                audio.view = view;
            }
        }
    });

    // The floating controls.
    let mut frame = context.frame(ui);
    Floating::new(id.with("transport"), rect, egui::Align2::CENTER_BOTTOM)
        .shown(!playing)
        .show(&mut frame, |f| {
            let (icon, tip) = if playing {
                (BuiltinIcon::Pause, "Pause (Space)")
            } else {
                (BuiltinIcon::Play, "Play (Space)")
            };
            if f.floating_button(icon, tip).clicked() {
                asked.toggle = true;
            }
            f.floating_label(format!("{} / {}", time(position), time(sound.duration)));
            if f.floating_toggle(BuiltinIcon::Repeat, "Loop", audio.looping)
                .clicked()
            {
                audio.looping = !audio.looping;
            }
            let icon = if audio.volume <= 0.0 {
                BuiltinIcon::VolumeOff
            } else {
                BuiltinIcon::Volume
            };
            let volume = f.floating_button(icon, "Volume");
            egui::Popup::menu(&volume)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let slider = egui::Slider::new(&mut audio.volume, 0.0..=1.0)
                        .show_value(false)
                        .vertical();
                    if ui.add(slider).changed()
                        && let Some(player) = &audio.player
                    {
                        player.sink.set_volume(audio.volume);
                    }
                });
        });
    Floating::new(id.with("views"), rect, egui::Align2::RIGHT_TOP).show(&mut frame, |f| {
        for (view, icon, tip) in [
            (AudioView::Waveform, BuiltinIcon::Waveform, "Waveform"),
            (AudioView::Spectrum, BuiltinIcon::Spectrum, "Spectrum"),
            (AudioView::Spectrogram, BuiltinIcon::Layers, "Spectrogram"),
        ] {
            if f.floating_toggle(icon, tip, audio.view == view).clicked() {
                audio.view = view;
            }
        }
    });
    let channels = match sound.channels {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        n => format!("{n} channels"),
    };
    Floating::new(id.with("info"), rect, egui::Align2::LEFT_TOP).show(&mut frame, |f| {
        f.floating_label(format!(
            "{:.1} kHz · {channels} · {}",
            sound.sample_rate as f32 / 1000.0,
            time(sound.duration)
        ));
    });
    asked
}

/// The waveform: each column's loudest peaks, mirrored about the middle.
/// What has played is in the accent color.
fn waveform(
    painter: &egui::Painter,
    rect: egui::Rect,
    sound: &Analysis,
    played: f32,
    p: &theme::Palette,
) {
    let columns = rect.width().max(1.0) as usize;
    let peaks = &sound.peaks;
    let middle = rect.center().y;
    let half = rect.height() / 2.0;
    let accent = theme::color(p.accent);
    let rest = theme::color(theme::mix(p.muted, p.panel_background, 0.35));
    painter.hline(
        rect.x_range(),
        middle,
        egui::Stroke::new(
            1.0,
            theme::color(theme::mix(p.border, p.panel_background, 0.4)),
        ),
    );
    let mut mesh = egui::Mesh::default();
    for column in 0..columns {
        let from = column * peaks.len() / columns;
        let to = ((column + 1) * peaks.len() / columns)
            .max(from + 1)
            .min(peaks.len());
        let [low, high] = peaks[from..to]
            .iter()
            .fold([0.0f32, 0.0f32], |[low, high], [l, h]| {
                [low.min(*l), high.max(*h)]
            });
        let x = rect.left() + column as f32;
        let top = middle - high.clamp(0.0, 1.0) * half;
        let bottom = middle - low.clamp(-1.0, 0.0) * half;
        let ink = if (column as f32) < played * columns as f32 {
            accent
        } else {
            rest
        };
        mesh.add_colored_rect(
            egui::Rect::from_min_max(
                egui::pos2(x, top),
                egui::pos2(x + 1.0, bottom.max(top + 1.0)),
            ),
            ink,
        );
    }
    painter.add(mesh);
    let x = rect.left() + rect.width() * played;
    painter.vline(
        x,
        rect.y_range(),
        egui::Stroke::new(1.5, theme::color(p.text)),
    );
}

/// Bars for frequencies (spaced as heard, 30 Hz up), from the spectrogram
/// where it's playing. They rise at once and fall smoothly; caps fall slower.
#[allow(clippy::too_many_arguments)]
fn spectrum(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    audio: &mut AudioPanel,
    sound: &Analysis,
    position: Duration,
    playing: bool,
    p: &theme::Palette,
) {
    let count = ((rect.width() / 14.0) as usize).clamp(16, 96);
    audio.bars.resize(count, [0.0, 0.0]);
    let column = sound.column(position);
    let nyquist = sound.sample_rate as f32 / 2.0;
    let (low, high) = (30.0f32, nyquist.min(20_000.0));
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    for (index, bar) in audio.bars.iter_mut().enumerate() {
        let from = low * (high / low).powf(index as f32 / count as f32);
        let to = low * (high / low).powf((index + 1) as f32 / count as f32);
        let bin = |f: f32| ((f / nyquist) * BINS as f32) as usize;
        let (a, b) = (bin(from).min(BINS - 1), bin(to).clamp(bin(from) + 1, BINS));
        let level = column
            .map(|column| column[a..b].iter().copied().max().unwrap_or(0) as f32 / 255.0)
            .unwrap_or(0.0);
        // Quieter than about -60 dB is nothing.
        let level = ((level - 0.35) / 0.65).clamp(0.0, 1.0);
        let level = if playing {
            level
        } else {
            level.max(bar[0] - dt * 1.5)
        };
        bar[0] = level.max(bar[0] - dt * 2.5);
        bar[1] = bar[0].max(bar[1] - dt * 0.6);
    }
    if audio.bars.iter().any(|bar| bar[1] > 0.0) {
        ui.ctx().request_repaint();
    }
    let gap = 3.0;
    let width = (rect.width() - gap * (count - 1) as f32) / count as f32;
    for (index, [level, cap]) in audio.bars.iter().copied().enumerate() {
        let x = rect.left() + index as f32 * (width + gap);
        let top = rect.bottom() - rect.height() * level;
        let ink = theme::color(theme::mix(p.accent, p.text, level * 0.5));
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, top), egui::pos2(x + width, rect.bottom())),
            egui::CornerRadius::same(2),
            ink.gamma_multiply(0.35 + 0.65 * level),
        );
        if cap < 0.01 {
            continue;
        }
        let cap_y = rect.bottom() - rect.height() * cap - 3.0;
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, cap_y), egui::vec2(width, 2.0)),
            1.0,
            theme::color(p.text).gamma_multiply(0.7),
        );
    }
}

/// Where `frequency` is, from 0 (30 Hz) to 1 (the top), spaced as heard.
fn frequency_at(sound: &Analysis, frequency: f32) -> f32 {
    let high = (sound.sample_rate as f32 / 2.0).min(20_000.0);
    (frequency / 30.0).ln() / (high / 30.0).ln()
}

/// 100 Hz, 1 kHz and 10 kHz: faint marks across (`along`) or up the view.
fn frequency_marks(
    painter: &egui::Painter,
    rect: egui::Rect,
    sound: &Analysis,
    along: bool,
    p: &theme::Palette,
) {
    let line = theme::color(theme::mix(p.border, p.panel_background, 0.5));
    let ink = theme::color(p.muted);
    for (frequency, label) in [(100.0, "100 Hz"), (1000.0, "1 kHz"), (10_000.0, "10 kHz")] {
        let t = frequency_at(sound, frequency);
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        if along {
            let x = rect.left() + rect.width() * t;
            painter.text(
                egui::pos2(x, rect.bottom() + 6.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::monospace(10.0),
                ink,
            );
        } else {
            let y = rect.bottom() - rect.height() * t;
            painter.hline(
                rect.x_range(),
                y,
                egui::Stroke::new(1.0, line.gamma_multiply(0.6)),
            );
            painter.text(
                egui::pos2(rect.left() + 4.0, y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::monospace(10.0),
                ink,
            );
        }
    }
}

/// The spectrogram: time across, frequency up (spaced as heard), loudness
/// as color, made once for each file.
#[allow(clippy::too_many_arguments)]
fn spectrogram(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    id: egui::Id,
    audio: &mut AudioPanel,
    sound: &Arc<Analysis>,
    p: &theme::Palette,
) {
    let key = Arc::as_ptr(sound) as usize;
    if audio.spectrogram.as_ref().is_none_or(|(at, _)| *at != key) {
        let image = spectrogram_image(sound, p);
        let texture = ui.ctx().load_texture(
            format!("{id:?}-spectrogram"),
            image,
            egui::TextureOptions::LINEAR,
        );
        audio.spectrogram = Some((key, texture));
    }
    let (_, texture) = audio.spectrogram.as_ref().unwrap();
    painter.image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

const ROWS: usize = 256;

fn spectrogram_image(sound: &Analysis, p: &theme::Palette) -> egui::ColorImage {
    let columns = sound.spectrogram.len().clamp(1, 4096);
    // From silence (the panel) through the accent and warning to white.
    let stops = [
        p.panel_background,
        theme::mix(p.accent, p.panel_background, 0.6),
        p.accent,
        p.warning,
        [1.0, 1.0, 1.0, 1.0],
    ];
    let ramp: Vec<egui::Color32> = (0..256)
        .map(|level| {
            let t = level as f32 / 255.0 * (stops.len() - 1) as f32;
            let i = (t.floor() as usize).min(stops.len() - 2);
            let mut color = theme::mix(stops[i], stops[i + 1], t - i as f32);
            color[3] = 1.0;
            theme::color(color)
        })
        .collect();
    let nyquist = sound.sample_rate as f32 / 2.0;
    let (low, high) = (30.0f32, nyquist.min(20_000.0));
    let bins: Vec<usize> = (0..ROWS)
        .map(|row| {
            // Row 0 is the top: the highest frequency.
            let t = 1.0 - row as f32 / (ROWS - 1) as f32;
            let f = low * (high / low).powf(t);
            (((f / nyquist) * BINS as f32) as usize).min(BINS - 1)
        })
        .collect();
    let mut pixels = vec![egui::Color32::TRANSPARENT; columns * ROWS];
    let total = sound.spectrogram.len();
    for column in 0..columns {
        let from = column * total / columns;
        let to = ((column + 1) * total / columns).max(from + 1).min(total);
        for (row, &bin) in bins.iter().enumerate() {
            let level = sound.spectrogram[from..to]
                .iter()
                .map(|spectrum| spectrum[bin])
                .max()
                .unwrap_or(0);
            pixels[row * columns + column] = ramp[level as usize];
        }
    }
    egui::ColorImage::new([columns, ROWS], pixels)
}
