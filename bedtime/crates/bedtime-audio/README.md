# bedtime-audio

Workbench audio player. Symphonia formats: MP3, WAV, FLAC, Ogg Vorbis,
AAC/M4A, AIFF and CAF.

```rust,ignore
app.add_plugins(AudioPlugin);
```

- **Analysis** (`Sound`, on the load worker): mixes samples to mono and retains
  waveform peaks every 256 samples and a spectrogram (2048-sample Hann
  window, 1024-sample hop, 512 bins, -96 to 0 dB). Original bytes are retained
  for playback.
- **Playback:** rodio, one sink per panel, shared output device opened on
  first playback (`Speakers`, non-send). Supports seeking; panel closure stops playback.
- **Views** fill the panel. Waveform: played samples use the accent color, click or
  drag to seek. Spectrum: perceptually spaced bars from 30 Hz at the playback position,
  with immediate attack, smooth decay and peak caps. Spectrogram: time
  across, frequency up, loudness as color from the theme; click to seek.
- **Keys:** Space plays and pauses, Left and Right skip five seconds, Home goes
  to the start.
- **Floating:** play, time, loop and volume at the bottom (shown while
  paused); the views at the top right; rate, channels and length at the top
  left. The context menu duplicates these controls. The view, looping and volume are
  saved.

```sh
cargo test -p bedtime-audio
```
