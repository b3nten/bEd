# bedtime-datatree

Workbench tree viewer for JSON and TOML. Parsing and the rows are
`bedtime_ui::data` (`parse`, `DataTree`); this crate shows a text document
with them, re-parsing each revision on a worker, and saves which rows are
open with the layout.

```rust,ignore
app.add_plugins(DataTreePlugin);
```

- **The document** is text (`TextBuffer`), the one a text editor shows, so
  the tree follows the editor's edits, unsaved too. Each revision parses on
  a worker. While the text doesn't parse, the last tree that did stays,
  with the error.
- **Not the default.** JSON and TOML files open in the editor; this viewer
  is in "Open With", and it's an editor's `other_viewer` (bed's Open to the
  Side shows it beside the editor).
- **Rows** open and close. Arrays and tables show their size, scalars their
  value coloured by kind. Top-level containers start open, deeper ones
  closed. The open rows are saved with the panel.

```sh
cargo test -p bedtime-datatree
```
