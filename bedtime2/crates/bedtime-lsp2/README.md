# Bedtime language servers

Local stdio language servers over Editor2's persistent text and Workbench2's
canonical documents. The default build exposes the backend without Workbench;
the optional `workbench` feature adds `LspPlugin`. This crate installs no windows,
file policy, settings storage or editor panel. It depends on no v1 LSP, remote
or document service.

```rust,ignore
use bedtime_lsp2::{FileUri, ServerConfig, workbench::*};

app.add_plugins(LspPlugin);
world.entity_mut(workspace).insert(LspWorkspace {
    root: project_directory,
    servers: [("rust".into(), ServerConfig::default())].into(),
});
world.entity_mut(document.entity()).insert(LspDocument {
    uri: FileUri::from_path(&absolute_source_path)?,
    language: "rust".into(),
});
```

The application supplies validated absolute local file URIs, protocol language
IDs and effective server definitions. Grammar names are independent of language
IDs. `ServerConfig` has a literal program, argument vector, environment overrides,
JSON settings and initialization options. There is no shell evaluation,
installation, project-config discovery or remote transport. Bare executables use
the child environment's PATH; rust-analyzer also checks `$HOME/.cargo/bin` when
GUI PATH omits Cargo. The rustup dispatch filename is preserved.

## Ownership and execution

One server entity/task belongs to its workspace. Languages with identical launch
configuration and root share that process. Servers start lazily for viewed
eligible documents; hidden tabs and inactive layouts/workspaces continue syncing.
After attachment, dirty documents remain synchronized without viewers. Clean
viewless documents detach; the server remains available until restart, changed
configuration or workspace shutdown. Canonical text/history stays in Workbench.

`LspDocument` projects source identity; `DocumentLsp` owns its current attachment,
admitted journal cursor, shared diagnostics and document-specific error. A source
or language change retires that attachment and creates a fresh generation.
`workspace_servers` and `server_status` expose observations; `restart` and
`restart_instance` request managed retirement before a replacement starts.
The application owns status rendering and which document it describes.

`ServerHandle::activity()` and each `ServerObservation::activity` share an
immutable latest observation: process/server identity, negotiated capabilities,
document/request counts, active work, recent messages, stderr and last inbound
activity. The worker coalesces observations at 100 ms intervals (with immediate
launch/termination publication). Progress and logs use this replaceable snapshot,
not the lossless document-result FIFO, so a progress flood cannot exhaust that
queue. Only the worker owns protocol progress state; observers never own workers.

The client advertises work-done progress and handles token creation and
`$/progress` begin/report/end. Tokens distinguish integers from strings; omitted
report fields retain their previous values. Unknown tokens are ignored. Invalid
progress is bounded and logged without failing document synchronization; invalid
creation requests receive InvalidParams. Tokens retire on end or process exit,
not because diagnostics arrived or an arbitrary timer elapsed. Cancel controls
for individual operations are not implemented. `window/logMessage` and
`window/showMessage` are retained; message requests return a null selection and
record their message without pretending to present interactive choices.

The backend worker exclusively owns the process, negotiated capabilities,
request deadlines, wire versions, source roots, serialization, coordinate
conversion and diagnostic preparation. Nonblocking Unix pipes, partial writes,
a bounded FIFO and a command wake socket keep I/O off the main thread. Idle
poll waits are at most 100 ms so Workbench cancellation remains observable;
this timeout is an explicit idle-wakeup tradeoff. Cancellation is also checked
between commands and received messages. No pipe-reader threads are detached.

Workbench owns the task; its cancellation closure performs the shutdown/exit
handshake, terminates the process group as needed and reaps the leader before
returning. A non-reaping exit observation reserves the group ID until cleanup.
Applications must keep updating through the existing workspace quit gate.
`WorkspaceProcess` is deliberately unused because its rolling output retention
would corrupt protocol framing.

## Text and protocol contracts

The adapter reads `document_changes_since::<TextBuffer>` independently of editor
position-delta consumers. It advances its cursor only after queue admission.
Workers retain shared canonical roots; ordinary incremental edits materialize
only replacement bytes. Applied journal entries are released after processing.
Undo/redo are new revisions; whole-model replacements, journal gaps, oversized
change batches and unrepresentable CRLF-interior positions use a full refresh.
All changes in a batch use successive pre-change states. A definition request
follows its captured document update on the same FIFO.

Wire versions are checked increasing `i32` values, independent of canonical
`u64` revisions. UTF-8 is preferred in negotiation, with UTF-16 fallback. Conversion
uses the captured root and code units, never display columns or graphemes.
Invalid UTF-8/surrogate boundaries and reversed/out-of-document ranges fail.
Protocol columns beyond a line's end clamp to that end.

Initialization honors sync None/Full/Incremental, `openClose`, and `save:false`.
It retains the latest desired open while pending, and does not resurrect closed
attachments. Configuration/workspace requests are answered; unsupported requests
receive MethodNotFound. Server-initiated workspace edits are explicitly refused.
Only implemented client capabilities are advertised. Complete malformed frames
or unsupported encodings fail the instance with a visible status; retry is
explicit. Stderr retains its newest 64 KiB for startup/connection failures.

Push diagnostics are immutable Editor2 `Diagnostics` contributions, prepared
and indexed in the worker. A versioned report must match the current wire
version; unknown/stale versions are discarded. Empty reports clear previous
items. Unversioned reports are labeled best-effort; an edit invalidates their
presentation until another report arrives. Drawing and status counts require
the captured canonical revision. Reports include cached severity counts so
status rendering does not scan every diagnostic.

`request_definition(world, panel)` captures document, attachment, revision,
selection and a per-view ticket. Only matching results reach `take_definitions`.
Locations preserve the negotiated encoding and selection range. The application
owns loading, target coordinate conversion, preview policy and navigation.
`definition_ticket_is_current` supports the later open stage when an accepted
preview replacement legitimately retires/rebinds its originating view. It does
not replace document/load checks at that application's acceptance boundary.

`request_definition_at` accepts a byte position independently of the caret.
`request_query` accepts `LanguageQuery::Hover`, `Completion` or `Resolve` and
returns a captured origin. Each kind has an independent latest ticket per view;
`query_is_current`, `cancel_query` and `take_intelligence` expose the controlled
acceptance boundary. Queries follow admitted source updates on the same FIFO.
Supersession, view retirement and source changes invalidate/cancel them.

The worker normalizes legacy/Markdown/plain hover contents, completion arrays or
lists, incomplete flags, identifier filtering, server sort/filter text,
TextEdit/InsertReplaceEdit ranges, snippets and additional edits. Invalid items
are omitted; invalid response structure is an optional-request error. Only
implemented completion capabilities are advertised. Resolve retains shared raw
server data with a worker-computed byte charge; main-world admission does not
serialize it. The immutable suggestion labels are prepared off-thread too.
Current completion bounds are 10,000 entries, 128 additional edits, 64 KiB per
documentation value and 16 KiB identifier prefixes. Overlapping edits are refused.
The editor validates snippet syntax and applies a prepared replacement atomically.

Explicit ContentModified (-32801) and ServerCancelled (-32802) errors retry
at most three times with 100/200/400 ms backoff, within the original 20-second
request deadline. The worker retains captured parameters (32 MiB combined) and
origin. Document updates/closure and superseding queries remove deferred retries;
late cancelled responses are ignored. Other errors and exhausted retries remain
visible failures. Server initialization/process failures still require restart.

`ServerActivity.analysis_epoch` advances for accepted diagnostics, completed work
and rust-analyzer settling. Hosts can refresh empty hover observations without
polling a symbol forever. The optional `experimental/serverStatus` notification
retains bounded health/quiescence/message state. An empty hover/definition answer
while that server reports nonquiescence is deferred until it settles (or the
original deadline expires); useful nonempty answers remain immediate. The worker
owns this protocol state, and malformed status messages are logged, not fatal.

`notify_saved` requires an acknowledged successful save and its actual written
root. The application must not substitute current editor bytes for an older
asynchronous capture. Failed/conflicted/source-changed saves emit no notification.
Pending save admission can be retried; stale attachment generations are dropped.

## Bounds and current scope

| Work | Bound |
| --- | --- |
| Eligible source | 8 MiB, independent of editor and Tree-sitter limits |
| Frame / header | 16 MiB / 64 KiB |
| Command FIFO / conservatively charged snapshot bytes | 64 entries / 32 MiB |
| Encoded write backlog | 32 MiB |
| Pending requests / attached documents per server | 128 / 128 |
| Result FIFO | 64 entries |
| Registered progress tokens / active work | 128 / 64 |
| Recent server messages / each message or progress field | 64 / 4,096 characters |
| Diagnostic entries / message text | 10,000 / 4,096 characters |
| Definition targets | 128 |
| Request / shutdown handshake deadline | 20 seconds / 500 ms |
| Main-world result acceptance | 32 events per instance and a shared 2 ms gate |

Sources above the limit do not launch a server through the adapter. JSON escaping
can make an otherwise eligible source exceed the encoded frame limit. Overflow
refuses optional work or explicitly fails the affected session; protocol bytes
are never silently discarded. Bounds cover work/result retention, not a hard
allocator cap. Report destruction and root reference release may still occur on
the host; comparative frame/memory profiling remains future work.

Implemented features are local synchronization, successful-save notifications,
push diagnostics, definition/hover/completion/resolve requests and lifecycle/status
controls. Pull diagnostics, watched-file registration, formatting, rename,
code actions, workspace edits and remote services remain later slices.
The host currently chooses the root; marker discovery and multi-folder sessions
are not implemented. Closing a last viewer is distinct from application preview
replacement, which may explicitly close a clean document.

## Verification

```sh
cargo test --offline --locked -p bedtime-lsp2 --features workbench
cargo check --offline --locked -p bedtime-lsp2 --no-default-features
cargo test --offline --locked -p bedtime-lsp2 --features workbench --test real_intelligence -- --ignored --nocapture
cargo run --offline --locked -p bedtime-lsp2 --example diagnostic_probe
cargo test --offline --locked -p bedtime-lsp2 --features workbench real_rust_analyzer -- --ignored --nocapture
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_lsp.py
```

The deterministic Python fixture verifies server mirrors, initialization races,
Unicode/CRLF conversion, full refreshes, saved captures, diagnostics, failures,
shared/hidden/inactive views, journal eviction, restart, large-file refusal and
idle/blocked/inherited-pipe cleanup. The opt-in real-server test requires an
installed rust-analyzer and Rust toolchain and uses an offline dependency-free
Cargo fixture. Native verification additionally exercises the bEd2 bridge,
rendering, F12, temporary cross-file tabs, real saves, status errors and quit.

Verified on 2026-10-22: 21 deterministic tests and the opt-in real rust-analyzer
test passed. The native macOS scenario passed five captures, including absent
status without a server, definition previews, saved text, restart/failure and
observed shutdown/exit messages, real progress completion, panel restart and
observer lifetime. Native Linux UI verification remains pending.

`diagnostic_probe` characterizes the installed rust-analyzer using temporary,
offline crates and unsaved edits, with save checking disabled. It compares default
and experimental native diagnostics, printing server version, observation windows,
latency and messages. It does not change application preferences or claim an
end-to-end editor benchmark. A null latency means no matching report arrived
during the observation window, not that the server can never produce one.

Interactive verification on 2026-10-22: all 29 deterministic tests passed, including
independent query tickets, Unicode completion ranges, lazy resolve, supersession,
finite transient-error retry and empty results during project loading. The opt-in
real Rust/Go test passed hover/completion/resolve and Rust standard-library
`Vec`/`Vec::new` hover plus readable definition targets. The seven-capture native
interaction smoke additionally verifies actual standard-library selection/reveal.
