//! Read-only audio preview, with background decoding and native playback.
use bed_editing::identity::DocumentId;
use bed_editor_api::gpu::GpuContext;
use bed_editor_api::{DocumentKind, HostContext, HostRequest, Registrar, Revision};
use bed_ui::TextureHandle;
use bed_ui::gpu::{RenderOutput, RenderTarget};
use bed_ui::util::popup_style::tooltip_text;
use dear_imgui_rs::{Condition, MouseButton, StyleColor, TreeNodeFlags, Ui};
use rodio::{OutputStream, OutputStreamBuilder, Sink};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

mod ecs;
mod status;
pub use ecs::{AudioDocument, AudioPlugin, AudioStates, AudioSystems};

mod analysis;
mod audio;
mod render;
use audio::{Audio, PcmSource};
#[cfg(test)]
mod tests;

pub const PLUGIN_ID: &str = "bed.audio";
pub const PANEL_ID: &str = "bed.audio.panel";
pub const VIEWER_ID: &str = "bed.audio.viewer";
pub const OPEN_COMMAND: &str = "bed.audio.open";
pub const OPEN_FILE_COMMAND: &str = "bed.audio.open-file";

const EXTENSIONS: &[&str] = &[
    "wav", "wave", "mp3", "flac", "ogg", "oga", "aac", "m4a", "mp4", "aif", "aiff", "aifc", "caf",
];

fn register_metadata(r: &mut Registrar<'_>) {
    r.panel(PANEL_ID, "Audio Viewer");
    r.panel_requires_input(PANEL_ID);
    r.viewer(
        VIEWER_ID,
        "Audio Viewer",
        PANEL_ID,
        EXTENSIONS,
        DocumentKind::Bytes,
    );
    r.command(OPEN_COMMAND, "Open Audio…", None);
    r.command(OPEN_FILE_COMMAND, "Open in Audio Viewer", None);
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .is_some_and(|(_, e)| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

struct Job {
    revision: Revision,
    serial: u64,
    bytes: Arc<[u8]>,
}

struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<(Revision, WorkerResult)>,
    serial: Arc<AtomicU64>,
    spectrum_sender: Option<mpsc::SyncSender<SpectrumJob>>,
    spectrum_receiver: mpsc::Receiver<(Revision, usize, [u8; analysis::BANDS])>,
}

struct SpectrumJob {
    revision: Revision,
    serial: u64,
    center: usize,
    audio: Arc<Audio>,
}

enum WorkerResult {
    Decoded(Result<Arc<Audio>, String>),
    Analyzed(Result<analysis::Analysis, String>),
}

impl Worker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::channel();
        let serial = Arc::new(AtomicU64::new(0));
        let (spectrum_sender, spectrum_jobs) = mpsc::sync_channel::<SpectrumJob>(1);
        let (spectrum_results, spectrum_receiver) = mpsc::channel();
        let active = Arc::clone(&serial);
        thread::spawn(move || {
            let mut computer: Option<(u64, analysis::SpectrumComputer)> = None;
            while let Ok(mut job) = spectrum_jobs.recv() {
                while let Ok(newer) = spectrum_jobs.try_recv() {
                    job = newer;
                }
                if active.load(Ordering::Relaxed) != job.serial {
                    continue;
                }
                if computer
                    .as_ref()
                    .is_none_or(|(serial, _)| *serial != job.serial)
                {
                    computer = Some((job.serial, analysis::SpectrumComputer::new(&job.audio)));
                }
                let result = computer
                    .as_mut()
                    .unwrap()
                    .1
                    .compute(&job.audio, job.center, || {
                        active.load(Ordering::Relaxed) != job.serial
                    });
                if let Ok(levels) = result
                    && active.load(Ordering::Relaxed) == job.serial
                    && spectrum_results
                        .send((job.revision, job.center, levels))
                        .is_err()
                {
                    break;
                }
            }
        });
        let active = Arc::clone(&serial);
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let result = audio::decode(Cursor::new(job.bytes), || {
                    active.load(Ordering::Relaxed) != job.serial
                })
                .map(Arc::new);
                if active.load(Ordering::Relaxed) != job.serial {
                    continue;
                }
                let decoded = result.as_ref().ok().cloned();
                if results
                    .send((job.revision, WorkerResult::Decoded(result)))
                    .is_err()
                {
                    break;
                }
                if let Some(audio) = decoded {
                    let result =
                        analysis::analyze(&audio, || active.load(Ordering::Relaxed) != job.serial);
                    if active.load(Ordering::Relaxed) == job.serial
                        && results
                            .send((job.revision, WorkerResult::Analyzed(result)))
                            .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
            serial,
            spectrum_sender: Some(spectrum_sender),
            spectrum_receiver,
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.serial.fetch_add(1, Ordering::Relaxed);
        self.sender.take();
        self.spectrum_sender.take();
    }
}

struct Playback {
    // drop the sink before the stream that services it
    sink: Sink,
    _stream: OutputStream,
}

pub struct AudioPanel {
    pub(crate) shortcuts: bed_keybindings::ShortcutInput,
    document: DocumentId,
    worker: Worker,
    revision: Option<Revision>,
    submitted: Option<Revision>,
    audio: Option<Arc<Audio>>,
    playback: Option<Playback>,
    error: Option<String>,
    position: f64,
    volume: f32,
    sections: [bool; 3],
    analysis: Option<analysis::Analysis>,
    analysis_error: Option<String>,
    texture: TextureHandle,
    output_revision: u64,
    spectrogram_visible: bool,
    gpu: Option<render::SpectrogramGpu>,
    gpu_error: Option<String>,
    spectrum: Option<[u8; analysis::BANDS]>,
    spectrum_submitted: Option<usize>,
    spectrum_ready: Option<usize>,
}

impl AudioPanel {
    fn new(document: DocumentId, state: &Value) -> Self {
        Self {
            shortcuts: crate::default_shortcuts(),
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
            sections: ["waveform", "spectrum", "spectrogram"]
                .map(|section| state[section].as_bool().unwrap_or(true)),
            analysis: None,
            analysis_error: None,
            texture: TextureHandle::next(),
            output_revision: 0,
            spectrogram_visible: false,
            gpu: None,
            gpu_error: None,
            spectrum: None,
            spectrum_submitted: None,
            spectrum_ready: None,
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
            self.analysis = None;
            self.analysis_error = None;
            self.gpu = None;
            self.gpu_error = None;
            self.spectrum = None;
            self.spectrum_submitted = None;
            self.spectrum_ready = None;
            self.output_revision = self.output_revision.wrapping_add(1);
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
                WorkerResult::Decoded(Ok(audio)) => {
                    self.position = self.position.min(audio.duration());
                    self.audio = Some(audio);
                }
                WorkerResult::Decoded(Err(error)) => self.error = Some(error),
                WorkerResult::Analyzed(Ok(analysis)) => {
                    self.analysis = Some(analysis);
                    self.output_revision = self.output_revision.wrapping_add(1);
                }
                WorkerResult::Analyzed(Err(error)) => self.analysis_error = Some(error),
            }
        }
        self.position = self.current_position();
        while let Ok((revision, center, levels)) = self.worker.spectrum_receiver.try_recv() {
            if Some(revision) == self.revision && Some(center) == self.spectrum_submitted {
                self.spectrum = Some(levels);
                self.spectrum_ready = Some(center);
            }
        }
        self.request_spectrum();
    }

    fn request_spectrum(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        if !self.sections[1] {
            return;
        }
        let center = self
            .spectrum_center()
            .expect("Decoded audio has a spectrum position");
        if self.spectrum_submitted == Some(center) {
            return;
        }
        let Some(revision) = self.revision else {
            return;
        };
        let job = SpectrumJob {
            revision,
            serial: self.worker.serial.load(Ordering::Relaxed),
            center,
            audio: Arc::clone(audio),
        };
        if self
            .worker
            .spectrum_sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(job).is_ok())
        {
            self.spectrum_submitted = Some(center);
        }
    }

    fn spectrum_center(&self) -> Option<usize> {
        self.audio.as_ref().map(|audio| {
            (self.position * audio.sample_rate as f64) as usize / analysis::HOP * analysis::HOP
        })
    }

    fn spectrum_pending(&self) -> bool {
        self.sections[1] && self.audio.is_some() && self.spectrum_center() != self.spectrum_ready
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
        self.request_spectrum();
    }

    fn waveform(&mut self, ui: &Ui, audio: &Audio) {
        let min = ui.cursor_screen_pos();
        let size = [
            ui.content_region_avail()[0].max(1.0),
            (80.0 * audio.channels as f32).clamp(80.0, 240.0),
        ];
        ui.invisible_button("##waveform", size);
        if ui.is_item_active() && ui.is_mouse_down(MouseButton::Left) {
            self.seek(
                ((ui.io().mouse_pos()[0] - min[0]) / size[0]).clamp(0.0, 1.0) as f64
                    * audio.duration(),
            );
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Click or drag to seek");
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

    fn section(&mut self, ui: &Ui, index: usize, label: &str) -> bool {
        ui.set_next_item_open_with_cond(self.sections[index], Condition::Always);
        self.sections[index] = ui.collapsing_header(label, TreeNodeFlags::empty());
        self.sections[index]
    }

    fn spectrum(&self, ui: &Ui, audio: &Audio) {
        let levels = self
            .spectrum
            .as_ref()
            .map(|levels| levels.as_slice())
            .or_else(|| {
                self.analysis
                    .as_ref()
                    .map(|analysis| analysis.spectrum(self.position / audio.duration()))
            });
        let Some(levels) = levels else {
            ui.text_disabled(
                self.analysis_error
                    .as_deref()
                    .unwrap_or("Analyzing frequencies…"),
            );
            return;
        };
        let maximum_hz = audio.sample_rate as f32 * 0.5;
        let minimum_hz = 20.0_f32.min(maximum_hz * 0.5);
        let frequency_fraction = |hz: f32| (hz / minimum_hz).ln() / (maximum_hz / minimum_hz).ln();
        let min = ui.cursor_screen_pos();
        let size = [ui.content_region_avail()[0].max(1.0), 170.0];
        ui.invisible_button("##spectrum", size);
        let draw = ui.get_window_draw_list();
        let max = [min[0] + size[0], min[1] + size[1]];
        draw.add_rect(min, max, ui.style_color(StyleColor::FrameBg))
            .filled(true)
            .build();
        let plot_min = [min[0] + 34.0_f32.min(size[0] * 0.3), min[1] + 12.0];
        let plot_max = [max[0] - 8.0_f32.min(size[0] * 0.1), max[1] - 22.0];
        for db in [0.0, -30.0, -60.0, analysis::FLOOR_DB] {
            let y = plot_min[1] + db / analysis::FLOOR_DB * (plot_max[1] - plot_min[1]);
            draw.add_line(
                [plot_min[0], y],
                [plot_max[0], y],
                ui.style_color(StyleColor::Border),
            )
            .build();
            draw.add_text(
                [min[0] + 3.0, y - 6.0],
                ui.style_color(StyleColor::TextDisabled),
                format!("{db:.0}"),
            );
        }
        for hz in [20.0, 100.0, 1000.0, 10000.0] {
            if hz < minimum_hz || hz > maximum_hz {
                continue;
            }
            let x = plot_min[0] + frequency_fraction(hz) * (plot_max[0] - plot_min[0]);
            draw.add_line(
                [x, plot_min[1]],
                [x, plot_max[1]],
                ui.style_color(StyleColor::Border),
            )
            .build();
            draw.add_text(
                [x, plot_max[1] + 4.0],
                ui.style_color(StyleColor::TextDisabled),
                frequency_label(hz),
            );
        }
        for band in 1..analysis::BANDS {
            let point = |index: usize| {
                [
                    plot_min[0]
                        + index as f32 / (analysis::BANDS - 1) as f32 * (plot_max[0] - plot_min[0]),
                    plot_max[1] - levels[index] as f32 / 255.0 * (plot_max[1] - plot_min[1]),
                ]
            };
            draw.add_line(
                point(band - 1),
                point(band),
                ui.style_color(StyleColor::PlotHistogram),
            )
            .thickness(1.5)
            .build();
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Logarithmic frequency · dBFS · mean channel power");
        }
    }

    fn spectrogram(&mut self, ui: &Ui, host: &HostContext<'_>, audio: &Audio) {
        if self.analysis.is_none() {
            ui.text_disabled(
                self.analysis_error
                    .as_deref()
                    .unwrap_or("Building spectrogram…"),
            );
            return;
        }
        let min = ui.cursor_screen_pos();
        let size = [ui.content_region_avail()[0].max(1.0), 220.0];
        ui.invisible_button("##spectrogram", size);
        if ui.is_item_active() && ui.is_mouse_down(MouseButton::Left) {
            self.seek(
                ((ui.io().mouse_pos()[0] - min[0]) / size[0]).clamp(0.0, 1.0) as f64
                    * audio.duration(),
            );
        }
        let hovered = ui.is_item_hovered();
        let analysis = self.analysis.as_ref().unwrap();
        let max = [min[0] + size[0], min[1] + size[1]];
        let draw = ui.get_window_draw_list();
        self.spectrogram_visible = ui.is_item_visible();
        if !self.spectrogram_visible {
            return;
        }
        if let Some(texture) = host.texture(self.texture) {
            draw.add_image(texture, min, max, [0.0; 2], [1.0; 2], [1.0; 4]);
        } else {
            // Native render targets appear on the following frame. This bounded
            // preview also keeps the panel useful without a GPU backend.
            let columns = analysis.columns.min(128);
            let bands = 64;
            for column in 0..columns {
                for band in 0..bands {
                    let source_column = column * analysis.columns / columns;
                    let source_band = band * analysis::BANDS / bands;
                    let level = analysis.levels[source_column * analysis::BANDS + source_band];
                    let rgba =
                        analysis::heat_color(level).map(|component| component as f32 / 255.0);
                    let x = min[0] + column as f32 / columns as f32 * size[0];
                    let y = max[1] - (band + 1) as f32 / bands as f32 * size[1];
                    draw.add_rect(
                        [x, y],
                        [
                            x + size[0] / columns as f32 + 0.5,
                            y + size[1] / bands as f32 + 0.5,
                        ],
                        rgba,
                    )
                    .filled(true)
                    .build();
                }
            }
        }
        for hz in [100.0, 1000.0, 10000.0] {
            if hz < analysis.minimum_hz || hz > analysis.maximum_hz {
                continue;
            }
            let y = max[1] - analysis.frequency_fraction(hz) * size[1];
            draw.add_text(
                [min[0] + 5.0, y - 7.0],
                [1.0, 1.0, 1.0, 0.8],
                frequency_label(hz),
            );
        }
        let play_x = min[0] + size[0] * (self.position / audio.duration()) as f32;
        draw.add_line([play_x, min[1]], [play_x, max[1]], [1.0; 4])
            .thickness(2.0)
            .build();
        if hovered {
            let fraction = ((ui.io().mouse_pos()[1] - min[1]) / size[1]).clamp(0.0, 1.0);
            let hz = analysis.minimum_hz
                * (analysis.maximum_hz / analysis.minimum_hz).powf(1.0 - fraction);
            tooltip_text(
                ui,
                format!("{} · click or drag to seek", frequency_label(hz)),
            );
        }
        ui.text_disabled("Time → · logarithmic frequency ↑ · −90 to 0 dBFS");
        if let Some(error) = &self.gpu_error {
            ui.text_disabled(format!("Spectrogram preview: {error}"));
        }
    }
}

fn frequency_label(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

impl AudioPanel {
    pub fn title(&self, host: &HostContext<'_>) -> String {
        self.panel_title(host)
    }

    pub fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.sync(host);
        self.draw_prepared(ui, host, requests);
    }

    pub fn render_output(&self) -> Option<RenderOutput> {
        self.canvas_output()
    }

    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        self.record_render(gpu, target)
    }

    pub fn save_state(&self) -> Value {
        self.saved_state()
    }

    pub fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }

    pub fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.playback = None;
        self.worker.serial.fetch_add(1, Ordering::Relaxed);
        self.worker.sender.take();
        self.worker.spectrum_sender.take();
        self.analysis = None;
        self.gpu = None;
    }
}

impl AudioPanel {
    fn panel_title(&self, host: &HostContext<'_>) -> String {
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

    fn saved_state(&self) -> Value {
        json!({"position":self.current_position(),"volume":self.volume,
            "waveform":self.sections[0],"spectrum":self.sections[1],"spectrogram":self.sections[2]})
    }

    fn canvas_output(&self) -> Option<RenderOutput> {
        if !self.spectrogram_visible {
            return None;
        }
        let analysis = self.analysis.as_ref()?;
        Some(RenderOutput {
            handle: self.texture,
            size: [analysis.columns as u32, analysis::BANDS as u32],
            depth: false,
            revision: self.output_revision,
        })
    }

    fn record_render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        let analysis = self
            .analysis
            .as_ref()
            .ok_or("Audio has not been analyzed")?;
        if self
            .gpu
            .as_ref()
            .is_none_or(|state| state.generation != gpu.generation)
        {
            match render::SpectrogramGpu::new(gpu, analysis) {
                Ok(state) => {
                    self.gpu = Some(state);
                    self.gpu_error = None;
                }
                Err(error) => {
                    self.gpu_error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        self.gpu.as_ref().unwrap().render(gpu, target);
        Ok(())
    }

    fn draw_prepared(&mut self, ui: &Ui, host: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        self.spectrogram_visible = false;
        let Some(audio) = self.audio.clone() else {
            ui.text_wrapped(self.error.as_deref().unwrap_or("Decoding audio…"));
            return;
        };
        if ui.button(if self.playing() { "Pause" } else { "Play" })
            || (ui.is_window_focused()
                && !ui.io().want_text_input()
                && !ui.is_any_item_active()
                && self.shortcuts.take(ui, "bed.audio.toggle_playback"))
        {
            self.toggle();
        }
        ui.same_line();
        if ui.button("Restart")
            || (ui.is_window_focused()
                && !ui.is_any_item_active()
                && self.shortcuts.take(ui, "bed.audio.restart"))
        {
            self.seek(0.0);
        }
        ui.same_line();
        ui.set_next_item_width(110.0);
        if ui.slider_config("Volume", 0.0, 1.0).build(&mut self.volume)
            && let Some(p) = &self.playback
        {
            p.sink.set_volume(self.volume);
        }
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
        if self.section(ui, 0, "Waveform") {
            self.waveform(ui, &audio);
        }
        if self.section(ui, 1, "Spectrum") {
            self.spectrum(ui, &audio);
        }
        if self.section(ui, 2, "Spectrogram") {
            self.spectrogram(ui, host, &audio);
        }
    }
}

pub(crate) fn shortcut_actions() -> Vec<bed_keybindings::Action> {
    vec![
        bed_keybindings::Action::new(
            "bed.audio.toggle_playback",
            "Play / Pause",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Space"],
        ),
        bed_keybindings::Action::new(
            "bed.audio.restart",
            "Restart Playback",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &[],
        ),
    ]
}

pub(crate) fn default_shortcuts() -> bed_keybindings::ShortcutInput {
    bed_keybindings::ShortcutInput::new(bed_keybindings::Bindings::new(
        shortcut_actions(),
        &Default::default(),
        bed_keybindings::Platform::current(),
    ))
    .with_scope(bed_keybindings::Scope::panel(PANEL_ID))
}
