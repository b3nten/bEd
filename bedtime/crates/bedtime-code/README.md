# bedtime-code

Code editing for workbench apps, in one crate: the text editor, syntax
(tree-sitter), Structure and language servers.
They call each other directly instead of through contribution APIs. The
grammars are their own crate, `bedtime-grammars` (30 languages, compiled C;
about 25 s on a clean build, cached after).

```rust,ignore
app.add_plugins(CodePlugin);
```

`CodePlugin` analyses the documents editors show, and registers `EditorPanel`
as the default opener for text files
(`text_fit`: no NUL bytes, UTF-8). Opening a file (`open_path`, a file tree)
shows it in an editor; the model's `TextOpenState` (a cursor or selection)
becomes a selection revealed in the middle, once the text has loaded.
Restoring a workspace reopens its editors (workbench's `ViewerFile`).

## The editor

`EditorPanel` is a `Viewer<TextBuffer>`. The text and its undo history are the
panel's workbench document; several panels on one document share them, and
each keeps its own selection, scroll and composition (`EditorView`). Every
edit goes through the document (`edit_document`) at the revision the view saw,
so a stale edit changes nothing. Save, Undo and Redo also work as workbench
actions on the focused panel's document. A document that can't be written is
read-only.

- **Editing.** Grapheme-aware movement and deletion, word movement
  (Alt, or Ctrl off macOS), Home to the first non-blank, Shift selection,
  double click selects a word and triple click a line, clipboard, Tab and
  Shift-Tab indentation, smart newline indentation and closing-bracket
  outdent, wrapping (`EditorSettings::wrap`), line numbers, the animated
  caret.
- **Right-click menu.** A right-click outside the selection moves the caret
  there; the menu offers the language server's actions (Go to Definition,
  Find References, Rename, Code Actions, Format Document) when the file has
  one, then Cut, Copy, Paste and Select All (`EditorView::queue_event`).
- **Find and Replace.** Cmd/Ctrl-F, and Ctrl-H off macOS; literal, case and
  whole-word search over the rope in the background; Replace All is one undo
  step.
- **Composition (IME).** Preedit is a draft on the panel (`DocumentDraft`):
  it blocks saving, undo and edits from other views until committed or
  cancelled.
- **Undo grouping.** Typing groups end on navigation, focus loss, another
  view's edit, or 650 ms idle.
- **Focus.** The text has keyboard focus while its panel has the layout's
  (also when a hidden tab is shown again), and gives it up when another panel
  takes it.
- **Positions** are UTF-8 byte offsets; views map them through the
  document's change journal, and clamp them when the journal has a gap.

Large files: the target is about 100 MiB and a million lines. `TextBuffer` is
a rope; drawing lays out only the visible part of each visible line, and
ordinary editing never copies the whole text. Large indentation changes and
wrapping run on background jobs.

For screen readers and scripts, the editor reports its visible lines as its
text value.

## Comparisons

`editor::diff` shows a comparison inline (the old editor's, brought back):
put an `EditorComparison` (a `Comparison`: the old text, and the changed line
ranges when known; and the `DiffAction` buttons) on an editor panel. Old lines
show between the new, tinted, with old and new line numbers; each change has
its buttons above it. A click comes back as a `ComparisonRequest` on the panel
(the action, the comparison, the revision and the line ranges). While the
text is edited the changes are worked out again on a worker (bounded line
diff). Folding is off while comparing; language server hover and Cmd-click
skip the old lines. `draw_editor` draws an editor panel's editor inside
another panel (mark that panel `HostedViewer`).

## Syntax

`syntax` is the old `bedtime-treesitter`: an `AnalysisSession` parses a
text's revision on a worker (incrementally from the previous tree while every
change since is known) and produces an `Observation`: highlights, folds and an
outline, all at that revision. Sources over 8 MiB aren't analysed; editing is
unaffected.

For documents, `update_syntax` (in `Update`) gives each text document an
editor shows a `DocumentSyntax` on its entity, with a worker it owns
(`ManagedWork`), and feeds it each revision. The language is detected from
the document's title (its file name, `language_name`); a document without one
isn't analysed. When no editor shows the document, the analysis stops.

The editor reads its document's `DocumentSyntax` when drawing:

- **Colours** from the highlights, mapped by the theme (`SyntaxPalette`: the
  terminal colours). While a newer analysis runs, the last one's colours are
  kept, mapped through the edits since (only for the visible lines).
- **Folds**: a fold column (reserved for any text with a language, so text
  doesn't shift when analysis arrives) with a disclosure arrow where each fold
  starts; clicking folds or unfolds, animated. `toggle_fold`, `fold_all` and
  `unfold_all` on `EditorView`. Folds and the outline are only used at their
  own revision, never mapped.

## Structure

`StructurePanel` lists the outline (declarations, from the syntax analysis)
of the focused editor's text, or of the last editor focused while another
kind of panel has focus (`StructureSource`, updated after `update_syntax`).
Entries collapse and expand, animated. While an edit is reanalysed the last
outline stays, so it doesn't flicker as you type; another editor or language
clears it. Clicking an entry selects its name in that editor and focuses the
editor (`reveal_entry`); an entry from an older analysis, or an editor that's
gone or composing, does nothing. Its byte ranges are never mapped through
edits.

## Language servers

`lsp` is the old `bedtime-lsp`: a backend that runs a local stdio server per
launch configuration and root on a worker (process, protocol, sync, progress,
diagnostics, definitions; its bounds and protocol details are in the module
docs), and `documents`, which connects it to workbench documents:

- **Which servers.** `config` has the built-in servers (`KNOWN_SERVERS`,
  Helix's defaults for the languages there's syntax for: rust-analyzer,
  basedpyright, pyright, ruff, typescript-language-server, gopls, clangd, …).
  An app that inserts a `ServerCatalog` (the built-ins changed by its
  settings: `ServerSettings` per server, every field optional, merged layer by
  layer with `config::resolve`; servers can be added) gets `LanguageServers`
  worked out from it: each language's first enabled server whose program is
  found. Programs are looked for on a worker (`InstalledServers`), on the
  login shell's PATH (`environment::shell_environment`, read once: an app
  started from Finder doesn't have the terminal's PATH), and launched by their
  full path in that shell's PATH, HOME and LANG plus their own environment.
  When the configuration changes, servers no longer configured stop and their
  documents reconnect to the new ones. Without a catalog, the app sets
  `LanguageServers` itself.
- A text document gets a server while an editor shows it from a file
  (`ViewerFile`) whose language has one in `LanguageServers`, under the
  workbench's `Root`. Without a root, none run. `server_states` reports each
  running server by name (its status and how many files it has).
  Documents with unsaved changes stay connected after their editor closes.
  Each document's connection is a `DocumentServer` on its entity; each server
  a `LanguageServer` entity, shared by its documents.
- Every revision is sent: its changes when all are known, else the text.
  Successful saves of documents with nothing unsaved are reported to the
  server (rust-analyzer runs `cargo check` then).
- Diagnostics are drawn in the document's editors (underlined, with the
  message on hover) while the text is at their revision.
- **Nothing waits.** A request goes to the server at once, or fails at once
  when the server can't answer now: starting, failed, loading the project
  (rust-analyzer says so), or without the document's latest text (which is
  sent right then if it hadn't been yet). One request, one answer: no queues,
  no retries, no waiting for a server to settle.
- Go to Definition (F12, or Cmd/Ctrl-click a name) opens the first answer
  with `open_path`; a newer request replaces an older one. Positions are
  taken as characters, which is exact for ASCII before the target.
- **Hover**: resting the pointer on a name (200 ms) shows its documentation,
  and the diagnostics there, in a popup (Markdown, its code coloured by a
  syntax analysis on a worker).
- **Completions** open as you type a word or a trigger character (`.`, `::`),
  or with Ctrl-Space. Up/Down choose, Enter or Tab accept, Escape closes. The
  selected entry is resolved for its documentation, shown beside the menu.
  Accepting applies its edits (an import too) as one change; a snippet's tab
  stops are then visited with Tab.
- Errors from what you asked for (F12, Cmd-click, Ctrl-Space) go to
  notifications; what's asked for you while typing just doesn't happen.
- `request_query`, `query_is_current`, `cancel_query` and
  `take_query_answers` are there for other callers; the editor's own
  queries are answered straight to it (`assist`).
- **Code actions** (Cmd/Ctrl+.): the server's fixes and refactorings for
  the selection, in a menu at the caret (Up/Down, Enter, Escape; disabled
  ones say why). Choosing one resolves it if the server sent it bare, applies
  its edit and runs its command; edits the server makes while running it
  (`workspace/applyEdit`) are applied the same way.
- **Rename** (F2): a box at the caret with the name; Enter renames it
  everywhere.
- **Find References** (Shift+F12): the References panel (in the current
  layout, made in its largest area if there isn't one) lists the uses by
  file, with each line's text; clicking one opens it there.
- **Format Document** (Cmd/Ctrl+Shift+I, not Alt-based: Alt with a letter
  types a character while editing).
- **Edits** from all of these apply to an open document as one undoable
  change, left unsaved; to a file that isn't open, on disk, only if it hasn't
  changed since it was read. Creating, renaming and deleting files is
  refused (the client tells servers so). Overlapping edits are refused.
- `request_code_actions`, `offered_code_actions`, `choose_code_action`,
  `rename_symbol`, `find_references_at` and `format_document` do the same
  without the UI.
- Restart Language Server; `restart` and `stop` for apps.

## Tests

```sh
cargo test -p bedtime-code --all-features
python3 tools/bed.py run tests/automation/editor.py tests/automation/syntax.py tests/automation/structure.py tests/automation/language_servers.py tests/automation/code_actions.py
```

The million-line tests and the real rust-analyzer backend tests are ignored by
default; run them with `--ignored`. `tests/lsp_server.py` is the scripted fake
server the other language server tests use. `language_servers.py` and
`code_actions.py` need rust-analyzer installed, and skip otherwise.
