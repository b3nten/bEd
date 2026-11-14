# bedtime-font

Workbench TrueType and OpenType viewer. Collections use the first face.

```rust,ignore
app.add_plugins(FontPlugin);
```

- **Loading:** `FontFile` reads names, style, version, designer, metrics,
  variation axes and supported characters with `skrifa` on the load worker.
  The font is registered as an egui family.
- **Preview:** the font name, an editable 48-point sample,
  the sample at sizes from 12 to 96, and the alphabet and figures.
- **Glyphs:** supported characters in a virtualized grid. Clicking one inspects it in a floating card: filled, with its
  outline, its on- and off-curve points, and the font's lines (ascender, cap
  height, x-height, baseline, descender) and its advance. Arrows move; Escape
  closes. Right-click copies the character or its code point.
- **Floating:** Preview and Glyphs at the top right. The mode and sample are
  saved.

WOFF and WOFF2 are unsupported.

```sh
cargo test -p bedtime-font
```
