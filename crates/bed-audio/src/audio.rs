use rodio::{Decoder, Source, source::SeekError};
use std::{
    io::{Read, Seek},
    sync::Arc,
    time::Duration,
};

const MAX_SAMPLES: usize = 256 * 1024 * 1024 / size_of::<f32>();
const WAVEFORM_BINS: usize = 4096;
pub(super) struct Audio {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
    pub peaks: Vec<Vec<(f32, f32)>>,
}
impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }
    fn new(samples: Vec<f32>, channels: u16, sample_rate: u32) -> Self {
        let frames = samples.len() / channels as usize;
        let bins = frames.min(WAVEFORM_BINS);
        let mut peaks = vec![vec![(0.0_f32, 0.0_f32); bins]; channels as usize];
        for (frame, samples) in samples.chunks_exact(channels as usize).enumerate() {
            let bin = frame * bins / frames;
            for (channel, sample) in samples.iter().enumerate() {
                peaks[channel][bin].0 = peaks[channel][bin].0.min(*sample);
                peaks[channel][bin].1 = peaks[channel][bin].1.max(*sample);
            }
        }
        Self {
            samples,
            channels,
            sample_rate,
            peaks,
        }
    }
}
pub(super) fn decode<R: Read + Seek + Send + Sync + 'static>(
    reader: R,
    cancelled: impl Fn() -> bool,
) -> Result<Audio, String> {
    let decoder = Decoder::new(reader).map_err(|e| format!("Unable to decode audio: {e}"))?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    if channels == 0 || channels > 32 || sample_rate == 0 {
        return Err("Unsupported audio channel count or sample rate".into());
    }
    let mut samples = Vec::new();
    for sample in decoder {
        if samples.len() % 4096 == 0 && cancelled() {
            return Err("Audio decoding cancelled".into());
        }
        if samples.len() == MAX_SAMPLES {
            return Err("Audio exceeds the 256 MiB decoded sample limit".into());
        }
        samples.push(if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        });
    }
    samples.truncate(samples.len() / channels as usize * channels as usize);
    if samples.is_empty() {
        return Err("The audio file contains no decodable samples".into());
    }
    if cancelled() {
        return Err("Audio decoding cancelled".into());
    }
    Ok(Audio::new(samples, channels, sample_rate))
}
/// Shares decoded PCM across playback restarts and implements exact frame-aligned seeking.
pub(super) struct PcmSource {
    audio: Arc<Audio>,
    offset: usize,
}
impl PcmSource {
    pub fn new(audio: Arc<Audio>) -> Self {
        Self { audio, offset: 0 }
    }
}
impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let value = self.audio.samples.get(self.offset).copied()?;
        self.offset += 1;
        Some(value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.audio.samples.len() - self.offset;
        (remaining, Some(remaining))
    }
}
impl Source for PcmSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.audio.channels
    }
    fn sample_rate(&self) -> u32 {
        self.audio.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(self.audio.duration()))
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.offset = ((position.as_secs_f64() * self.audio.sample_rate as f64) as usize)
            .min(self.audio.frames())
            * self.audio.channels as usize;
        Ok(())
    }
}
pub(super) fn time_label(seconds: f64) -> String {
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
