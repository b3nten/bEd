# bedtime-table

Workbench CSV and TSV editor.

```rust,ignore
app.add_plugins(TablePlugin);
```

- **The document** is text (`TextBuffer`), the one a text editor shows, so
  the table and an editor (another panel, from "Open With") follow each
  other's edits. The table (`Table`) is read from it on a worker: rows of
  text fields, the delimiter (sniffed from the first lines), line endings,
  and where each row starts. RFC 4180: quoted fields, doubled quotes, line
  breaks in fields, rows of different lengths. Table edits (`TableChange`: a
  cell, inserting and deleting rows and columns) become text edits touching
  only that field or row (`Table::edit`), so undo, redo and save work as for
  any text, and the file keeps its own quoting elsewhere.
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
