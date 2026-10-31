# bedtime-video

A video player for workbench apps, on macOS: AVFoundation plays it (what
QuickTime plays: MP4, MOV, M4V, 3GP), with its sound.

```rust,ignore
app.add_plugins(VideoPlugin);
```

- **The file isn't read into memory:** its model (`VideoFile`) is `UNREAD`, so
  the document only knows its revision; a new revision (the file changed)
  opens it again where it was.
- **Playing.** An `AVPlayer` plays it; an `AVPlayerItemVideoOutput` hands
  each new frame over as BGRA pixels, copied to an egui texture. Players
  belong to their panels (`Players`, a non-send resource: AVFoundation stays
  on the main thread) and go when the panel does.
- **Viewing.** The picture fills its panel, letterboxed. Click or Space plays
  and pauses; Left and Right skip five seconds; Home goes to the start.
- **Floating:** play, a scrubber (dragging pauses, and plays again after),
  time, loop and volume (and mute) at the bottom: shown while paused, faded
  while playing unless pointed at. Size and length at the top left. The
  right-click menu plays, restarts, loops and mutes. Looping, muting and the
  volume are saved.

Elsewhere than macOS there's no opener (files go to the next fit).

```sh
cargo test -p bedtime-video
```
