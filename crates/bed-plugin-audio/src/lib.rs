//! Read-only audio preview, with background decoding and native playback.
use bed_editing::identity::DocumentId;
use bed_plugin::{
    CommandContext, DocumentKind, HostContext, HostRequest, MenuSlot, Plugin, PluginPanel,
    Registrar, Revision,
};
use dear_imgui_rs::{Key, MouseButton, StyleColor, Ui};
use rodio::{OutputStream, OutputStreamBuilder, Sink};
use serde_json::{Value, json};
use std::{
    any::Any,
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

mod audio;
use audio::{Audio, PcmSource};
#[cfg(test)]
mod tests;

pub const PANEL_ID: &str = "bed.audio.panel";
pub const VIEWER_ID: &str = "bed.audio.viewer";
pub const OPEN_COMMAND: &str = "bed.audio.open";
pub const OPEN_FILE_COMMAND: &str = "bed.audio.open-file";
const EXTENSIONS: &[&str] = &[
    "wav", "wave", "mp3", "flac", "ogg", "oga", "aac", "m4a", "mp4", "aif", "aiff", "aifc", "caf",
];

pub struct AudioPlugin;
impl Plugin for AudioPlugin {
    fn id(&self) -> &'static str {
        "bed.audio"
    }
    fn register(&self, r: &mut Registrar<'_>) {
        r.panel(PANEL_ID, "Audio Viewer");
        r.viewer(
            VIEWER_ID,
            "Audio Viewer",
            PANEL_ID,
            EXTENSIONS,
            DocumentKind::Bytes,
        );
        r.command(OPEN_COMMAND, "Open Audio…", None);
        r.command(OPEN_FILE_COMMAND, "Open in Audio Viewer", None);
        r.menu(MenuSlot::Application, OPEN_COMMAND);
        r.menu(MenuSlot::File, OPEN_FILE_COMMAND);
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
    ) -> bool {
        command != OPEN_FILE_COMMAND
            || context.path.as_deref().is_some_and(|p| {
                p.rsplit(['/', '\\'])
                    .next()
                    .and_then(|n| n.rsplit_once('.'))
                    .is_some_and(|(_, e)| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
            })
    }
    fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        match command {
            OPEN_COMMAND => requests.push(HostRequest::OpenFileDialog {
                viewer: Some(VIEWER_ID.into()),
            }),
            OPEN_FILE_COMMAND => {
                if let Some(path) = &context.path {
                    requests.push(HostRequest::OpenFile {
                        path: path.clone(),
                        viewer: Some(VIEWER_ID.into()),
                    });
                }
            }
            _ => {}
        }
    }
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        if kind != PANEL_ID {
            return Err(format!("Unknown audio panel: {kind}"));
        }
        Ok(Box::new(AudioPanel::new(
            document.ok_or("Audio panels require a document")?,
            state,
        )))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct Job {
    revision: Revision,
    serial: u64,
    bytes: Arc<[u8]>,
}
struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<(Revision, Result<Arc<Audio>, String>)>,
    serial: Arc<AtomicU64>,
}
impl Worker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::channel();
        let serial = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&serial);
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let result = audio::decode(Cursor::new(job.bytes), || {
                    active.load(Ordering::Relaxed) != job.serial
                })
                .map(Arc::new);
                if active.load(Ordering::Relaxed) == job.serial
                    && results.send((job.revision, result)).is_err()
                {
                    break;
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
            serial,
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.serial.fetch_add(1, Ordering::Relaxed);
        self.sender.take();
    }
}
struct Playback {
    // Drop the sink before the stream that services it.
    sink: Sink,
    _stream: OutputStream,
}
pub struct AudioPanel {
    document: DocumentId,
    worker: Worker,
    revision: Option<Revision>,
    submitted: Option<Revision>,
    audio: Option<Arc<Audio>>,
    playback: Option<Playback>,
    error: Option<String>,
    position: f64,
    volume: f32,
}
impl AudioPanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        Self {
            document,
            worker: Worker::new(),
            revision: None,
            submitted: None,
            audio: None,
            playback: None,
            error: None,
            position: state["position"]
                .as_f64()
                .filter(|p| p.is_finite())
                .unwrap_or(0.0)
                .max(0.0),
            volume: state["volume"]
                .as_f64()
                .filter(|p| p.is_finite())
                .unwrap_or(0.8)
                .clamp(0.0, 1.0) as f32,
        }
    }
    fn sync(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.playback = None;
            return;
        };
        if self.revision != Some(document.revision) {
            self.worker.serial.fetch_add(1, Ordering::Relaxed);
            if self.revision.is_some() {
                self.position = 0.0;
            }
            self.revision = Some(document.revision);
            self.submitted = None;
            self.playback = None;
            self.audio = None;
            self.error = None;
        }
        if self.submitted != self.revision {
            let job = Job {
                revision: document.revision,
                serial: self.worker.serial.load(Ordering::Relaxed),
                bytes: Arc::clone(&document.bytes),
            };
            if self
                .worker
                .sender
                .as_ref()
                .is_some_and(|s| s.try_send(job).is_ok())
            {
                self.submitted = self.revision;
            }
        }
        while let Ok((revision, result)) = self.worker.receiver.try_recv() {
            if Some(revision) != self.revision {
                continue;
            }
            match result {
                Ok(audio) => {
                    self.position = self.position.min(audio.duration());
                    self.audio = Some(audio);
                }
                Err(error) => self.error = Some(error),
            }
        }
        self.position = self.current_position();
    }
    fn current_position(&self) -> f64 {
        self.playback.as_ref().map_or(self.position, |p| {
            let duration = self.audio.as_ref().map_or(0.0, |a| a.duration());
            if p.sink.empty() {
                duration
            } else {
                p.sink.get_pos().as_secs_f64().min(duration)
            }
        })
    }
    fn playing(&self) -> bool {
        self.playback
            .as_ref()
            .is_some_and(|p| !p.sink.is_paused() && !p.sink.empty())
    }
    fn toggle(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        if self.playing() {
            self.playback.as_ref().unwrap().sink.pause();
            return;
        }
        if self.position >= audio.duration() {
            self.position = 0.0;
        }
        if self.playback.is_none() {
            match OutputStreamBuilder::open_default_stream() {
                Ok(mut stream) => {
                    stream.log_on_drop(false);
                    let sink = Sink::connect_new(stream.mixer());
                    sink.pause();
                    self.playback = Some(Playback {
                        sink,
                        _stream: stream,
                    });
                }
                Err(error) => {
                    self.error = Some(format!("Audio output unavailable: {error}"));
                    return;
                }
            }
        }
        let p = self.playback.as_ref().unwrap();
        if p.sink.empty() {
            p.sink.append(PcmSource::new(Arc::clone(audio)));
            if let Err(error) = p.sink.try_seek(Duration::from_secs_f64(self.position)) {
                self.error = Some(error.to_string());
            }
        }
        p.sink.set_volume(self.volume);
        p.sink.play();
        self.error = None;
    }
    fn seek(&mut self, position: f64) {
        let Some(audio) = &self.audio else {
            return;
        };
        self.position = position.clamp(0.0, audio.duration());
        if let Some(p) = &self.playback {
            if p.sink.empty() {
                p.sink.pause();
                p.sink.append(PcmSource::new(Arc::clone(audio)));
            }
            if let Err(error) = p.sink.try_seek(Duration::from_secs_f64(self.position)) {
                self.error = Some(format!("Unable to seek: {error}"));
            }
        }
    }
    fn waveform(&mut self, ui: &Ui, audio: &Audio) {
        let min = ui.cursor_screen_pos();
        let size = [
            ui.content_region_avail()[0].max(1.0),
            ui.content_region_avail()[1]
                .max(80.0)
                .min(110.0 * audio.channels as f32),
        ];
        ui.invisible_button("##waveform", size);
        if ui.is_item_active() && ui.is_mouse_down(MouseButton::Left) {
            self.seek(
                ((ui.io().mouse_pos()[0] - min[0]) / size[0]).clamp(0.0, 1.0) as f64
                    * audio.duration(),
            );
        }
        if ui.is_item_hovered() {
            ui.tooltip_text("Click or drag to seek");
        }
        let draw = ui.get_window_draw_list();
        let max = [min[0] + size[0], min[1] + size[1]];
        draw.add_rect(min, max, ui.style_color(StyleColor::FrameBg))
            .filled(true)
            .build();
        let play_x = min[0] + size[0] * (self.position / audio.duration()) as f32;
        let mut tint = ui.style_color(StyleColor::SliderGrabActive);
        tint[3] = 0.12;
        draw.add_rect(min, [play_x, max[1]], tint)
            .filled(true)
            .build();
        let height = size[1] / audio.channels as f32;
        let columns = (size[0].ceil() as usize).min(audio.peaks[0].len()).max(1);
        for (channel, peaks) in audio.peaks.iter().enumerate() {
            let center = min[1] + height * (channel as f32 + 0.5);
            draw.add_line(
                [min[0], center],
                [max[0], center],
                ui.style_color(StyleColor::Border),
            )
            .build();
            for column in 0..columns {
                let start = column * peaks.len() / columns;
                let end = ((column + 1) * peaks.len() / columns).max(start + 1);
                let (lo, hi) = peaks[start..end]
                    .iter()
                    .fold((0.0_f32, 0.0_f32), |(lo, hi), p| (lo.min(p.0), hi.max(p.1)));
                let x = min[0] + size[0] * (column as f32 + 0.5) / columns as f32;
                draw.add_line(
                    [x, center - hi * height * 0.4],
                    [
                        x,
                        (center - lo * height * 0.4).max(center - hi * height * 0.4 + 1.0),
                    ],
                    ui.style_color(StyleColor::PlotHistogram),
                )
                .build();
            }
            draw.add_text(
                [min[0] + 8.0, min[1] + height * channel as f32 + 4.0],
                ui.style_color(StyleColor::TextDisabled),
                format!("CH {}", channel + 1),
            );
        }
        draw.add_line(
            [play_x, min[1]],
            [play_x, max[1]],
            ui.style_color(StyleColor::SliderGrabActive),
        )
        .thickness(2.0)
        .build();
    }
}
impl PluginPanel for AudioPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document)
            .map(|d| {
                d.path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&d.path)
                    .to_owned()
            })
            .unwrap_or_else(|| "Audio Viewer".into())
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        self.sync(host);
        let Some(audio) = self.audio.clone() else {
            ui.text_wrapped(self.error.as_deref().unwrap_or("Decoding audio…"));
            return;
        };
        if ui.button(if self.playing() { "Pause" } else { "Play" })
            || (ui.is_window_focused()
                && !ui.io().want_text_input()
                && ui.is_key_pressed(Key::Space))
        {
            self.toggle();
        }
        ui.same_line();
        if ui.button("Restart") {
            self.seek(0.0);
        }
        ui.same_line();
        ui.text(format!(
            "{} / {}",
            audio::time_label(self.position),
            audio::time_label(audio.duration())
        ));
        ui.same_line();
        ui.set_next_item_width(110.0);
        if ui.slider_config("Volume", 0.0, 1.0).build(&mut self.volume)
            && let Some(p) = &self.playback
        {
            p.sink.set_volume(self.volume);
        }
        ui.text_disabled(format!(
            "{} Hz · {} channel{} · {} frames",
            audio.sample_rate,
            audio.channels,
            if audio.channels == 1 { "" } else { "s" },
            audio.frames()
        ));
        if let Some(error) = &self.error {
            ui.text_wrapped(error);
        }
        let mut position = self.position as f32;
        ui.set_next_item_width(ui.content_region_avail()[0]);
        if ui
            .slider_config("##seek", 0.0, audio.duration() as f32)
            .build(&mut position)
        {
            self.seek(position as f64);
        }
        self.waveform(ui, &audio);
    }
    fn save_state(&self) -> Value {
        json!({"position":self.current_position(),"volume":self.volume})
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.playback = None;
        self.worker.serial.fetch_add(1, Ordering::Relaxed);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
