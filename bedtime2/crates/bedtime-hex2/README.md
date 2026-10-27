# bedtime-hex2

Read-only byte viewing, with an independent UI2 canvas and an optional complete
Workbench panel. The default build requires no Workbench or filesystem service.
`HexView::show` takes a length and a cached-byte lookup, draws only visible rows,
and reports needed ranges and copy/find/navigation intentions. Stored offsets
remain integers, including beyond 4 GiB. `parse_hex` and `parse_offset` validate
user input.

## Workbench panel

Enable `workbench` and install `HexPlugin` alongside the host's Workbench and UI2
plugins. Compose a concrete panel with a document byte capture:

```rust,ignore
use bedtime_workbench2::document_bytes;
use bedtime_hex2::workbench::{HexPanel, HexPlugin};
use bedtime_workbench2::InArea;

app.add_plugins(HexPlugin);
commands.spawn((HexPanel::new(document_bytes(world, document.id())?), InArea(area)));
```

The source is mandatory. Required `PanelInstance` metadata/drawing exists at
spawn; there is no registration catalog or delayed panel initialization. Drawing
and workers require the normal Workbench services, including its shared clipboard
for copying. The library creates no window, renderer, project root or documents.

The panel owns Go To, forward/backward wrapped byte or UTF-8 search, byte/numeric
inspection, endianness, copy formats, loading/error/reload presentation and user
jobs. Workbench shares 64 KiB pages across panels viewing the same document capture,
retaining at most 32 pages per capture. Viewport demand is bounded independently
of source length. Reads use Workbench document byte access; pages, search and
copy validate the captured revision or binary snapshot version. Search handles page and wrap boundaries;
copy is limited to 16 MiB of selected bytes. Hex remains read-only.

Initial loading, search/copy and page reads use Workbench managed tasks. Dropping
panels cancels their user/load jobs; removing the last view retires its source
cache and page job. Panel retirement does not block the UI thread. Hosts must
continue updating through the normal managed-work shutdown gate.

For hosts with a broader file lifecycle, `prepare(source)` runs on their loader
worker and `accept(world, panel, prepared)` installs a checked result. `update`
may be called explicitly once per frame instead of installing `HexPlugin`.
`draw` accepts a host context-menu contribution and returns a reload intention;
`reload` provides the standalone panel's loading route. Confirmed source rebinding
through `accept` retains navigation, clamps it to the new size and cancels obsolete
user jobs. A prepared result must pass the host's load-generation checks first.

bEd2 composes this panel within its existing file tab, preserving preview reuse,
Open With, placement and persistence. Its small adapter supplies the menu contribution and shared file reload route. It owns no Hex search,
rendering, paging or filesystem implementation. Persisted viewer descriptors
retain their existing application schema and local paths.

## Verification

```sh
cargo test --offline --locked -p bedtime-hex2
cargo test --offline --locked -p bedtime-hex2 --features workbench,automation
cargo check --offline --locked -p bedtime-hex2 --no-default-features
cargo build --offline --locked -p bed2 --features automation
python3 -B bedtime2/tools/smoke_hex.py
# Interaction-only alternative; explicitly does not verify rendered pixels:
python3 -B bedtime2/tools/smoke_hex.py --functional-only
```

The panel tests run without bEd2, including ordinary concrete-panel loading,
shared-cache retirement, confirmed moves, large sparse files, stale reads and
search boundaries. The native smoke adds real panel controls, clipboard,
resizing/font changes and persisted explicit-viewer restoration. `automation`
exposes optional inspection data without adding an application command registry.

The Workbench adapter now consumes `DocumentByteSource` captures from
`document_bytes`. It supports both `TextDocument` (including unsaved text and
undo/redo) and `BinaryDocument`; every file-backed Hex view shares the source's
canonical document. Page caches and read jobs are owned by Workbench's shared byte
access. Hex retains navigation, search/copy controls and formatting. The canvas
without Workbench remains independent.
