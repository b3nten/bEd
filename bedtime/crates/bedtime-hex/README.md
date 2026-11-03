# bedtime-hex

Read-only workbench viewer for file offsets, hexadecimal bytes and text.

```rust,ignore
app.add_plugins(HexPlugin);
```

- **Fallback opener:** `Fit::Can` accepts any file. `Bytes` loads up to
  256 MiB on the document worker, read-only.
- **Drawing.** Only visible rows are drawn. Integer positions preserve exact offsets. Bytes are colored by kind: zero, text, control, high.
- **Keys.** Arrows, Page Up/Down and Home/End move (with Shift, select);
  Cmd/Ctrl+A selects all; Copy copies the selection as hex.
- **Find and go to.** Cmd/Ctrl+F finds bytes (`89 50 4E 47`) or else text, in a
  box floating at the top right (Enter next, Shift+Enter previous, wrapping);
  Cmd/Ctrl+G goes to an offset (`1024`, `0x400`). Cursor offset and selection length appear at the bottom right.
- **Right-click** copies the selection as hex, text or its offset, selects all,
  or opens Find or Go to Offset.

```sh
cargo test -p bedtime-hex
```
