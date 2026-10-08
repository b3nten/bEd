# Terminal regression baselines

The nine `input.sh` and `expected.json` pairs are unchanged files from
ImGui-Terminal commit `1a6733ba9e5cd777acc86cbb111c1c9b4f7500a4` by Neal Mick.
Each `output.bin` captures its script's Bash printf output with the original
Unix PTY's ONLCR conversion (LF becomes CRLF). Core checks feed these bytes in
three-byte chunks. Drawing tests compare the captured drawing operations,
including styles, colors and cursor placement.

`state.json` contains twelve cases and 54 transitions captured from the
original `Terminal` methods. They cover alternate screens and saved attributes,
resize/cursor placement, tabs and pending wrap, wide/combining characters,
erase colors, input modes, selections, scrolling regions, DEC character sets,
X11 color names, palette changes, titles and PTY replies.

Ordinary Rust tests consume these baselines directly, without original C++
sources, Fontconfig headers or a reference checkout:

```sh
cargo test --locked -p bed-terminal --test terminal_core
cargo test --locked -p bed-terminal --test terminal_view
cargo test --locked -p bed-terminal --test terminal_input
```

The terminal drawing/input baselines complement the real PTY, scrollback and
native UI regressions; they do not establish whole-application GPU behavior.
Related keyboard/clipboard cases are retained in `../terminal_input`.

The translated terminal material and fixtures retain Neal Mick's Terminal
Adapter [Business Source License 1.1](../../../LICENSES/terminal-adapter-BSL-1.1.txt),
including its non-commercial additional use grant and April 27, 2030 Change
Date. Source attribution remains in [NOTICE](../../../NOTICE). Keeping these
baselines does not change their license or the adapter's license.
