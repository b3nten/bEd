# bedtime-filesystem

Headless file access for Bedtime consumers. The default build has no Bevy, UI or application-settings dependency.
Native byte operations are synchronous;
call them from the host's existing managed workers. Only the local backend is
implemented. No SSH connection or additional task runtime is installed.

## Sources and reads

`Filesystem` owns a backend and a process-local identity. `FileSource` pairs that
identity with a lossless native path. Clones share identity; distinct filesystem
instances cannot collide in source caches just because their paths match.
`Filesystem::local()` and `FileSource::local(path)` share the native filesystem
identity. Identity is not a persisted endpoint format. Paths are not converted
to display strings for access. Canonicalization is an explicit worker operation.


`FilesystemBackend` supplies metadata, entry metadata (without following a final
symlink), directory reads, observed readers and staged writes. `FileReader` is a
streaming `Read + Seek` with captured metadata and explicit `verify()`. Convenience
source methods provide bounded whole reads, bounded prefixes, version-checked
ranges and exact streamed comparisons. `ReadSnapshot` returns bytes together
with their observation; text/binary interpretation stays with document adapters.
`FileVersion` is an opaque provider token, separate from canonical document revisions.

```rust,no_run
use bedtime_filesystem::FileSource;

let source = FileSource::local("/project/data.bin");
let header = source.read_prefix(1024)?;
let bytes = source.read_range(&header.metadata.version, 0..header.metadata.len.min(32))?;
# Ok::<(), bedtime_filesystem::Error>(())
```

Native readers reject nonregular files and use nonblocking opens on Unix so a
concurrent replacement with a FIFO cannot block opening. Verification compares
both the open object and current path against the captured version. Unix tokens
include device, inode, length, modification and change times. This detects
replacement/truncation without relying on timestamps alone. Observations are not
locked filesystem snapshots; external changes after a final check remain possible.

## Checked streaming writes

`prepare_write` streams bytes into an exclusively created sibling temporary,
preserves existing permissions and syncs the file. It does not change the target.
Dropping the returned `PendingWrite` removes its owned temporary, including when
encoding fails. Cleanup checks object identity before removing a staging name.
The host can recheck document/source generations before calling `commit`.

`WriteExpectation::Contents` compares exact baseline bytes from a reader, verifies
the destination binding and atomically replaces the target. `Missing` publishes a
new file without replacing a concurrent creation, including a dangling symlink.
Atomic no-replace publication uses the shared macOS/Linux rename primitive;
unsupported targets return `Unsupported`.
`ChunkReader` streams an existing chunked model without collecting another full
buffer. Text adapters can use independent readers over the captured model and
its acknowledged checkpoint; the filesystem has no knowledge of ropes or UTF-8.

```rust,no_run
use bedtime_filesystem::{FileSource, WriteExpectation};
use std::io::Cursor;

let source = FileSource::local("/project/notes.txt");
let pending = source.prepare_write(&mut Cursor::new(b"new contents"))?;
// Host rechecks its source identity here.
let receipt = pending.commit(WriteExpectation::Contents(&mut Cursor::new(b"old contents")))?;
# Ok::<(), bedtime_filesystem::Error>(())
```

Changed sources return `Error::Changed`; failures after publication whose result
cannot be confirmed return `CommitUncertain`. Consumers must refresh uncertain
sources before retrying. The source attachment maps these to Workbench's conflict/source
refresh gates and acknowledges only the captured successful save. Filesystem
replacement is not atomic content CAS against unrelated external writers, nor a
promise of parent-directory crash durability. The document source owns save/write admission; Workbench owns source generations
and acknowledgement. Hosts choose recovery decisions.

## Directories, operations and watching

Native directory reads retain all entries, native names and hidden attributes.
They reject replaced directory symlinks/ancestors before enumeration and honor
cancellation. Filetree owns sorting, visibility, cached projection and selection.

`operations` owns the existing native root-scoped create/rename/copy/move/duplicate/
trash implementation, with progress, cancellation and explicit partial outcomes.
Copies stage and commit without replacement; cross-device moves fail without a
copy/delete fallback. Source/temporary validation and the original regression
coverage moved here unchanged in meaning. Filetree reexports its existing public
operation names, so application consumers keep the same API.

`watch::DirectoryWatcher` owns bounded, nonrecursive native invalidations. macOS
uses directory-only kqueue; other targets use notify with a bounded callback queue.
Batches report lost registrations, overflow/rescan and errors. Consumers reconcile
observations and own fallback polling. Native operations and watching are currently
local services; adding a remote provider will also require transport-specific
mutation and watch implementations.

## Document sources

`Filesystem::document_source(path)` canonicalises a path (on a worker) and
returns an `FsDocumentSource`: one identity per canonical file, shared by
symlink aliases (hard links stay separate), which follows confirmed moves
(`move_document_sources`) and notices deletions (`delete_document_sources`).
It's a `bedtime_document::DocumentSource<M>` for any `M: FileModel`, the
model's `decode` and `encode`; `TextBuffer` is one (UTF-8, no NUL bytes, up to
256 MiB).

A save is refused (`Changed`) unless the file is still what was last loaded or
saved: the same version, or, when the version moved on without the contents
changing (a move changes a file's change time), the same bytes, compared
exactly when committing. A document that was never on disk is only written
where nothing exists yet.

## Verification

```sh
cargo test -p bedtime-filesystem
```
