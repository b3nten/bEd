# bedtime-hex

Read-only workbench viewer for file offsets, hexadecimal bytes and text. The
view, its find and go-to bar and menu are `bedtime_ui::hex::HexViewer`; this
crate makes any file a `Bytes` document shown in it.

```rust,ignore
app.add_plugins(HexPlugin);
```

- **Fallback viewer:** registered for `DATA`, so it opens any file, but as the furthest fit. `Bytes` loads up to
  256 MiB on the document worker, read-only.
- **Drawing.** Only visible rows are drawn. Integer positions preserve exact offsets. Bytes are colored by kind: zero, text, control, high.
- **Scrolling.** The wheel or trackpad scrolls by points, parts of a row
  included, and sideways when the panel is narrower than the columns. Each
  scrollbar's thumb drags from where it's pressed; pressing the track jumps
  there. Dragging a selection past the top or bottom scrolls along.
- **Pointer.** The byte under the pointer is lit in both columns; the cursor's
  row offset and column name are lit too. Clicking an offset selects its row.
- **Keys.** Arrows, Page Up/Down (the view moves with the cursor) and Home/End
  move (with Shift, select); Cmd/Ctrl+Home/End go to the start and end;
  Cmd/Ctrl+A selects all; Copy copies the selection as hex.
- **Find and go to.** Cmd/Ctrl+F finds bytes (`89 50 4E 47`) or else text, in a
  box floating at the top right (Enter next, Shift+Enter previous, wrapping);
  Cmd/Ctrl+G goes to an offset (`1024`, `0x400`). Cursor offset and selection length appear at the bottom right.
- **Right-click** copies the selection as hex, text or its offset, selects all,
  or opens Find or Go to Offset.

```sh
cargo test -p bedtime-hex
```
