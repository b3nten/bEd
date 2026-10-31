# bedtime-hex

A hex viewer for workbench apps: any file's bytes as offsets, hex and text.

```rust,ignore
app.add_plugins(HexPlugin);
```

- **Opens anything** (`Fit::Can`), so it's the viewer of last resort: a file
  nothing else shows opens here. The document is the file's bytes (`Bytes`,
  read-only, up to 256 MiB, read on the load worker).
- **Drawing.** Only the rows on screen are drawn; positions are integers, so
  offsets stay exact. Bytes are colored by kind: zero, text, control, high.
- **Keys.** Arrows, Page Up/Down and Home/End move (with Shift, select);
  Cmd/Ctrl+A selects all; Copy copies the selection as hex.
- **Find and go to.** Cmd/Ctrl+F finds bytes (`89 50 4E 47`) or else text, in a
  box floating at the top right (Enter next, Shift+Enter previous, wrapping);
  Cmd/Ctrl+G goes to an offset (`1024`, `0x400`). Where the cursor is, and how
  much is selected, floats at the bottom right.
- **Right-click** copies the selection as hex, text or its offset, selects all,
  finds, goes to.

```sh
cargo test -p bedtime-hex
```
