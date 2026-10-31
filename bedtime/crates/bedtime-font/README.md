# bedtime-font

A font viewer for workbench apps: TrueType and OpenType (the first face of a
collection).

```rust,ignore
app.add_plugins(FontPlugin);
```

- **Reading** (`FontFile`, on the load worker, with `skrifa`, as egui uses):
  names, style, version, designer, metrics, variation axes and the characters
  it has. The font is given to egui as its own family to draw with.
- **Preview:** the name in the font, a sample you can type into at 48 points,
  the sample at sizes from 12 to 96, and the alphabet and figures.
- **Glyphs:** every character it has, in a grid (only the rows on screen are
  laid out). Clicking one inspects it in a floating card: filled, with its
  outline, its on- and off-curve points, and the font's lines (ascender, cap
  height, x-height, baseline, descender) and its advance. Arrows move; Escape
  closes. Right-click copies the character or its code point.
- **Floating:** Preview and Glyphs at the top right. The mode and sample are
  saved.

WOFF and WOFF2 aren't read.

```sh
cargo test -p bedtime-font
```
