# bedtime-sqlite

An SQLite database viewer and editor for workbench apps. SQLite itself is built in
([rusqlite](https://github.com/rusqlite/rusqlite), `bundled`), so nothing is
needed from the system.

```rust,ignore
app.add_plugins(SqlitePlugin);
```

- **Files.** The `sqlite` format: `.db`, `.sqlite`, `.sqlite3`, `.db3`,
  `.s3db`, `.sl3`, and any file with another extension that starts with
  SQLite's header. A file with one of those names that isn't a database says
  so; "Open With" still offers the hex viewer.
- **The document** (`DatabaseFile`) holds nothing: the file isn't read into
  memory. Each panel keeps its own connection (`Database`, opened for writing
  if the file allows it, never creating it) and asks SQLite on a task, so a
  slow or locked database never stalls drawing. Changes are written to the
  file as they're made, each on its own: there is no Save, Undo or unsaved
  state.
- **Other programs' changes** show within a second: the panel asks SQLite
  (`PRAGMA data_version`) whether another connection changed the database,
  and reads it again if so. The selection stays if it's still there.
- **The panel.** Tables, then views, are listed down the side (SQLite's own
  `sqlite_…` tables are left out). The one selected shows its Data or its
  Structure, or Query; the selection and which of the three are saved with
  the panel.
- **Data** is its rows in a grid (`grid::Grid`), a page of 1,000 at a time
  (`PAGE_ROWS`), with the previous and next page and which rows show
  underneath. SQLite does the work over every row, not the page: click a
  column's name to sort by it (again to reverse, a third time to stop), and
  type an SQL condition (`year > 2000 AND title LIKE 'a%'`) in the filter,
  with Enter, to show only the rows passing it. A filter can only read: a
  statement that would change anything is refused. NULL and BLOBs (shown by
  size) are dimmed, numbers line up on the right. Click a cell to select it;
  arrows move, Copy copies it, the right-click menu copies it or its row.
  Drag a name's edge to resize its column (double-click to undo that).
  Only the cells on screen are drawn. Another program's changes are read
  again without losing the place.
- **Changing rows.** In a table (not a view, and not in a file that can't
  be written, which says Read-only under the rows): double-click a cell, or
  select it and press Enter or F2 or just type, to edit it; Enter keeps it
  and moves down, Tab right, Escape doesn't keep it. Paste puts text in the
  selected cell. What's typed is stored as typed in a column that holds
  text (`007` stays `007`), and as a number elsewhere if it reads as one.
  The right-click menu sets a cell to NULL or deletes its row, which asks
  first under the rows (Delete, Keep). Add Row shows a new row after the
  others, each cell its column's default until given something; Insert Row
  writes it, Discard gives it up. What SQLite refuses (a constraint) is
  said under the rows in its words, nothing changes, and a row being made
  stays to be put right. BLOBs and generated columns can't be edited.
  Rows are found again by their rowid, or by their primary key in a
  WITHOUT ROWID table; if another program removed one first, the change
  says so instead of touching another row.
- **Structure** is its columns with their declared types and constraints
  (primary key, not null, default, generated, references), indexes,
  triggers, and the statement that made it, coloured as SQL.
- **Query** runs SQL typed into it (Run, or Cmd/Ctrl+Enter): one statement
  or several, one after another, each kept as it finishes, as in the
  `sqlite3` shell. One that fails stops the rest and says which it was and
  what SQLite said. The rows of the last statement that gives any show in a
  grid underneath (the first 10,000, `RESULT_ROWS`; clicking a name sorts
  those), with how many statements ran, how many rows they changed and how
  long it took. Cancel interrupts what's running, as does closing the
  panel. After statements that write, the list of tables and the rows in
  Data are read again. The SQL is saved with the panel. An empty database
  opens too, so its first table can be made here.
- **Untrusted files.** `trusted_schema` is off, so a file's views and
  triggers can't call functions with side effects; extensions aren't loaded.
- **`db`** is the database side without Bevy or egui (`Database`, `Schema`,
  `Object`, `Column`, `Index`, `ForeignKey`; `Browse`, `Page`, `Value`;
  `Outcome`, `Interrupter`; `Edit`, `Input`, `RowKey`),
  tested against databases in memory.
- **`grid`** draws rows it's given and keeps only how it looks (widths, the
  selected cell, what's being typed). What it can't do itself (sorting,
  putting a value in a cell, deleting a row) comes back as `grid::Asked`
  for the owner of the rows.

```sh
cargo test -p bedtime-sqlite
python3 tools/bed.py run tests/automation/sqlite.py
```
