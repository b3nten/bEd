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

## Language servers

Bed bundles 35 language configurations. Language servers provide diagnostics,
hover information and definition/reference navigation. Install the executables
and their required runtimes separately, either locally or on the SSH host where
the project lives. Servers start when matching documents open. Compatible
languages share a process when their server name and resolved project root match.

| Languages | Server executable |
| --- | --- |
| Rust | `rust-analyzer` |
| C, C++ | `clangd` |
| JavaScript, JSX, TypeScript, TSX | `typescript-language-server` |
| Python | `pyright-langserver` |
| Go, go.mod | `gopls` |
| Java | `jdtls` |
| C# | `omnisharp` or `OmniSharp` |
| Lua | `lua-language-server` |
| Bash | `bash-language-server` |
| Markdown | `marksman` |
| TOML | `taplo` |
| JSON, JSONC | `vscode-json-language-server` |
| YAML | `yaml-language-server` |
| HTML | `vscode-html-language-server` |
| CSS, SCSS, Less | `vscode-css-language-server` |
| Ruby | `ruby-lsp` |
| PHP | `intelephense` |
| Swift | `sourcekit-lsp` |
| Elixir | `elixir-ls` |
| Erlang | `erlang_ls` |
| OCaml | `ocamllsp` |
| Clojure | `clojure-lsp` |
| Dart | `dart` |
| CMake | `cmake-language-server` |
| Dockerfile | `docker-langserver` |
| Terraform | `terraform-ls` |
| Nix | `nil` |

The bundled catalog excludes Zig. One server is associated with each language;
completion, inlay hints, formatting, rename and code actions are not implemented
by this foundation.

Diagnostics use server push notifications and document/workspace pulls when the
server supports them. Document pulls refresh diagnostics for unsaved buffers on
open, edit, save and server refresh requests; outdated replies are discarded.
Project-wide diagnostic coverage still depends on workspace pulls or a configured
project check.

### Configuration overrides

Desktop projects load the embedded [catalog](resources/config/lsp.json), then
`~/.config/bed/lsp.json`, then `<project>/.bed/lsp.json`. SSH projects use the local
user file and remote project file. New user files contain `{}` so catalog updates
are inherited. Missing override files are optional. Invalid configuration is
reported; failed reloads preserve running servers.

Languages merge by `name`, servers by their map key. Objects merge recursively;
scalars and arrays replace earlier values. Empty arrays clear inherited lists.
`enabled: false` stops language detection. `language_server: null` retains language
detection without attaching a server. Detection conflicts follow project, user,
then bundled ordering, and declaration order within each layer. Explicit file
globs take priority over extensions, followed by shebangs from document content.
Globs match suffixes of full paths and support brace alternatives such as
`{t,j}sconfig.json`. Path separators are normalized; uppercase C/C++ extensions
retain their distinct associations. `.h` belongs to C++ by default.

For example, a user or project override can configure Rust's existing server,
add a filename association, and detach Python from its default server:

```json
{
  "languages": [
    {
      "name": "rust",
      "file_types": ["rs", {"glob": "*.rust"}],
      "workspace_lsp_roots": ["packages/compiler"]
    },
    {"name": "python", "language_server": null}
  ],
  "language_servers": {
    "rust-analyzer": {
      "command": ["rust-analyzer", "/opt/tools/rust-analyzer"],
      "args": [],
      "environment": {"RUST_LOG": "warn"},
      "settings": {"rust-analyzer": {"check": {"command": "clippy"}}},
      "initialization_options": {"cargo": {"allFeatures": true}},
      "timeout_secs": 30
    }
  }
}
```

Language fields are `name`, `language_id`, `file_types`, `shebangs`, `roots`,
`workspace_lsp_roots`, `language_server` and `enabled`. New languages default to
their name as the protocol ID, empty detection/root lists and `enabled: true`.
Use `javascriptreact`/`typescriptreact` for JSX/TSX protocol IDs. Server `command`
accepts one executable or an ordered list of candidates. Server recipes include
system-location candidates for GUI launches. `args` are literal
arguments, and `environment` affects only the child process; an explicit `PATH`
also controls executable discovery. `settings` is an object delivered after
initialization and used to answer configuration requests. Match settings namespaces to the server’s requested
sections, such as `rust-analyzer`, `python`, `json` or `css`. Request timeouts
accept 1–3600 seconds and default to 20; Java uses 60 seconds.

Roots are found by searching ancestor directories for configured marker names
or globs, stopping at the opened project boundary. The highest matching ancestor
wins; without a marker, the project root is used. `workspace_lsp_roots` contains
relative subdirectories that bound discovery in monorepos. Paths cannot escape
the project. Files outside the project use user configuration and their own
parent directory as the server root.

Configuration files use the schema shown above. Unknown configuration fields are
reported as errors.

Embedded hosts can load an exact configuration with
`SessionOptions.lsp_config_mode = LspConfigMode::Snapshot` (the
default). Snapshot files do not inherit the bundled catalog. Hosts can select
`Layered` to use desktop precedence and project
root discovery. The LSP dashboard exposes server discovery, resolved roots,
startup errors, stderr and restart controls.

### Helix provenance

Launch recipes and selected file associations are adapted from
[Helix's language catalog](https://github.com/helix-editor/helix/blob/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc/languages.toml)
at revision `ba40e547426b0f9896c8bdc699a4ab11f2b37dbc`. The named language/server
separation, layered overrides and bounded root selection follow Helix's approach.
Bed retains its own transport and document ownership; no Helix crates or runtime
catalog downloads are required. Bed uses its own client identity and enables
Python workspace diagnostics in the bundled recipe.
Helix is licensed under MPL 2.0; the upstream license is retained beside the
[catalog](resources/config/LICENSE.helix).

Bed is licensed under GNU GPL version 3 only (`GPL-3.0-only`); see [LICENSE](LICENSE).
Third-party code, fonts and artwork retain their original licenses and notices
in [NOTICE](NOTICE) and the corresponding resource/vendor directories.
The original MIT notice for portions adapted from ned is retained in
[NOTICE](NOTICE), in the ned section.

When distributing binaries, provide the corresponding source and build scripts,
including required dependency sources, using a method permitted by GPLv3 section 6.
