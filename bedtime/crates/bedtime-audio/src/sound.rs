//! A sound file, read on the document's load worker: its bytes (played as
//! they are) and what's drawn from them, worked out once. Samples are mixed
//! to mono; only summaries are kept, so long files stay small: the
//! waveform's peaks, and a spectrogram of loudness (dB) by frequency.
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
use rodio::Source;
use rustfft::{FftPlanner, num_complex::Complex};
use std::{io::Cursor, sync::Arc, time::Duration};

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "flac", "ogg", "oga", "m4a", "aac", "aif", "aiff", "caf",
];

/// Samples per waveform peak.
pub const PEAK_HOP: usize = 256;
/// Samples between spectrogram columns, and the window each looks at.
pub const SPECTRUM_HOP: usize = 1024;
pub const SPECTRUM_WINDOW: usize = 2048;
/// Frequency bins kept per column (of `SPECTRUM_WINDOW / 2`).
pub const BINS: usize = 512;
/// The quietest loudness drawn, in dB.
pub const FLOOR_DB: f32 = -96.0;

#[derive(Clone, Debug, Default)]
pub struct Sound(pub Option<Arc<Analysis>>);

#[derive(Debug)]
pub struct Analysis {
    pub bytes: Arc<[u8]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub duration: Duration,
    /// Each `PEAK_HOP` samples' lowest and highest.
    pub peaks: Vec<[f32; 2]>,
    /// Each `SPECTRUM_HOP` samples: `BINS` loudnesses, 0 (`FLOOR_DB`) to 255 (0 dB).
    pub spectrogram: Vec<[u8; BINS]>,
}

impl PartialEq for Sound {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl DocumentModel for Sound {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl FileModel for Sound {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Analysis::read(bytes.into()).map(|analysis| Self(Some(Arc::new(analysis))))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        match &self.0 {
            Some(analysis) => out.write_all(&analysis.bytes),
            None => Ok(()),
        }
    }
}

/// A decoder for `bytes`, seekable.
pub fn decoder(bytes: Arc<[u8]>) -> Result<rodio::Decoder<Cursor<Arc<[u8]>>>, String> {
    let len = bytes.len() as u64;
    rodio::Decoder::builder()
        .with_data(Cursor::new(bytes))
        .with_byte_len(len)
        .with_seekable(true)
        .build()
        .map_err(|_| "It isn't audio this can play".to_owned())
}

impl Analysis {
    pub fn read(bytes: Arc<[u8]>) -> Result<Self, String> {
        let decoder = decoder(bytes.clone())?;
        let channels = decoder.channels().max(1);
        let sample_rate = decoder.sample_rate();
        let mut analyzer = Analyzer::new();
        let mut frame = 0.0;
        let mut in_frame = 0;
        let mut samples = 0u64;
        for sample in decoder {
            frame += sample;
            in_frame += 1;
            if in_frame == channels {
                analyzer.push(frame / channels as f32);
                frame = 0.0;
                in_frame = 0;
                samples += 1;
            }
        }
        if samples == 0 {
            return Err("It has no sound in it".into());
        }
        let (peaks, spectrogram) = analyzer.finish();
        Ok(Self {
            bytes,
            channels,
            sample_rate,
            duration: Duration::from_secs_f64(samples as f64 / sample_rate.max(1) as f64),
            peaks,
            spectrogram,
        })
    }

    /// The spectrogram column at `time`.
    pub fn column(&self, time: Duration) -> Option<&[u8; BINS]> {
        let index = (time.as_secs_f64() * self.sample_rate as f64 / SPECTRUM_HOP as f64) as usize;
        self.spectrogram
            .get(index.min(self.spectrogram.len().saturating_sub(1)))
    }

    /// The frequency at the middle of `bin`, in Hz.
    pub fn frequency(&self, bin: f32) -> f32 {
        bin * self.sample_rate as f32 / SPECTRUM_WINDOW as f32
    }
}

/// Mono samples in; peaks and spectrogram columns out.
struct Analyzer {
    peaks: Vec<[f32; 2]>,
    peak: [f32; 2],
    in_peak: usize,
    window: Vec<f32>,
    recent: Vec<f32>,
    since: usize,
    columns: Vec<[u8; BINS]>,
    fft: Arc<dyn rustfft::Fft<f32>>,
    buffer: Vec<Complex<f32>>,
}

impl Analyzer {
    fn new() -> Self {
        // A Hann window.
        let window = (0..SPECTRUM_WINDOW)
            .map(|i| {
                let x = i as f32 / (SPECTRUM_WINDOW - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
            })
            .collect();
        Self {
            peaks: Vec::new(),
            peak: [f32::MAX, f32::MIN],
            in_peak: 0,
            window,
            recent: Vec::with_capacity(SPECTRUM_WINDOW * 2),
            since: 0,
            columns: Vec::new(),
            fft: FftPlanner::new().plan_fft_forward(SPECTRUM_WINDOW),
            buffer: vec![Complex::default(); SPECTRUM_WINDOW],
        }
    }

    fn push(&mut self, sample: f32) {
        self.peak = [self.peak[0].min(sample), self.peak[1].max(sample)];
        self.in_peak += 1;
        if self.in_peak == PEAK_HOP {
            self.peaks.push(self.peak);
            self.peak = [f32::MAX, f32::MIN];
            self.in_peak = 0;
        }
        self.recent.push(sample);
        if self.recent.len() > SPECTRUM_WINDOW {
            // Drop the oldest half at a time, not one by one.
            if self.recent.len() >= SPECTRUM_WINDOW * 2 {
                self.recent.drain(..SPECTRUM_WINDOW);
            }
        }
        self.since += 1;
        if self.since == SPECTRUM_HOP {
            self.since = 0;
            self.column();
        }
    }

    fn column(&mut self) {
        let start = self.recent.len().saturating_sub(SPECTRUM_WINDOW);
        let recent = &self.recent[start..];
        let pad = SPECTRUM_WINDOW - recent.len();
        for (i, slot) in self.buffer.iter_mut().enumerate() {
            let sample = if i < pad { 0.0 } else { recent[i - pad] };
            *slot = Complex::new(sample * self.window[i], 0.0);
        }
        self.fft.process(&mut self.buffer);
        // Half the window's gain is the Hann window's; this makes a full-scale
        // sine about 0 dB.
        let scale = 4.0 / SPECTRUM_WINDOW as f32;
        let per_bin = (SPECTRUM_WINDOW / 2) / BINS;
        let mut column = [0u8; BINS];
        for (bin, value) in column.iter_mut().enumerate() {
            let magnitude = self.buffer[bin * per_bin..(bin + 1) * per_bin]
                .iter()
                .map(|c| c.norm())
                .fold(0.0, f32::max)
                * scale;
            let db = 20.0 * magnitude.max(1e-9).log10();
            *value = ((db - FLOOR_DB) / -FLOOR_DB * 255.0).clamp(0.0, 255.0) as u8;
        }
        self.columns.push(column);
    }

    fn finish(mut self) -> (Vec<[f32; 2]>, Vec<[u8; BINS]>) {
        if self.in_peak > 0 {
            self.peaks.push(self.peak);
        }
        if self.since > 0 || self.columns.is_empty() {
            self.column();
        }
        (self.peaks, self.columns)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A WAV file of a sine at `frequency` Hz, mono 16-bit.
    pub fn sine_wav(frequency: f32, seconds: f32, rate: u32) -> Vec<u8> {
        let count = (seconds * rate as f32) as u32;
        let mut data = Vec::new();
        for i in 0..count {
            let t = i as f32 / rate as f32;
            let sample = (t * frequency * std::f32::consts::TAU).sin() * 0.5;
            data.extend(((sample * i16::MAX as f32) as i16).to_le_bytes());
        }
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        wav
    }

    #[test]
    fn a_sine_has_its_peaks_and_its_frequency() {
        let analysis = Analysis::read(sine_wav(1000.0, 1.0, 44_100).into()).unwrap();
        assert_eq!(analysis.channels, 1);
        assert_eq!(analysis.sample_rate, 44_100);
        assert!((analysis.duration.as_secs_f32() - 1.0).abs() < 0.01);
        assert_eq!(analysis.peaks.len(), 44_100usize.div_ceil(PEAK_HOP));
        let [low, high] = analysis.peaks[100];
        assert!(low < -0.45 && high > 0.45, "{low} {high}");
        // The loudest bin is at 1 kHz.
        let column = analysis.column(Duration::from_millis(500)).unwrap();
        let loudest = (0..BINS).max_by_key(|&bin| column[bin]).unwrap();
        let frequency = analysis.frequency(loudest as f32 * 2.0 + 1.0);
        assert!((frequency - 1000.0).abs() < 50.0, "{frequency}");
        assert!(column[loudest] > 200);
    }

    #[test]
    fn garbage_is_refused() {
        assert!(Analysis::read(Arc::from(b"not audio".as_slice())).is_err());
    }
}
