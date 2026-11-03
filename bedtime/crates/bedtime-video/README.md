# bedtime-video

macOS workbench video player using AVFoundation. Supports MP4, MOV, M4V
and 3GP with audio.

```rust,ignore
app.add_plugins(VideoPlugin);
```

- **Loading:** `VideoFile::UNREAD` avoids loading file bytes into the document.
  A revision change reopens playback at the previous position.
- **Playback:** `AVPlayerItemVideoOutput` supplies BGRA frames from `AVPlayer`
  to a persistent Bevy `Image` per frame size, displayed as an egui user
  texture. Reusing the image avoids upload flicker. `Players` is non-send;
  AVFoundation runs on the main thread. Panel closure releases its player.
- **Viewing.** The picture fills its panel, letterboxed. Click or Space plays
  and pauses; Left and Right skip five seconds; Home goes to the start.
- **Floating:** play, a scrubber (dragging pauses, and plays again after),
  time, loop and volume (and mute) at the bottom: shown while paused, faded
  while playing unless pointed at. Size and length at the top left. The
  right-click menu plays, restarts, loops and mutes. Looping, muting and the
  volume are saved.

No opener is registered on other platforms; files use the next matching viewer.

```sh
cargo test -p bedtime-video
```
