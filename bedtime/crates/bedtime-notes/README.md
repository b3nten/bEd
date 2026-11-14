# bedtime-notes

A Notes panel: personal plain-text notes for a workspace. The notes list runs
down the side (+ for a new note; right-click to rename, duplicate or delete),
and the selected note is edited beside it.

Notes are kept in the workspace's store (`notes.ron`), not as files in the
workspace's folder, so they never show up in the project or its repository.

```rust,ignore
app.add_plugins(NotesPlugin);
```
