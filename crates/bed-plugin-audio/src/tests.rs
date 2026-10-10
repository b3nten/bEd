use super::*;
use bed_plugin::{PluginDocument, Registry};
use rodio::Source;
use std::{collections::HashMap, time::Instant};

fn wav(samples: &[i16], channels: u16, sample_rate: u32) -> Vec<u8> {
    let size = (samples.len() * 2) as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(channels.to_le_bytes());
    bytes.extend(sample_rate.to_le_bytes());
    bytes.extend((sample_rate * channels as u32 * 2).to_le_bytes());
    bytes.extend((channels * 2).to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for sample in samples {
        bytes.extend(sample.to_le_bytes());
    }
    bytes
}

#[test]
fn stereo_decode_preserves_channel_peaks_and_seek_uses_complete_frames() {
    let decoded = audio::decode(
        Cursor::new(wav(&[-32768, 16384, 0, -16384, 32767, 8192, 4096, 0], 2, 4)),
        || false,
    )
    .unwrap();
    assert_eq!(decoded.frames(), 4);
    assert_eq!(decoded.duration(), 1.0);
    assert_eq!(decoded.peaks.len(), 2);
    assert_eq!(decoded.peaks[0][0], (-1.0, 0.0));
    assert_eq!(decoded.peaks[1][0], (0.0, 0.5));
    let decoded = Arc::new(decoded);
    let mut source = PcmSource::new(Arc::clone(&decoded));
    source.try_seek(Duration::from_secs_f64(0.5)).unwrap();
    assert_eq!(source.next(), Some(decoded.samples[4]));
    assert_eq!(source.next(), Some(decoded.samples[5]));
    source.try_seek(Duration::from_secs(20)).unwrap();
    assert_eq!(source.next(), None);
    source.try_seek(Duration::ZERO).unwrap();
    assert_eq!(source.next(), Some(-1.0));
    assert_eq!(source.total_duration(), Some(Duration::from_secs(1)));
}

#[test]
fn malformed_empty_and_cancelled_audio_do_not_become_playable() {
    assert!(audio::decode(Cursor::new(b"invalid audio".to_vec()), || false).is_err());
    assert!(audio::decode(Cursor::new(wav(&[], 1, 8000)), || false).is_err());
    assert!(audio::decode(Cursor::new(wav(&[1; 200], 1, 8000)), || true).is_err());
}

fn tone_audio(amplitude: f32, channels: u16, opposite_phase: bool) -> Audio {
    let samples: Vec<_> = (0..8192)
        .flat_map(|frame| {
            let sample = (std::f32::consts::TAU * 1000.0 * frame as f32 / 8000.0).sin() * amplitude;
            (0..channels).map(move |channel| {
                let sign = if opposite_phase && channel == 1 {
                    -1.0
                } else {
                    1.0
                };
                (sample * sign * 32767.0).round() as i16
            })
        })
        .collect();
    audio::decode(Cursor::new(wav(&samples, channels, 8000)), || false).unwrap()
}

#[test]
fn spectrum_identifies_tones_and_keeps_opposite_phase_channel_power() {
    let mono = tone_audio(0.5, 1, false);
    let stereo = tone_audio(0.5, 2, true);
    let mut mono_computer = analysis::SpectrumComputer::new(&mono);
    let mut stereo_computer = analysis::SpectrumComputer::new(&stereo);
    let mono_levels = mono_computer.compute(&mono, 4096, || false).unwrap();
    let stereo_levels = stereo_computer.compute(&stereo, 4096, || false).unwrap();
    assert_eq!(mono_levels, stereo_levels);
    let (peak_band, &peak) = mono_levels
        .iter()
        .enumerate()
        .max_by_key(|(_, level)| *level)
        .unwrap();
    let frequency = mono_computer.minimum_hz
        * (mono_computer.maximum_hz / mono_computer.minimum_hz)
            .powf((peak_band as f32 + 0.5) / analysis::BANDS as f32);
    assert!(
        (950.0..1050.0).contains(&frequency),
        "peak at {frequency} Hz"
    );
    let db = analysis::FLOOR_DB + peak as f32 / 255.0 * -analysis::FLOOR_DB;
    assert!(
        (db + 6.02).abs() < 0.4,
        "0.5-amplitude tone reads {db} dBFS"
    );
    let spectrogram = analysis::analyze(&stereo, || false).unwrap();
    assert_eq!(spectrogram.spectrum(0.5), mono_levels);
    assert!(spectrogram.frequency_fraction(1000.0) > 0.0);
    assert!(spectrogram.frequency_fraction(1000.0) < 1.0);
}

#[test]
fn silence_short_audio_and_cancelled_analysis_are_bounded() {
    let audio = audio::decode(Cursor::new(wav(&[0; 16], 1, 8000)), || false).unwrap();
    let result = analysis::analyze(&audio, || false).unwrap();
    assert_eq!(result.columns, 1);
    assert_eq!(result.levels, vec![0; analysis::BANDS]);
    assert_eq!(result.rgba.len(), analysis::BANDS * 4);
    assert!(
        result
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|rgba| *rgba == analysis::heat_color(0))
    );
    assert!(
        analysis::analyze(&audio, || true)
            .unwrap_err()
            .contains("cancelled")
    );
}

#[test]
fn long_spectrogram_preserves_a_transient_within_the_column_budget() {
    let frames = (analysis::MAX_COLUMNS + 3) * analysis::HOP;
    let mut samples = vec![0; frames];
    samples[frames / 2 + 211] = 32767;
    let audio = audio::decode(Cursor::new(wav(&samples, 1, 8000)), || false).unwrap();
    let result = analysis::analyze(&audio, || false).unwrap();
    assert_eq!(result.columns, analysis::MAX_COLUMNS);
    assert_eq!(result.levels.len(), analysis::MAX_COLUMNS * analysis::BANDS);
    assert_eq!(
        result.rgba.len(),
        analysis::MAX_COLUMNS * analysis::BANDS * 4
    );
    assert!(result.spectrum(0.5).iter().any(|level| *level > 0));
    assert!(result.spectrum(0.0).iter().all(|level| *level == 0));
    assert!(result.spectrum(1.0).iter().all(|level| *level == 0));
}

#[test]
fn dashboard_sections_restore_and_default_to_expanded() {
    let id = DocumentId::next();
    let panel = AudioPanel::new(id, &Value::Null);
    assert_eq!(panel.sections, [true; 3]);
    let panel = AudioPanel::new(
        id,
        &json!({"waveform":false,"spectrum":true,"spectrogram":false}),
    );
    assert_eq!(panel.sections, [false, true, false]);
    let state = panel.save_state();
    let restored = AudioPanel::new(id, &state);
    assert_eq!(restored.sections, panel.sections);
    assert!(restored.playback.is_none());
    assert!(restored.render_output().is_none());
}

#[test]
fn paused_seeking_refreshes_live_spectrum_and_closing_cancels_workers() {
    let id = DocumentId::next();
    let samples: Vec<_> = (0..8192)
        .map(|frame| {
            let hz = if frame < 4096 { 1000.0 } else { 2000.0 };
            ((std::f32::consts::TAU * hz * frame as f32 / 8000.0).sin() * 16000.0).round() as i16
        })
        .collect();
    let documents = [PluginDocument {
        id,
        path: "two-tones.wav".into(),
        kind: DocumentKind::Bytes,
        language_id: String::new(),
        revision: (0, 0),
        dirty: false,
        bytes: wav(&samples, 1, 8000).into(),
        text: None,
    }];
    let mut panel = AudioPanel::new(id, &json!({"position":0.25}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.spectrum.is_none() {
        panel.sync(&context(&documents));
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let before = panel.spectrum.unwrap();
    panel.seek(0.75);
    while panel.spectrum == Some(before) {
        panel.sync(&context(&documents));
        assert!(
            Instant::now() < deadline,
            "paused spectrum did not update after seek"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert!(panel.playback.is_none());
    assert_eq!(panel.position, 0.75);
    panel.close(&mut Vec::new());
    assert!(panel.worker.sender.is_none());
    assert!(panel.worker.spectrum_sender.is_none());
    assert!(panel.render_output().is_none());
}

#[test]
#[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
fn native_spectrogram_renders_uploaded_colors_and_rebuilds_for_a_new_generation() {
    use bed_plugin::gpu::{renderer_device_descriptor, wgpu};
    use std::{future::Future, task::Wake};

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        struct ThreadWake(thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(Arc::new(ThreadWake(thread::current())));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => thread::park(),
            }
        }
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) =
        block_on(adapter.request_device(&renderer_device_descriptor(&adapter))).unwrap();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut panel = AudioPanel::new(DocumentId::next(), &Value::Null);
    panel.analysis = Some(analysis::analyze(&tone_audio(0.5, 2, true), || false).unwrap());
    panel.spectrogram_visible = true;
    let target = RenderTarget::new(&device, panel.render_output().unwrap()).unwrap();
    let stride = (target.size[0] * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Spectrogram readback"),
        size: u64::from(stride) * u64::from(target.size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for generation in [1, 2] {
        let mut encoder = device.create_command_encoder(&Default::default());
        panel
            .render(
                &mut GpuContext {
                    instance: &instance,
                    adapter: &adapter,
                    device: &device,
                    queue: &queue,
                    encoder: &mut encoder,
                    error_handlers: None,
                    generation,
                },
                &target,
            )
            .unwrap();
        assert_eq!(panel.gpu.as_ref().unwrap().generation, generation);
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(target.size[1]),
                },
            },
            target.texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let mapped = buffer.slice(..).get_mapped_range();
        let expected = &panel.analysis.as_ref().unwrap().rgba;
        for row in 0..target.size[1] as usize {
            let actual =
                &mapped[row * stride as usize..row * stride as usize + target.size[0] as usize * 4];
            let expected = &expected
                [row * target.size[0] as usize * 4..(row + 1) * target.size[0] as usize * 4];
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(*expected) <= 1)
            );
        }
        drop(mapped);
        buffer.unmap();
    }
    assert!(block_on(validation.pop()).is_none());
}

#[test]
fn downsampled_waveform_keeps_short_transients_in_their_channels() {
    let mut samples = vec![0_i16; 20000];
    samples[17000] = -32768;
    samples[17001] = 32767;
    let decoded = audio::decode(Cursor::new(wav(&samples, 2, 10000)), || false).unwrap();
    assert_eq!(decoded.peaks[0].len(), 4096);
    assert!(decoded.peaks[0].iter().any(|p| p.0 == -1.0));
    assert!(decoded.peaks[0].iter().all(|p| p.1 == 0.0));
    assert!(decoded.peaks[1].iter().any(|p| p.1 > 0.99));
    assert!(decoded.peaks[1].iter().all(|p| p.0 == 0.0));
}

fn context(documents: &[PluginDocument]) -> HostContext<'_> {
    static SETTINGS: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));
    static TEXTURES: std::sync::LazyLock<
        HashMap<bed_plugin::TextureHandle, dear_imgui_rs::TextureId>,
    > = std::sync::LazyLock::new(HashMap::new);
    HostContext {
        remote: false,
        default_viewers: &Value::Null,
        viewer_menu: None,
        documents,
        active_document: documents.first().map(|d| d.id),
        settings: &SETTINGS,
        textures: &TEXTURES,
        animations: false,
        workspace: 1,
        diagnostics: &SETTINGS,
    }
}
#[test]
fn revisions_discard_old_decode_and_restored_state_never_autoplays() {
    let id = DocumentId::next();
    let mut panel = AudioPanel::new(id, &json!({"position":0.5,"volume":0.25}));
    let mut documents = vec![PluginDocument {
        id,
        path: "tone.WAV".into(),
        kind: DocumentKind::Bytes,
        language_id: String::new(),
        revision: (0, 0),
        dirty: false,
        bytes: wav(&[1000; 8000], 1, 8000).into(),
        text: None,
    }];
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.audio.is_none() {
        panel.sync(&context(&documents));
        assert!(
            Instant::now() < deadline,
            "audio worker did not finish: {:?}",
            panel.error
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(panel.position, 0.5);
    assert_eq!(panel.volume, 0.25);
    assert!(!panel.playing());
    documents[0].revision = (1, 0);
    documents[0].bytes = b"corrupt edit".to_vec().into();
    panel.sync(&context(&documents));
    assert!(panel.audio.is_none());
    assert_eq!(panel.position, 0.0);
    documents[0].revision = (2, 0);
    documents[0].bytes = wav(&[0; 4000], 1, 8000).into();
    while panel.audio.is_none() {
        panel.sync(&context(&documents));
        assert!(
            Instant::now() < deadline,
            "replacement decode did not finish"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(panel.audio.as_ref().unwrap().duration(), 0.5);
    assert!(panel.error.is_none());
    assert!(panel.playback.is_none());
}

#[test]
fn registers_audio_extensions_and_rejects_unknown_panels() {
    let mut registry = Registry::default();
    let mut plugin = AudioPlugin;
    registry.register(&plugin).unwrap();
    for path in [
        "tone.WAV",
        "music.mp3",
        "track.FLAC",
        "voice.ogg",
        "sample.aiff",
        "music.m4a",
    ] {
        assert_eq!(registry.viewer_for_path(path).unwrap().id, VIEWER_ID);
    }
    assert!(registry.viewer_for_path("code.rs").is_none());
    assert!(plugin.create_panel(PANEL_ID, None, &Value::Null).is_ok());
    assert!(
        plugin
            .create_panel("bad.panel", Some(DocumentId::next()), &Value::Null)
            .is_err()
    );
}

#[test]
fn audio_panel_draws_a_decoded_waveform_without_an_output_device() {
    let mut ctx = dear_imgui_rs::Context::create();
    ctx.set_ini_filename(None::<std::path::PathBuf>).unwrap();
    ctx.font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let id = DocumentId::next();
    let documents = vec![PluginDocument {
        id,
        path: "tone.wav".into(),
        kind: DocumentKind::Bytes,
        language_id: String::new(),
        revision: (0, 0),
        dirty: false,
        bytes: wav(&[8000; 8000], 1, 8000).into(),
        text: None,
    }];
    let mut panel = AudioPanel::new(id, &Value::Null);
    panel.revision = Some((0, 0));
    panel.submitted = Some((0, 0));
    panel.audio = Some(Arc::new(
        audio::decode(Cursor::new(Arc::clone(&documents[0].bytes)), || false).unwrap(),
    ));
    panel.analysis = Some(analysis::analyze(panel.audio.as_ref().unwrap(), || false).unwrap());
    ctx.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 900.0],
        1.0 / 60.0,
    ));
    let ui = ctx.frame();
    ui.window("Audio")
        .size([780.0, 880.0], dear_imgui_rs::Condition::Always)
        .build(|| panel.draw(ui, &context(&documents), &mut Vec::new()));
    let draw = ctx.render_legacy();
    assert!(draw.total_vtx_count() > 100);
    drop(draw);
    assert!(panel.playback.is_none());
    let output = panel
        .render_output()
        .expect("expanded spectrogram requests a GPU target");
    panel.position = 0.5;
    for collapsed in [false, true] {
        panel.sections[2] = !collapsed;
        ctx.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [800.0, 900.0],
            1.0 / 60.0,
        ));
        let ui = ctx.frame();
        ui.window("Audio")
            .size([780.0, 880.0], dear_imgui_rs::Condition::Always)
            .build(|| panel.draw(ui, &context(&documents), &mut Vec::new()));
        drop(ctx.render_legacy());
        if collapsed {
            assert!(
                panel.render_output().is_none(),
                "collapsed analysis must release its render request"
            );
        } else {
            assert_eq!(
                panel.render_output(),
                Some(output),
                "moving the playhead must not re-upload the heatmap"
            );
        }
    }
    // A previously registered output can scroll out of view. Its draw commands
    // must stop using the ID when the panel releases the render request.
    let texture = dear_imgui_rs::TextureId::from(122usize);
    let textures = HashMap::from([(panel.texture, texture)]);
    let mut host = context(&documents);
    host.textures = &textures;
    panel.sections[2] = true;
    ctx.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 300.0],
        1.0 / 60.0,
    ));
    let ui = ctx.frame();
    ui.window("Audio")
        .position([0.0; 2], dear_imgui_rs::Condition::Always)
        .size([780.0, 280.0], dear_imgui_rs::Condition::Always)
        .build(|| panel.draw(ui, &host, &mut Vec::new()));
    let draw = ctx.render_legacy();
    assert!(panel.render_output().is_none());
    assert!(draw.draw_lists().all(|list| list.commands().all(|command| {
        !matches!(command, dear_imgui_rs::DrawCmd::Elements { cmd_params, .. }
            if cmd_params.texture_id == texture)
    })));
}

#[test]
#[ignore = "requires a native audio output device; plays silence"]
fn native_audio_transport_plays_pauses_seeks_restarts_and_closes() {
    let mut panel = AudioPanel::new(DocumentId::next(), &Value::Null);
    panel.audio = Some(Arc::new(
        audio::decode(Cursor::new(wav(&vec![0; 16000], 1, 8000)), || false).unwrap(),
    ));
    panel.toggle();
    assert!(
        panel.playing(),
        "native playback could not start: {:?}",
        panel.error
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while panel.current_position() < 0.03 {
        assert!(
            Instant::now() < deadline,
            "native audio clock did not advance"
        );
        thread::sleep(Duration::from_millis(5));
    }
    panel.toggle();
    assert!(!panel.playing());
    thread::sleep(Duration::from_millis(30));
    let paused = panel.current_position();
    thread::sleep(Duration::from_millis(50));
    assert!((panel.current_position() - paused).abs() < 0.02);
    panel.seek(0.75);
    assert!((panel.current_position() - 0.75).abs() < 0.03);
    assert!(!panel.playing(), "seeking while paused must preserve pause");
    assert!((panel.save_state()["position"].as_f64().unwrap() - 0.75).abs() < 0.03);
    panel.toggle();
    assert!(panel.playing());
    panel.seek(0.0);
    assert!(panel.playing(), "restart must preserve playback");
    panel.seek(2.0);
    while panel.playing() {
        assert!(Instant::now() < deadline, "audio did not finish at EOF");
        thread::sleep(Duration::from_millis(5));
    }
    panel.position = panel.current_position();
    panel.toggle();
    assert!(panel.playing(), "Play at EOF must restart");
    assert!(panel.current_position() < 0.2);
    panel.close(&mut Vec::new());
    assert!(panel.playback.is_none());
}
