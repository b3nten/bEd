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
    assert!(plugin.create_panel(PANEL_ID, None, &Value::Null).is_err());
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
    ctx.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [800.0, 500.0],
        1.0 / 60.0,
    ));
    let ui = ctx.frame();
    ui.window("Audio")
        .size([780.0, 480.0], dear_imgui_rs::Condition::Always)
        .build(|| panel.draw(ui, &context(&documents), &mut Vec::new()));
    let draw = ctx.render_legacy();
    assert!(draw.total_vtx_count() > 100);
    assert!(panel.playback.is_none());
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
