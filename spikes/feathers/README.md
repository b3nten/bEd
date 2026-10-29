# Spike: feathers + bevy_immediate

What bed's shell and Settings panel look like on bevy_ui, with
`bevy_feathers` widgets driven through `bevy_immediate` (0.9, already on Bevy
0.20, from crates.io). Run it with `cargo run -p feathers-spike`.

- `bedtime-workbench` is the real one, built with `default-features = false`:
  layouts, areas and tabs, with no egui.
- `shell.rs` (~230 lines) draws areas (gaps, rounding), tab bars (hover,
  close) and a status bar from that model every frame. Tab clicks call
  `focus_panel`/`close_panel`.
- Each panel's content is its own immediate tree, attached to a host node the
  shell puts in the area (`panels::host`).
- `look.rs` maps a bed `Theme` onto feathers' `UiTheme` tokens, and registers
  Bedfont under feathers' embedded Fira Sans paths.

Screenshots: `screenshots/reference-egui.png` (bed today),
`spike-everforest.png`, and `spike-switched.png` (switched to Everforest
Light while running).

## Findings

- **The aesthetic carries over.** Areas, gaps, rounded surfaces, tabs with
  muted/emphasised titles, Bedfont and every bed palette work. Feathers'
  sliders (a filled bar with the value inside) and its square-knob toggles look
  different from bed's, but they fit.
- **Immediate state really is gone.** The Settings panel binds widgets straight
  to fields (`.slider(&mut look.gap)`, `.checked(&mut look.blur)`), and the
  shell re-reads the workbench model each frame. There are no messages, no
  per-widget observers, and no "applied" diffing.
- **But it is not egui.** `construct` gets declared `SystemParam`s, not
  `&mut World`. Writes go through commands, so a click is seen the next frame.
  Only components declared by the capability set can be changed in place
  (`node_mut`, colors, text). Anything else is `on_change_insert`, or a new
  capability.
- **Feathers has rough edges** that we would have to own:
  - Inherited text colors aren't refreshed when `UiTheme` changes (fixed here
    by `refresh_inherited_text`).
  - Font sizes are hard-coded `Px` constants, so widget text doesn't follow
    the Font size setting.
  - Fonts are fixed asset paths (worked around by replacing the embedded
    bytes).
  - Its docs call it editor-only and suggest copying it.
- **Not tried yet:**
  - Tab and corner dragging, border resizing, drop overlays.
  - Text editing, the editor and terminal panels (custom painting; egui's
    painter has no direct bevy_ui equivalent).
  - Animations, tooltips, accessibility/automation (`tools/bed.py` reads
    egui's AccessKit tree).
  - Driving the spike with real mouse input: only screenshots were checked.
