//! Bounded, whole-file STFT analysis. Channels are combined as power rather than
//! samples, so an out-of-phase stereo signal remains visible.
use crate::audio::Audio;
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::{f32::consts::TAU, sync::Arc};

pub(super) const WINDOW: usize = 2048;
pub(super) const HOP: usize = 512;
pub(super) const BANDS: usize = 256;
pub(super) const MAX_COLUMNS: usize = 4096;
pub(super) const FLOOR_DB: f32 = -90.0;

#[derive(Debug)]
pub(super) struct Analysis {
    pub columns: usize,
    /// Time-major, ascending logarithmic frequency bands, -90 to 0 dBFS.
    pub levels: Vec<u8>,
    /// Image rows run from high to low frequencies.
    pub rgba: Vec<u8>,
    pub minimum_hz: f32,
    pub maximum_hz: f32,
}
impl Analysis {
    pub fn spectrum(&self, fraction: f64) -> &[u8] {
        let column =
            ((fraction.clamp(0.0, 1.0) * self.columns as f64) as usize).min(self.columns - 1);
        &self.levels[column * BANDS..(column + 1) * BANDS]
    }
    pub fn frequency_fraction(&self, hz: f32) -> f32 {
        (hz / self.minimum_hz).ln() / (self.maximum_hz / self.minimum_hz).ln()
    }
}

pub(super) struct SpectrumComputer {
    pub minimum_hz: f32,
    pub maximum_hz: f32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    ranges: Vec<(usize, usize)>,
    normalization: f32,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    power: Vec<f32>,
}
impl SpectrumComputer {
    pub fn new(audio: &Audio) -> Self {
        let maximum_hz = audio.sample_rate as f32 * 0.5;
        // Keep a logarithmic range for unusual low-rate source files too.
        let minimum_hz = 20.0_f32.min(maximum_hz * 0.5);
        let ratio = maximum_hz / minimum_hz;
        let frequency_step = audio.sample_rate as f32 / WINDOW as f32;
        let ranges = (0..BANDS)
            .map(|band| {
                let lo = minimum_hz * ratio.powf(band as f32 / BANDS as f32);
                let hi = minimum_hz * ratio.powf((band + 1) as f32 / BANDS as f32);
                let start = (lo / frequency_step)
                    .floor()
                    .clamp(1.0, (WINDOW / 2) as f32) as usize;
                let end = ((hi / frequency_step).ceil() as usize)
                    .max(start + 1)
                    .min(WINDOW / 2 + 1);
                (start, end)
            })
            .collect();
        let window: Vec<_> = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (TAU * i as f32 / WINDOW as f32).cos())
            .collect();
        let gain = window.iter().sum::<f32>();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        Self {
            minimum_hz,
            maximum_hz,
            scratch: vec![Complex::default(); fft.get_inplace_scratch_len()],
            fft,
            window,
            ranges,
            normalization: 4.0 / (gain * gain * audio.channels as f32),
            buffer: vec![Complex::default(); WINDOW],
            power: vec![0.0; WINDOW / 2 + 1],
        }
    }
    pub fn compute(
        &mut self,
        audio: &Audio,
        center: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<[u8; BANDS], String> {
        self.power.fill(0.0);
        for channel in 0..audio.channels as usize {
            if cancelled() {
                return Err("Audio analysis cancelled".into());
            }
            for (i, value) in self.buffer.iter_mut().enumerate() {
                let frame = (center + i).checked_sub(WINDOW / 2);
                let sample = frame
                    .filter(|&frame| frame < audio.frames())
                    .map_or(0.0, |frame| {
                        audio.samples[frame * audio.channels as usize + channel]
                    });
                *value = Complex::new(sample * self.window[i], 0.0);
            }
            self.fft
                .process_with_scratch(&mut self.buffer, &mut self.scratch);
            for (bin, sum) in self.power.iter_mut().enumerate() {
                // Nyquist is not paired with a negative-frequency bin.
                let scale = if bin == WINDOW / 2 { 0.25 } else { 1.0 };
                *sum += self.buffer[bin].norm_sqr() * self.normalization * scale;
            }
        }
        let mut levels = [0; BANDS];
        for (band, &(start, end)) in self.ranges.iter().enumerate() {
            let peak = self.power[start..end]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);
            let db = (10.0 * peak.max(1e-9).log10()).clamp(FLOOR_DB, 0.0);
            levels[band] = ((db - FLOOR_DB) / -FLOOR_DB * 255.0).round() as u8;
        }
        Ok(levels)
    }
}

pub(super) fn analyze(audio: &Audio, cancelled: impl Fn() -> bool) -> Result<Analysis, String> {
    let steps = audio.frames().div_ceil(HOP);
    let columns = steps.min(MAX_COLUMNS);
    let mut computer = SpectrumComputer::new(audio);
    let mut levels = vec![0; columns * BANDS];
    for step in 0..steps {
        let spectrum = computer.compute(audio, step * HOP, &cancelled)?;
        let column = step * columns / steps;
        for (band, level) in spectrum.into_iter().enumerate() {
            let cell = &mut levels[column * BANDS + band];
            // Peak reduction keeps brief transients when long files exceed the
            // time-column budget. Every 512-sample hop is still analyzed.
            *cell = (*cell).max(level);
        }
    }
    let mut rgba = vec![0; columns * BANDS * 4];
    for column in 0..columns {
        if cancelled() {
            return Err("Audio analysis cancelled".into());
        }
        for band in 0..BANDS {
            let offset = ((BANDS - 1 - band) * columns + column) * 4;
            rgba[offset..offset + 4].copy_from_slice(&heat_color(levels[column * BANDS + band]));
        }
    }
    Ok(Analysis {
        columns,
        levels,
        rgba,
        minimum_hz: computer.minimum_hz,
        maximum_hz: computer.maximum_hz,
    })
}

/// Dark indigo → purple → coral → warm yellow; silence remains distinguishable.
pub(super) fn heat_color(level: u8) -> [u8; 4] {
    const STOPS: [[f32; 3]; 5] = [
        [10.0, 12.0, 28.0],
        [47.0, 25.0, 112.0],
        [140.0, 40.0, 130.0],
        [237.0, 100.0, 69.0],
        [255.0, 238.0, 160.0],
    ];
    let position = level as f32 / 255.0 * 4.0;
    let start = (position as usize).min(3);
    let blend = position - start as f32;
    let mut rgba = [0, 0, 0, 255];
    for channel in 0..3 {
        rgba[channel] = (STOPS[start][channel] * (1.0 - blend) + STOPS[start + 1][channel] * blend)
            .round() as u8;
    }
    rgba
}
