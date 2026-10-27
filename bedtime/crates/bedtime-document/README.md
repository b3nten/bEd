# bedtime-document

Headless document representations, source contracts and transaction history.
No Bevy, filesystem, renderer, UI or task runtime is installed.

`TextDocument` is the persistent UTF-8 `TextBuffer`; `BufferEdit` defines validated
changes and exact inverses. `BinaryDocument` is a read-only, version-bound
`ByteSnapshot`, so opening a sparse or large file does not materialize it.
`DocumentModel` defines shape/change semantics. `ByteDocument` adds byte decoding,
streaming encoding and snapshot loading for byte-oriented sources; other models
can implement their own typed `DocumentSource<Model>` integration.

`History<Model, Group>` owns atomic transactions, inverse history, grouping and a
128-commit independently readable journal. Source checkpoints and viewer state
are not undo records. Workbench and the standalone editor use this same engine.
The editor reexports `TextBuffer` and `BufferEdit`; its standalone owner is named
`MemoryTextSession` to distinguish it from the shared representation.

Sources own a process-local `SourceId`. Clones reuse it; changing a source's
location must not allocate a new identity. `DocumentSource<Model>` loads a model,
persists a captured model against its acknowledged baseline and optionally
observes external versions. Filesystem adapters delegate codecs to `ByteDocument`.
`MemorySource<Model>` supplies clean or explicitly unsaved in-memory storage.
Source methods are synchronous and may block: Workbench calls them on managed work.

`SourceVersion` is provider knowledge, distinct from content revision. `Changed`
refuses an overwrite; `Uncertain` requires observation before retry. A source
implementation must enforce its actual write expectation, not simply return success.

Only byte-addressable representations expose ranges; table-shaped models need not
pretend to have byte offsets. Paged binary reads are supported now. Paged text
loading/editing, byte mutation, SQLite and remote providers are deferred.

```sh
cargo test --offline --locked -p bedtime-document
```
