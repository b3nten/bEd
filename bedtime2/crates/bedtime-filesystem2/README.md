# bedtime-filesystem2

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
Applications retain their existing local path descriptors and opening policy.

`FilesystemBackend` supplies metadata, entry metadata (without following a final
symlink), directory reads, observed readers and staged writes. `FileReader` is a
streaming `Read + Seek` with captured metadata and explicit `verify()`. Convenience
source methods provide bounded whole reads, bounded prefixes, version-checked
ranges and exact streamed comparisons. `ReadSnapshot` returns bytes together
with their observation; text/binary interpretation stays with document adapters.
`FileVersion` is an opaque provider token, separate from canonical document revisions.

```rust,no_run
use bedtime_filesystem2::FileSource;

let source = FileSource::local("/project/data.bin");
let header = source.read_prefix(1024)?;
let bytes = source.read_range(&header.metadata.version, 0..header.metadata.len.min(32))?;
# Ok::<(), bedtime_filesystem2::Error>(())
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
use bedtime_filesystem2::{FileSource, WriteExpectation};
use std::io::Cursor;

let source = FileSource::local("/project/notes.txt");
let pending = source.prepare_write(&mut Cursor::new(b"new contents"))?;
// Host rechecks its source identity here.
let receipt = pending.commit(WriteExpectation::Contents(&mut Cursor::new(b"old contents")))?;
# Ok::<(), bedtime_filesystem2::Error>(())
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

## Verification

```sh
cargo test --offline --locked -p bedtime-filesystem2
cargo check --offline --locked -p bedtime-filesystem2
```

Tests cover streamed saves/conflicts, abandoned/failed staging, permissions and
symlink retargeting, create collisions, native source identity, bounded reads,
large sparse ranges, directory cancellation and the migrated operation/watch
regressions. Native filename/watcher fixtures may require execution outside the
macOS filesystem sandbox. No ignored performance probes run by default.

Verified 2026-10-24 on macOS: the focused suites passed 157 bEd2, 44 filesystem,
67 Filetree and nine Hex tests (two existing application opt-ins ignored), plus
independent Hex workbench/plain builds and the automation application build.
Strict native Hex verification passed all interactions/restoration and eight
validated captures in `target/native-fix-hex`. Filetree passed eleven validated
captures, native clipboard cut/paste/replacement and 10,000-entry virtualization
in `target/native-fix-filetree`.

The native verification repair fixes two failures also observed before extraction:
the [Bevy renderer patch](../../../vendor/bevy_render/BED_PATCHES.md) reads capture
pixels even when no swapchain drawable is available, and Filetree's smoke waits
for paste's asynchronous row reveal instead of clicking the expanding destination
again. The native `screenshot_without_drawable` regression verifies both colored
and fully transparent output. The repaired build also passed 30 UI2, 12 Automation,
67 Filetree and 157 application tests (two existing opt-ins ignored).
Native Linux was not exercised.

## Canonical document sources (`workbench` feature)

`Filesystem::document_source(path)` is a worker-only canonicalization/interning
operation returning `FsDocumentSource`. Provider clones and symlink aliases share
identity; hard-link paths remain separate. Sources have stable IDs and mutable
locations: confirmed moves preserve the same document/history. Retargeted symlinks
resolve to their new canonical target on subsequent opening. Native paths remain
lossless. Raw `FileSource` is still the synchronous path-based byte-access value.

`FsDocumentSource` implements `DocumentSource<Model>` for byte representations.
It delegates codecs to the model, streams writes from captured model/baseline
readers and retains exact-baseline overwrite protection. It owns no canonical
editable copy or adapter-maintained checkpoint. Binary sources use bounded,
version-checked snapshots and reject saving.

Install `workbench::FilesystemDocumentsPlugin` with Workbench. It supplies standard
asynchronous file opening, shared native invalidation with bounded polling fallback,
file-panel source projection and reusable external-mutation reconciliation.
`open_file_document` classifies new regular sources independently of the selected
viewer, reuses existing documents, and invokes a completion callback outside drawing.
Text remains UTF-8/NUL-checked up to 256 MiB. Unsupported/nonregular sources report
errors; an assigned document is never silently changed to another representation.

`FileDocumentPanel` follows source locations and handles missing sources through
the ordinary checked panel lifecycle. The application callback updates only its
presentation. `prepare_source_change(...).spawn(...)` holds repository write admission through
worker I/O and retained main-world reconciliation, including after cancellation
or observer closure. Its presentation handle receives output only after reconciliation.
Native Filetree operations update source identities from their actual outcomes.
The watcher owns one managed worker per World, not one thread per source;
`source_watch_error` exposes failures while periodic checks continue.

Verified 2026-10-26 on macOS: 49 tests with `workbench`, including source identity,
symlink/hard-link behavior, exact-baseline saves after moves, sparse binary ranges
and retained mutation reconciliation during shutdown. The affected application/
viewer suites and native Code/Hex/Filetree/image/model/Git/session/quit/drag flows
also passed; current artifacts and feature checks are recorded in
[CONTEXT.md](../../../CONTEXT.md). Native Linux remains unverified.
