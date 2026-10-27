# bedtime-document

Document models, sources and transaction history, without Bevy, a filesystem, a
renderer or a task runtime.

- **Models.** `DocumentModel` says what a document is made of and how it
  changes. A change is pure and returns its inverse, so history can undo any
  model the same way. `TextBuffer` is the text model: a persistent rope whose
  snapshots share structure, edited with `BufferEdit`.
- **History.** `History<Model, Group>` applies transactions atomically, keeps a
  bounded undo/redo stack, merges grouped edits, and keeps a journal of recent
  commits that readers can follow independently.
- **Sources.** `DocumentSource<Model>` is where a document's contents live:
  `load`, `save` (refused with `SourceError::Changed` unless the source still
  holds the expected version), and optionally a cheap `version` for noticing
  outside changes. `SourceId` identifies a source; clones share it. Source
  methods may block; Workbench calls them on worker threads.
  `MemorySource` keeps contents in memory, and its `set` and `remove` simulate
  outside changes.

```sh
cargo test -p bedtime-document
```
