# bedtime-audio

An audio player for workbench apps: MP3, WAV, FLAC, Ogg Vorbis, AAC/M4A, AIFF
and CAF (what Symphonia decodes).

```rust,ignore
app.add_plugins(AudioPlugin);
```

- **Analysis** (`Sound`, on the load worker): the samples are mixed to mono
  and only summaries kept, so long files stay small: the waveform's peaks
  (every 256 samples) and a spectrogram (a 2048-sample Hann window every 1024
  samples, 512 bins of dB from -96 to 0). The file's bytes are kept to play.
- **Playing** is rodio's: a sink per panel on one output device, opened the
  first time something plays (`Speakers`, a non-send resource). It seeks; it
  stops when the panel closes.
- **Views** fill the panel. Waveform: what's played in the accent, click or
  drag to seek. Spectrum: bars spaced as heard (30 Hz up), from where it's
  playing, rising at once and falling smoothly, with caps. Spectrogram: time
  across, frequency up, loudness as color from the theme; click to seek.
- **Keys:** Space plays and pauses, Left and Right skip five seconds, Home goes
  to the start.
- **Floating:** play, time, loop and volume at the bottom (shown while
  paused); the views at the top right; rate, channels and length at the top
  left. The right-click menu has the same. The view, looping and volume are
  saved.

```sh
cargo test -p bedtime-audio
```
