bed.

Syntax highlighting uses Arborium's bundled collection of 112 language grammars
and queries, including Markdown headings, inline formatting and embedded code.
The Markdown inline grammar comes from tree-sitter-md. Grammars and queries are
compiled into Bed; no separate grammar downloads or query resource files are needed.

Code folding uses the bundled syntax grammars. Click a gutter triangle to fold
or unfold a block, or use the editor context menu's Toggle Fold, Fold All and
Unfold All commands. Cmd/Ctrl+Alt+[ toggles the fold at the caret,
Cmd/Ctrl+Alt+Shift+[ folds all blocks, and Cmd/Ctrl+Alt+] unfolds all blocks.
Folds are independent in each pane and work with word wrapping. Navigation to
hidden text reveals it; copying a selection includes its hidden lines.
Opening and closing animate over 160 ms, including the gutter arrow; toggling
again reverses the motion. Disabling UI animations makes folding immediate.
Folding is disabled in inline diff and conflict views.
Supported languages include Rust, Python, JavaScript/TypeScript, JSON, C/C++, Go,
HTML, CSS, Java, C#, Kotlin, Ruby, Bash, HCL and TOML.

Bed is licensed under GNU GPL version 3 only (`GPL-3.0-only`); see [LICENSE](LICENSE).
Third-party code, fonts and artwork retain their original licenses and notices
in [NOTICE](NOTICE) and the corresponding resource/vendor directories.
The original MIT notice for portions adapted from ned is retained in
[NOTICE](NOTICE), in the ned section.

When distributing binaries, provide the corresponding source and build scripts,
including required dependency sources, using a method permitted by GPLv3 section 6.
