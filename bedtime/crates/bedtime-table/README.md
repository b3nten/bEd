# bedtime-table

A table editor for workbench apps: CSV and TSV files as a grid.

```rust,ignore
app.add_plugins(TablePlugin);
```

- **The document** is the table (`Table`): rows of text fields, and how its
  file was written (delimiter, sniffed from the first lines; line endings;
  final newline), so it saves the same way. RFC 4180: quoted fields, doubled
  quotes, line breaks in fields, rows of different lengths. Edits
  (`TableChange`: a cell, inserting and deleting rows and columns) undo, redo
  and save as for any document. Clones share rows until changed.
- **The first row** is the header if it looks like one (different, not
  numbers); the menus switch it, and the choice is saved.
- **Viewing.** Only the rows and columns on screen are drawn; the header and
  the row numbers (the file's) stay put. Numbers line up on the right. Column
  widths come from what's in them; drag a header's edge to resize (double-click
  to undo that).
- **Sorting and filtering** change only the view. Click a header to sort
  (again to reverse, a third time to stop): numbers by value, text naturally
  ("a2" before "a10"), empty cells last. Cmd/Ctrl+F (or the floating filter)
  shows rows with a cell containing the text; how many shows at the bottom
  right.
- **Editing.** Click a cell to select it; double-click, Enter, F2 or typing
  edits it; Enter keeps it and moves down, Tab right, Escape cancels. Arrows
  move, Delete clears, Copy and Paste work on the cell. Double-click a header
  (or Rename Column) to rename it.
- **Right-click** a cell: edit, copy, clear, insert rows above and below,
  delete the row, sort, insert and delete columns. A header: rename, sort,
  insert and delete columns.

```sh
cargo test -p bedtime-table
```
