# Bedside Table

A small independent SQLite desktop application built with Bedtime. It installs
the standard foundation and its own SQL panels. It has no dependency on bEd,
the code editor, workspaces, File Explorer, Terminal, or language services.

```sh
cargo run --locked -p bedside-table -- /path/to/database.sqlite
```

Open several databases from File → Open Database. Click a table or view for its
data; its context menu opens the schema in a separate tab. Data is paged. The
connection menu can refresh, disconnect, or reconnect without closing queries.

New Query opens a custom SQL widget backed by a shared typed document. Run with
Primary+Enter; undo/redo and Save use the foundation's document commands.
Choosing a saved query focuses its existing tab when it is already open. Query
execution accepts a single read-only statement, with bounded results and a
deadline. Cancelling a query affects that tab's current request only.

Save stores SQL in `~/.config/bedside-table/metadata.sqlite`; it never modifies
the inspected database. This app-owned database also stores connection records
and the workbench layout through its `StateStore` adapter. `--config-dir PATH`
selects another config directory. Settings use the foundation's settings file.

Dirty queries prompt for Save, Discard, or Cancel on close/quit. A failed save
keeps the document live. Restart restores saved SQL and panel layout without
running restored SQL automatically. Unsaved crash recovery and undo history
across restart are outside this first version.

```sh
cargo test --locked -p bedside-table
cargo build --locked -p bedside-table
python3 bedside-table/scripts/smoke.py
```

The Python automation scenario uses two seeded temporary databases, enters SQL in the actual
query widget, verifies shared undo/redo and Save, captures the GPU frame, then
starts a fresh process to verify restoration. The headless tests additionally
cover independent cancellation, write rejection, and dirty-query close choices.

The scenario uses the shared local socket and `bedtime.py`, with a launch-only
approval for the client named `smoke test`. `bedside.state` publishes structured
connection, query and result observations. The normal application has no native
smoke flags or scenario state machine.
