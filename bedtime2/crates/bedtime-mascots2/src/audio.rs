//! One lazily opened device; each view owns its replaceable playback sink.
use crate::{MascotKind, MascotScene};
use bevy::prelude::*;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, buffer::SamplesBuffer};
use std::io::Cursor;

#[derive(Default)]
pub(crate) enum AudioOutput {
    #[default]
    Pending,
    Ready(OutputStream),
    Unavailable,
}

pub(crate) fn play(world: &mut World, entity: Entity, kind: MascotKind) {
    if world.get::<MascotScene>(entity).unwrap().sound.is_none() {
        let sink = {
            let mut output = world.non_send_mut::<AudioOutput>();
            if matches!(*output, AudioOutput::Pending) {
                *output = match OutputStreamBuilder::open_default_stream() {
                    Ok(mut stream) => {
                        stream.log_on_drop(false);
                        AudioOutput::Ready(stream)
                    }
                    Err(error) => {
                        warn!("Mascot audio unavailable: {error}");
                        AudioOutput::Unavailable
                    }
                };
            }
            let AudioOutput::Ready(stream) = &*output else {
                return;
            };
            Sink::connect_new(stream.mixer())
        };
        world.get_mut::<MascotScene>(entity).unwrap().sound = Some(sink);
    }
    let state = world.get::<MascotScene>(entity).unwrap();
    let sink = state.sound.as_ref().unwrap();
    sink.clear();
    match kind {
        MascotKind::Bed => match Decoder::try_from(Cursor::new(
            include_bytes!("../resources/bed.mp3").as_slice(),
        )) {
            Ok(source) => sink.append(source),
            Err(error) => {
                warn!("Invalid bundled Bed click sound: {error}");
                return;
            }
        },
        MascotKind::Duck => sink.append(SamplesBuffer::new(1, 22_050, squeak_samples())),
    }
    sink.play();
}

fn squeak_samples() -> Vec<f32> {
    let rate = 22_050.0;
    let mut phase = 0.0;
    (0..5512)
        .map(|i| {
            let t = i as f32 / rate;
            let envelope = (t * 70.0).min(1.0) * (1.0 - t / 0.25).max(0.0).powi(2);
            let frequency = 950.0 + 1300.0 * (t * 13.0).sin().abs();
            phase += std::f32::consts::TAU * frequency / rate;
            (phase.sin() * 0.75 + (phase * 2.0).sin() * 0.25) * envelope * 0.16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_and_synthesized_click_sounds_have_nonzero_samples() {
        let mut source = Decoder::try_from(Cursor::new(
            include_bytes!("../resources/bed.mp3").as_slice(),
        ))
        .unwrap();
        assert!(source.any(|sample| sample.abs() > 0.001));
        let samples = squeak_samples();
        assert_eq!(samples.len(), 5512);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(samples.iter().any(|s| s.abs() > 0.001));
    }
}
