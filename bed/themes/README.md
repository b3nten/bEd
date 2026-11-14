# bEd's themes

Bundled themes are installed in `~/.config/bed/themes/` when missing at
startup; existing files are preserved. Edit or copy files in this directory.
The Theme Editor (View → Theme Editor, or the sliders button beside Settings →
Theme) edits these files: duplicate a theme under a new name, change its
colors (saved as you go), and reset a shipped theme to how bEd ships it.
Deleted bundled themes return on restart. Settings → Theme → folder opens
the directory. Changes are loaded at runtime. Invalid files produce an error
in Settings and fall back to bedtime-ui's dark theme.

A theme's id is its file name (`tokyo`), which the `appearance.theme` setting
names.

## Format

A theme file is bedtime-ui's `Theme` component in RON. Every color is
required; colors are `"#rrggbb"`, or `"#rrggbbaa"` when not opaque.

```ron
(
    name: "My Theme",
    appearance: Dark,              // or Light
    palette: (
        background: "#272f35",     // around panels: titlebar, gaps
        panel_background: "#2d353b",
        surface: "#343f44",        // controls and popups
        elevated: "#3e474a",       // menus and windows
        border: "#525553",
        text: "#d3c6aa",
        muted: "#918c7e",
        accent: "#a7c080",
        selection: "#543a48",
        success: "#a7c080",
        warning: "#dbbc7f",
        error: "#e67e80",
        terminal: (black: "#475258", red: "#e67e80", green: ..., yellow: ...,
                   blue: ..., magenta: ..., cyan: ..., white: ...,
                   bright_black: ..., bright_red: ..., ... bright_white: ...),
        syntax: (text: ..., comment: ..., keyword: ..., string: ..., number: ...,
                 function: ..., type: ..., variable: ..., parameter: ...,
                 property: ..., constant: ..., operator: ..., punctuation: ...,
                 special: ...),
    ),
)
```

`syntax` colors the editor (and documentation popups) by role; `terminal` is
for terminals, with all sixteen colors named.

Font, size, opacity and effects are settings, not part of a theme.

Converted from bed's earlier JSON themes: roles those didn't have were derived
once (`elevated`, `border` and `muted` blend foreground and backgrounds;
`success`, `warning` and `error` are the terminal's green, yellow and red).

Sources and retained licenses:

| Themes | Source | License |
| --- | --- | --- |
| Tokyo Night | [Tokyo Night](https://github.com/folke/tokyonight.nvim) | tokyo-LICENSE.txt |
| Solarized Light | [Solarized](https://github.com/altercation/solarized) | solarized-LICENSE.txt |
| Carbon | [Carbon Design System Theme](https://github.com/taotao7/carbon-theme/blob/main/themes/taotao7-color-theme.json) | carbon-LICENSE.txt |
| Catppuccin Latte, Frappé, Macchiato, Mocha | [Palette v1.8.0](https://github.com/catppuccin/palette) | catppuccin-LICENSE.txt |
| Rosé Pine, Moon, Dawn | [Rosé Pine](https://github.com/rose-pine/neovim) | rose-pine-LICENSE.txt |
| Synthwave ’84 | [SynthWave ’84](https://github.com/robb0wen/synthwave-vscode/tree/ecfa2fe1279f7233663fa3f98a96e6756000567b) | synthwave-LICENSE.txt |
| Everforest Light/Dark, Hard/Soft (Medium ships in bedtime-ui) | [Everforest](https://github.com/sainnhe/everforest/tree/85a86eb62409e3ec88713bff3d1b9d7374e112e4) | everforest-LICENSE.txt |
| Oxocarbon Light and Dark | [Oxocarbon](https://github.com/nyoom-engineering/oxocarbon.nvim/tree/cd6523a0836d6e8ee823d343149fd06c7b71fdde) | oxocarbon-LICENSE.txt |


Slash (`"slash"`) adapts a contributed palette with a pure-black background,
neutral surfaces, blue keywords/functions/types, and warm tan literals and accents.
Comments and tan colors are slightly brighter so every syntax role remains
readable on the selection background. The terminal retains the contributed ANSI
palette, with the same tan accent and a brighter gray for bright black.

These are adaptations to bEd's UI and fourteen syntax roles, rather than complete
VS Code or Neovim theme interpreters. Terminal palettes provide normal
and bright entries. Everforest and Oxocarbon retain upstream ANSI mappings,
including repeated colors. Synthwave's missing ANSI neutrals use its UI palette;
its translucent selection is flattened against the editor background. Oxocarbon's
gray ramp uses upstream HSLuv lightness interpolation. These adaptations provide
colors only; selecting Synthwave does not change effects or enable glow.
