# bedtime-code

Code editing for workbench apps, in one crate: the text editor now; syntax
(tree-sitter), Structure and language servers will be modules here too, so
they call each other directly instead of through contribution APIs.

```rust,ignore
app.add_plugins(CodePlugin);
```

`CodePlugin` registers `EditorPanel` as the default opener for text files
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
- **Find and Replace.** Cmd/Ctrl-F, and Ctrl-H off macOS; literal, case and
  whole-word search over the rope in the background; Replace All is one undo
  step.
- **Composition (IME).** Preedit is a draft on the panel (`DocumentDraft`):
  it blocks saving, undo and edits from other views until committed or
  cancelled.
- **Undo grouping.** Typing groups end on navigation, focus loss, another
  view's edit, or 650 ms idle.
- **Positions** are UTF-8 byte offsets; views map them through the
  document's change journal, and clamp them when the journal has a gap.

Large files: the target is about 100 MiB and a million lines. `TextBuffer` is
a rope; drawing lays out only the visible part of each visible line, and
ordinary editing never copies the whole text. Large indentation changes and
wrapping run on background jobs.

For screen readers and scripts, the editor reports its visible lines as its
text value.

Folding is kept in the view (and drawn), but nothing supplies folds until the
syntax module comes back.

## Tests

```sh
cargo test -p bedtime-code --all-features
python3 tools/bed.py run tests/automation/editor.py
```

The million-line tests are ignored by default; run them with `--ignored`.
