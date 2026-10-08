# Color themes

Built-in themes are read-only runtime resources. To customize one, copy its JSON
into `~/.config/bed/themes/my-theme.json`, edit it, and select it in Appearance.
Alternatively, open Settings → Theme Editor and choose a saved custom theme or
clone an existing theme. The editor then lets you name, edit, and save it.
UI, syntax, and all sixteen terminal colors support pickers and hex inputs.
Paste Theme JSON imports a complete Bed-format theme into a new draft. Invalid
imports leave the existing draft intact. Save & Apply selects the saved theme;
Save Theme just saves it. New copies get unique filenames. Editing an existing
custom theme keeps its filename even when its display name changes. Each settings
panel retains its own unsaved draft while switching categories. Back to Themes
returns to the choices; Continue editing resumes the draft without losing changes.
The selected file reloads automatically. An invalid edit keeps the previous colors
and displays an error until the file is repaired. No app restart is needed.

A theme contains `name`, `appearance` (`light` or `dark`), `ui`, `syntax`, and
`terminal`. Every color is an opaque `#RRGGBB` string. Copy an existing file as a
complete template; all palette roles are required.

- `ui`: `background`, `surface`, `foreground`, `accent`, and `selection`.
  `surface` colors controls and popups.
  Optional `tab_bar` sets a separate tab/title bar color. When omitted, it uses
  `background`, as all bundled themes do.
- `syntax`: `text`, `comment`, `keyword`, `string`, `number`, `function`, `type`,
  `variable`, `parameter`, `property`, `constant`, `operator`, `punctuation`, and `special`.
- `terminal`: sixteen ANSI colors in this order: black, red, green, yellow, blue,
  magenta, cyan, white, followed by the eight bright counterparts.

Font, size, background opacity, animations, effects, and editor preferences belong
in `settings.json`. Additional theme fields never override these preferences.

The `theme` preference selects a built-in ID (for example `"catppuccin-mocha"`)
or a custom relative path (for example `"themes/my-theme.json"`). Built-ins and
custom files have distinct identifiers, so a custom file never shadows a built-in.

Sources and retained licenses:

| Themes | Source | License |
| --- | --- | --- |
| Tokyo Night | [Tokyo Night](https://github.com/folke/tokyonight.nvim) | tokyo-LICENSE.txt |
| Solarized Light | [Solarized](https://github.com/altercation/solarized) | solarized-LICENSE.txt |
| Carbon | [Carbon Design System Theme](https://github.com/taotao7/carbon-theme/blob/main/themes/taotao7-color-theme.json) | carbon-LICENSE.txt |
| Catppuccin Latte, Frappé, Macchiato, Mocha | [Palette v1.8.0](https://github.com/catppuccin/palette) | catppuccin-LICENSE.txt |
| Rosé Pine, Moon, Dawn | [Rosé Pine](https://github.com/rose-pine/neovim) | rose-pine-LICENSE.txt |
| Synthwave ’84 | [SynthWave ’84](https://github.com/robb0wen/synthwave-vscode/tree/ecfa2fe1279f7233663fa3f98a96e6756000567b) | synthwave-LICENSE.txt |
| Everforest Light/Dark, Hard/Medium/Soft | [Everforest](https://github.com/sainnhe/everforest/tree/85a86eb62409e3ec88713bff3d1b9d7374e112e4) | everforest-LICENSE.txt |
| Oxocarbon Light and Dark | [Oxocarbon](https://github.com/nyoom-engineering/oxocarbon.nvim/tree/cd6523a0836d6e8ee823d343149fd06c7b71fdde) | oxocarbon-LICENSE.txt |

These are adaptations to bEd's UI and fourteen syntax roles, rather than complete
VS Code or Neovim theme interpreters. Terminal palettes provide normal
and bright entries. Everforest and Oxocarbon retain upstream ANSI mappings,
including repeated colors. Synthwave's missing ANSI neutrals use its UI palette;
its translucent selection is flattened against the editor background. Oxocarbon's
gray ramp uses upstream HSLuv lightness interpolation. These adaptations provide
colors only; selecting Synthwave does not change effects or enable glow.
Colors are loaded locally; no network is used at runtime.
