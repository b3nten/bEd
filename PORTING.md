# Bed implementation and source history

The current application follows the user-approved dockable workspace design.
The dated entries record implementation and validation at each stage. Historical
dependency revisions describe that stage, not an obligation to keep ned's build
stack. Current dependencies are selected by Cargo.toml and Cargo.lock; retained
local forks and their purposes are listed in `vendor/README.md`.

## Pinned baseline

Upstream: https://github.com/nealmick/ned

Revision: **2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff**. `UPSTREAM_REVISION`
records the commit and `UPSTREAM_SUBMODULES` records all 23 gitlink revisions.
The full source checkout is in `reference/ned` (ignored by Git), with all 23
submodules initialized at their recorded gitlinks. The upstream ImGui revision
is `ca49eff3980443a97c470e09fe55b1740cfb9584`, used for original headless tests.
Tree-sitter and all 17 grammar repositories are recorded there as source
provenance for the original highlighting fixtures. The original port vendored
those sources; Bed's current dependency choices are recorded separately below.
No upstream C++ editor is linked into Bed.

LICENSE preserves upstream's MIT/X Consortium notice. NOTICE identifies ned,
nealmick, and contributors. SourceCodePro-Regular.ttf is copied unchanged from
the pinned resources; Adobe's SIL Open Font License accompanies it.

The source was read before translating each implementation and its associated
tests. The rope is upstream's persistent binary tree with small-leaf fusion,
not an AVL tree or a replacement rope crate. GUI types stay out of document,
operations, commands, history and save code.

## Conversion mapping

“Partial” means implemented portions are identified below; it does not claim a
whole-module conversion or full upstream parity.

| Original source | Rust source | Status |
| --- | --- | --- |
| `editor/buffer/text_buffer.h`, `.cpp` | `crates/bed-core/src/buffer/text_buffer.rs` | Converted: persistent tree, edits, byte/line indexing, snapshots |
| `editor/editor_state.h`, `.cpp` | `crates/bed-core/src/editor_state.rs` | Converted: bytes, metadata, BOM, newline normalization |
| `editor/util/utf8.h`, encoding/indexing helpers in `editor_utils.h` | `crates/bed-core/src/util/utf8.rs` | Converted helpers; remaining GUI helpers are in view layout |
| `editor/editor_operations.h`, `.cpp` | `crates/bed-core/src/editor_operations.rs` | Converted: apply/invert, byte spans, sequential UTF-16 changes |
| `editor/editor_commands.h`, `.cpp` | `crates/bed-core/src/editor_commands.rs` | Converted editing/navigation/history; save orchestration is on Editor/EditorApi |
| `editor/editor_view_state.h`, `.cpp` | `crates/bed-core/src/editor_view_state.rs`, `crates/bed-ui/src/editor_frame.rs` | Selection/movement converted; GUI-dependent scrolling lives in frame |
| `editor/editor_events.h`, `.cpp` | `crates/bed-core/src/editor_events.rs` | Converted: main-thread subscriptions and notifications |
| `util/project_undo.h`, `.cpp` | `crates/bed-core/src/project_undo.rs` | Converted: file stacks, 300 ms coalescing, selection groups, v2/v3 JSON |
| `editor/services/save_service.h`, `.cpp` | `crates/bed-session/src/save_service.rs` | Converted: chunked writes, BOM, truncated guard, 1 s idle autosave |
| `editor/editor.h`, `.cpp` | `crates/bed-session/src/editor.rs` | Model/events/history/save/highlight/git/diagnostics composition translated; shared LSP and project-history handles follow original host-owned services |
| `editor/editor_api.h`, `.cpp` | `crates/bed-session/src/editor_api.rs` | Standalone/embedded document, caret, event, save, git and diagnostic service boundary translated; presentation queries use the borrowed frame/LSP pane adapter |
| `workbench.h`, `.cpp` | `src/workbench.rs` | Application shell redesigned with ordinary document/tool/terminal docking tabs, shared Session documents, free native undocking, guarded group close and project persistence; old splitters/forced redocking removed |
| `ned_embed.h`, `.cpp` | `crates/bed-session/src/editor_session.rs`, `crates/bed-ui/src/editor_view.rs` | Intentional API change: session/services plus individual document widgets; host owns containers, style/fonts, context/frame and native windows. Old BedEmbed facade removed |
| Workbench/embedding ownership boundary; no upstream Workbench test module | `crates/bed-ui/tests/embedding.rs`, `tests/workbench_focus.rs`, Workbench module tests, `crates/bed-session/tests/shared_undo.rs` | Added native ImGui lifecycle/docking/focused-input tests and multi-editor shared-history persistence/reload regressions |
| `main.cpp`, `ned.h`, `ned.cpp` | `src/main.rs`, `src/bed.rs` | Standalone winit/wgpu lifecycle, settings/files/services composition, native dialogs/clipboard, asset upload, CRT effects, configured frame pacing and native chrome/menu integration translated; every-platform runtime acceptance remains pending |
| `editor/editor_input.h`, `.cpp` | `crates/bed-ui/src/editor_input.rs` | Keyboard/Unicode/clipboard, source input ordering, focus/hit guards and mouse selection translated |
| `editor/editor_frame.h`, `.cpp` | `crates/bed-ui/src/editor_frame.rs` | Custom document layout, width cache, gutter/text/caret, scrolling, overlays/minimap and diagnostics/hover translated; shell and terminal composition moved to Workbench |
| `editor/views/view_layout.h`, GUI measurement helpers in `editor_utils.h` | `crates/bed-ui/src/views/view_layout.rs` | Layout metrics, native float glyph measurement, byte hit testing, tabs and rainbow translated |
| `editor/views/text_view.h`, `.cpp` | `crates/bed-ui/src/views/text_view.rs` | Text/selections/current line/indent guides/syntax spans and UTF-16 diagnostic squiggles/hover translated |
| `editor/views/caret_view.h`, `.cpp` | `crates/bed-ui/src/views/caret_view.rs` | Basic caret drawing translated |
| `editor/views/gutter_view.h`, `.cpp` | `crates/bed-ui/src/views/gutter_view.rs` | Line numbers/range emphasis, source git marks and severity diagnostics/hover translated |
| `editor/views/title_bar_view.h`, `.cpp` | Removed during crate extraction | Obsolete standalone title-strip presentation; Workbench tabs and native titlebar own document/window titles |
| `editor/util/editor_finder.h`, `.cpp` | `crates/bed-ui/src/util/editor_finder.rs` | Converted: byte search, case toggle, overlapping matches, navigation, replacements, multi-selection and two-row inline UI |
| `editor/util/editor_line_jump.h`, `.cpp` | `crates/bed-ui/src/util/editor_line_jump.rs` | Converted: pane-relative geometry, input/focus lifecycle, clamping and centered navigation |
| `tests/editor/text_buffer_test.cpp`, `utf8_utils_test.cpp` | state and UTF-8 module tests | Translated cases, including all upstream buffer tests |
| `tests/editor/editor_operations_test.cpp`, `editor_commands_test.cpp`, `multi_cursor_test.cpp` | operations/commands module tests | Translated cases (some Catch sections grouped into one Rust test) |
| `tests/editor/save_service_test.cpp`, `undo_service_test.cpp` | `crates/bed-session/tests/save_undo.rs`, commands/history module tests | Translated behaviors plus write-error/reopen regressions |
| `tests/monaco/text_model.h`, `.cpp` | `tests/support/text_model.rs` | Translated test-only adapter; not an alternative production model |
| `tests/editor/text_model_data_test.cpp`, `text_model_test.cpp`, `model_edit_operation_test.cpp`, `editable_text_model_test.cpp` | `crates/bed-core/tests/model.rs` | All 46 original fixture cases translated |
| `resources/fonts/SourceCodePro-Regular.ttf` | same path | Copied, licensed, used at upstream's default 20 logical pixels |
| `files/files.h`, `.cpp` | `src/files/files.rs`, `crates/bed-files/src/files.rs`, `crates/bed-session/src/editor_session.rs` | Explorer composition, binary check and 1 MiB cap retained; document lifecycle belongs to Session |
| `files/file_explorer_events.h`, `.cpp` | Removed during crate extraction | Legacy explorer lifecycle notifications replaced by Session events and returned panel actions |
| `files/file_tree.h`, `.cpp` | `src/files/file_tree.rs` | Converted: lazy directory tree, refresh/open-state retention, row layout and icon drawing |
| `files/file_finder.h`, `.cpp` | `crates/bed-files/src/file_finder.rs`, `src/files/file_finder.rs` | Converted: background scans, substring filtering and path-length ranking; app owns selection overlay |
| No corresponding module in the pinned revision | `crates/bed-files/src/content_search.rs`, `search_files.rs`, `src/files/content_search.rs` | Bed extension: cancellable Git-aware project discovery, streamed results, capped reads and overlapping byte matches; app owns result navigation |
| No corresponding module in the pinned revision | `crates/bed-files/src/file_monitor.rs`, file-service/host integration | Bed extension: active-document metadata polling, bounded disk reads, clean-buffer reload and dirty-buffer conflict retention |
| `util/settings.h`, `.cpp` | `src/util/settings.rs` | Profile lifecycle, source section layout/labels, fullscreen SVG close header, all font/theme/toggle/CRT controls, macOS opacity/blur, configuration/dashboard links and remembered floating embedded window translated |
| `util/keybinds.h`, `.cpp` | `src/util/keybinds.rs`, frame shortcut dispatch | Key names, action map/reload and editor/file/LSP/terminal dispatch translated |
| `util/font.h`, `.cpp` | `src/util/font.rs` | Converted: font selection/reload, 52px large font, VT100 metrics, source priority/range metadata, braille and 70% color-emoji merge through bundled FreeType/PlutoSVG |
| `util/icons.h`, `.cpp` | `src/util/icons.rs`, host upload adapter | File-icon mapping and 32px rasterization translated; GPU texture ownership uses wgpu |
| `util/welcome.h`, `.cpp` | `src/util/welcome.rs`, `src/util/workspace_state.rs` | Intentionally replaced with recent-project picker/Open Folder and per-project saved layouts; unused welcome artwork removed during crate extraction; remaining assets stay attributed |
| `editor/views/minimap_view.h`, `.cpp` | `crates/bed-ui/src/views/minimap_view.rs` | Density cache, UTF-8/tab columns, highlighted runs, viewport slider, wheel and dragging translated |
| `editor/services/highlight/capture_map.h`, `.cpp` | `crates/bed-highlight/src/capture_map.rs` | Capture hierarchy, slot mapping and priority translated |
| `editor/services/highlight/span_map.h`, `.cpp` | `crates/bed-highlight/src/span_map.rs` | Incremental span/line morphing translated |
| `editor/services/highlight/tree_sitter.h`, `.cpp` | `crates/bed-highlight/src/tree_sitter.rs` | Pinned parser/grammar/query engine, incremental snapshots, predicates, shared query cache and chunked results translated |
| `editor/services/highlight/highlight_service.h`, `.cpp` | `crates/bed-highlight/src/highlight_service.rs` | Synchronous small parses, worker cancellation, span publication and theme remapping translated |
| `shaders/shader.h`, `.cpp`, `shader_types.h`, `.cpp`, `shader_manager.h`, `.cpp` | `crates/bed-effects/src/*.rs` | Owned targets, parameters, quad passes and temporal ping-pong translated to wgpu |
| `shaders/vertex.glsl`, `fragment.glsl`, `burn_in.frag` | `crates/bed-effects/src/*.wgsl` | Original effect formulas and pass/texture ordering translated |
| `util/macos_window.h`, `.mm` | `src/util/macos_window.rs`, standalone host | Full-size native content, passive HUD blur, Metal-only opacity, traffic lights and centered title translated while preserving winit's content view/delegate; Bed adds seven uniformly spaced workspace actions |
| `util/windows_window.h`, `.cpp`; Windows caption drawing in `workbench.cpp` | `src/util/windows_window.rs`, standalone host | Per-window subclass, DWM frame/corners, excludes-first hit tests, resize grips, caption buttons/Snap return codes and ImGui caption drawing translated; native Windows execution pending |
| No corresponding upstream menu module | `src/util/macos_menu.rs` | Requested Bed extension: Muda native App/File/Edit/View/Window menus, configured accelerators, main-thread actions and focus-safe edit routing; standalone only |
| `scripts/pack-mac.sh`, `pack-deb.sh`, `build-win-ci.bat`, Windows icon/version resources | `scripts/pack-mac.sh`, `pack-deb.sh`, `pack-win.ps1`, `windows_resources.rs`, `resources/windows/bed.rc.in` | Bed app/archive/Debian/portable packaging with original icons, renamed metadata, runtime resources and license notices; macOS bundle verified, Linux/Windows execution pending |
| Native release acceptance; no upstream equivalent for this Rust backend | `.github/workflows/ci.yml`, `scripts/ci-linux-native.sh`, `collect-licenses.py` | Three-OS build/test/Clippy/release/package jobs plus native GPU/lifecycle fixtures and std-only notice collection configured; workflow has not run here |
| Original grammar sources and `resources/queries` | Released Tree-sitter Cargo packages, `resources/queries` | Original native copies replaced by released providers; query attribution and tested syntax behavior retained |
| Original highlight tests | `crates/bed-highlight/tests/highlight.rs`, capture fixtures | All 11 original cases plus native differential captures for every supported language |
| `editor/views/text_view.cpp`, `caret_view.cpp`, `gutter_view.cpp` paint/measurement leaves | `crates/bed-ui/tests/views.rs`, `tests/fixtures/upstream_views.cpp`, `views.json` | Fourteen fixtures generated from pinned original source compare native glyph widths, vertex/index ordering, syntax/Git/diagnostic colors and UTF-16 squiggles |
| `editor/util/doc_path.h` | `crates/bed-core/src/util/doc_path.rs` | Converted: weak canonicalization with existing symlink prefixes and lexical missing tails; native Windows prefix spelling adapter |
| `lsp/lsp_request.h` | `crates/bed-lsp/src/lsp_request.rs` | Converted: ticketed pending/result state, cancellation and stale reply rejection |
| `lsp/lsp_document_sync.h`, `.cpp` | `crates/bed-lsp/src/lsp_document_sync.rs` | Converted: queued opens, language resolution, negotiated sync/save, lazy full text and ordered UTF-16 change ranges |
| Config/discovery helpers in `lsp/lsp_client.cpp` | `crates/bed-lsp/src/lsp_config.rs` | Converted: original JSON shape, extension matching, path order and Windows percent expansion |
| `lsp/lsp_locations.h` | `crates/bed-lsp/src/lsp_locations.rs` | Converted: definition/location links and reference response adapters |
| Pinned `lsp-framework/lsp/uri.h`, `.cpp` | `crates/bed-lsp/src/lsp_uri.rs` | Converted URI/path adapter: original byte encoding and filesystem/path distinction |
| `editor/services/diagnostics/diagnostics_store.h`, `.cpp` | `crates/bed-lsp/src/diagnostics/diagnostics_store.rs` | Converted: shared document store, version filtering, inclusive ranges and line severity |
| `editor/views/hover_markdown.h`, `.cpp` | `crates/bed-ui/src/views/hover_markdown.rs` | Converted: fenced code, prose/rules and line splitting |
| `editor/views/hover_trigger.h` | `crates/bed-ui/src/views/hover_trigger.rs` | Converted: movement/rest/dismissal state with original delay floor and frozen targets |
| `editor/services/git/line_diff.h`, `.cpp` | `crates/bed-session/src/git/line_diff.rs` | Converted: original LCS line diff, tie ordering, prefix/suffix trimming and large-middle fallback |
| `editor/services/git/git_repo.h`, `.cpp` | `crates/bed-session/src/git/git_repo.rs` | Converted: local repository discovery, HEAD blobs and worktree status through libgit2 |
| `editor/services/git/git_service.h`, `.cpp` | `crates/bed-session/src/git/git_service.rs`, `git/mod.rs` | Converted: normalized paths, HEAD baselines, incremental line cache/counts and timed modified-file polling |
| Pinned framework `io/standardio`, `io/stream`, `connection` | `crates/bed-lsp/src/connection.rs` | Translated framing rules with bounded std I/O |
| Pinned framework `json/json`, `jsonrpc` | `crates/bed-lsp/src/jsonrpc.rs` | Translated messages/IDs/errors/batches, strict UTF-8 and duplicate-key rejection |
| Pinned framework platform `process`, `messagehandler`, `requestresult` | `crates/bed-lsp/src/process.rs`, `message_handler.rs` | Owned std children/pipes/workers and main-thread callback dispatch replace platform infrastructure |
| `lsp/lsp_client.h`, `.cpp` | `crates/bed-lsp/src/lsp_client.rs`, editor/host binding | Configuration, initialization/handlers, document synchronization and server lifecycle translated |
| `lsp/lsp_goto.h`, `.cpp` | `crates/bed-ui/src/lsp/lsp_goto.rs` | Converted: caret UTF-16 positions, definition/reference requests and ticketed results |
| `lsp/lsp_uri_options.h`, `.cpp` | `crates/bed-ui/src/lsp/lsp_uri_options.rs` | Converted: shared picker geometry/styles, wrapping keys, empty/pending display and document-open actions |
| `lsp/lsp_symbol_info.h`, `.cpp` | `crates/bed-ui/src/lsp/lsp_symbol_info.rs` | Converted: all hover-content shapes, caret anchors, rest-triggered requests, sticky popup and cancellation |
| `lsp/lsp_dashboard.h`, `.cpp` | `crates/bed-ui/src/lsp/lsp_dashboard.rs` | Converted: cached discovery/status, table, refresh/reload/config actions and dismissal |
| `LSPClient::keybinds/render`, host dashboard drawing | `crates/bed-ui/src/lsp/lsp_ui.rs` | Rust aggregate adapter: original dispatch/render order; document actions return to host after the client borrow ends |
| `editor/views/diagnostic_style.h`, `hover_tooltip.h`, `.cpp` | `crates/bed-ui/src/views/diagnostic_style.rs`, `hover_tooltip.rs` | Severity palette/labels, one-tooltip-per-frame arbitration and highlighted markdown/diagnostic rendering translated |
| Pinned `imgui-terminal/terminal.h`, `.cpp` emulator/selection/palette | `crates/bed-terminal/src/terminal.rs` | Alacritty-owned parser/grid with translated source attributes, palette, cursor styles, selection, resize and protocol metadata; 12 unchanged original-source cases / 54 transitions pass |
| Pinned `imgui-terminal/terminal.cpp` drawing/input adapter | `crates/bed-terminal/src/terminal_view.rs` | Converted: DrawOp row/overlay cache, ordered colors/styles/clips, transparent background, cursor/text blink, keyboard/mouse reporting and raw bracketed paste |
| Pinned terminal font setup in `terminal.cpp` | `crates/bed-terminal/src/terminal_font.rs` | Four real font styles, base-only Unicode fallbacks, override, FreeType collection faces and original ceil metrics translated; platform discovery uses owned file adapters |
| Pinned terminal Unix/Windows tty/exec/pump/teardown code | `crates/bed-terminal/src/terminal_pty.rs` | Owned PTY/child/worker adapter, child-only environment/cwd and bounded read/write/resize/shutdown |
| `util/ned_terminal.h`, `.cpp` | `crates/bed-terminal/src/bed_terminal.rs` | Converted: fixed non-reorderable shell tabs, trailing +, stable IDs, close/respawn, visibility/focus/font resync and project working directory |
| Pinned terminal nine core and six input automation fixtures | `crates/bed-terminal/tests/terminal_core.rs`, `terminal_view.rs`, `terminal_input.rs`, `tests/fixtures/terminal_*` | Unchanged DrawOp goldens, native ImGui/Bash input scripts and strengthened unchanged-source state differential fixtures pass |

## Initial-port differences and staging limits (historical)

These entries describe the initial port. The dated workspace revision and crate
extraction sections below supersede the old single-buffer and BedEmbed design.

- Rust uses `Vec<u8>` for document bytes, including invalid UTF-8 and NUL, and
  immutable `Arc` nodes for persistent snapshots. Row/column coordinates remain
  signed and byte-based. UTF-16 conversion preserves upstream edge behavior.
- Borrowed command/API facades replace self-referential C++ pointers. Clipboard
  data enters/leaves commands as bytes; native clipboard code is in input/host.
- Save errors return to the UI and retain dirty state. Upstream checks opening
  the stream but does not check every write/close. Failed file opens preserve
  the current document and caret; the original shell replaces them with a
  failure message. Reopening a dirty document saves before reading it.
- History JSON retains its upstream v2/v3 field names and shape. Its file is
  `.undo-redo-bed.json`; Bed does not write ned's history file. Invalid UTF-8
  history cannot be serialized into JSON strings (matching upstream's JSON
  limitation), but in-memory undo and document save still retain exact bytes.
- As upstream, empty document paths are not recorded in project history.
  Named-document undo/redo is validated. Opening a project now loads its history;
  the standalone host flushes history on shutdown.
- Commands publish only newly added changes through an event cursor. Ordered
  pending edits remain available until highlighting consumes them. Rust polls
  the event-driven highlight work on the main thread and rejects stale worker
  generations and paths.
- The standalone host is single-buffer as requested. This pinned upstream
  revision now uses a multi-tab Workbench even standalone; that change from
  the plan's baseline is explicitly not adopted for standalone Bed. Embedding
  and host-driven multi-tab docking are implemented through Rust BedEmbed.
- Embedded editors share one `Rc<RefCell<ProjectUndo>>`, loaded once by the
  project shell and flushed once at cleanup. The default public `Editor.undo`
  remains the standalone store; borrowed/shared guards route commands, file
  opens, pending caret updates and external-reload invalidation to the active
  store. Tabs keep separate documents, selections, view state and monitors.
  No C++ ABI compatibility is promised.
- BedEmbed stores the host context's generation identity and rejects a
  different/replaced context or initialization/settings changes inside an
  open frame. It never creates/destroys a context, begins/ends a frame, or
  owns a window/render backend. Floating settings append fonts to the host's
  compatible FreeType atlas and preserve existing host fonts/style. Window IDs
  include an instance identifier so separate workbenches do not share dock
  memory. Failed opens/close saves preserve the existing tab, matching Bed's
  documented error-preservation policy rather than upstream's failure screen.
- A temporary File/View menu retains native Open File/Save As dialogs.
  Cmd/Ctrl+O now opens a project folder, as upstream does; Cmd/Ctrl+Shift+S
  remains an additional Save As shortcut.
  Home/End/PageUp/PageDown are additional navigation shortcuts. Upstream's
  editing bindings, Alt cursor/word gestures, find and Cmd/Ctrl+; remain.
- Finder and line-jump preserve the original 255/31-byte input limits and
  focus/dismissal behavior. Rust truncation retains a valid UTF-8 boundary;
  overflowing line-number input clamps to signed 32-bit bounds, where the
  original `atoi` overflow had undefined behavior.
- Settings, themes and keybinds keep their upstream formats and filenames,
  including the `ned.json` profile pointer. Missing defaults are seeded into
  `~/bed/config`; existing files are retained. The JSON stream reader preserves
  upstream's acceptance of trailing tutorial comments after a complete value.
- Settings retain the original fullscreen sizing, style scopes and internal
  Modal flag through a small bound-context Begin/End guard, because the safe
  binding accepts only public window flags. Embedded Settings keep the host
  palette, movable/resizable geometry and native title-bar close behavior,
  and omit platform/CRT controls as upstream does. Header focus state is owned
  by each Settings instance rather than a process-wide static. The shader
  heading reads "Shaders" for the wgpu implementation; a host that has not yet
  uploaded icons receives a close glyph in the same source hit rectangle.
- Highlight queries retain all existing lookup paths and additionally resolve
  beside an installed binary at `../share/Bed/queries`, then on Linux at
  `/usr/share/Bed/queries`. A temporary package-layout fixture verifies content
  loading, executable-local override priority and existing Resources fallback.
- Upstream's finder uses case-insensitive substring matching and path byte-length
  ranking, includes `.git`, and does not apply ignore files. Bed preserves those
  algorithms. Project content search and active-document monitoring are Bed
  extensions specified by the plan; this pinned revision has no corresponding
  modules. Search reuses the byte-match and file-read algorithms. Monitoring
  checks at most once per 500 ms while rendering, ignores matching self-saves,
  reloads clean buffers and retains dirty buffers for explicit conflict handling.
  A conflict cancels pending autosave; Reload confirms discarding dirty edits,
  while Keep buffer retains them and restores saving. Later edits can schedule
  autosave again.
  In-place writes preserving metadata are not detected.
- SVGs use resvg instead of NanoSVG; their dimensions, scale, assets and straight
  alpha are preserved, but antialiasing can differ. Original Ned artwork is
  retained with attribution. Font licenses accompany copied assets; restricted
  Microsoft emoji and unlicensed VT100 fonts are omitted. Users can supply a
  licensed VT100 font under resources/fonts.
- The CRT port preserves upstream's actual 8-bit targets, overwritten pixelate
  result and one-frame accumulation delay. New temporal targets initialize to
  zero; upstream GL allocation left the initial contents unspecified. The final
  sRGB attachment transfer is canceled to preserve display-referred GL values.
- Native platform adapters preserve upstream formulas and controls through
  winit's owned window. macOS retains WinitView instead of replacing/reparenting
  the content view: winit's delegate relies on that concrete type. A passive
  NSVisualEffectView beneath the GPU layer supplies HUD/BehindWindow blur;
  only the Metal sublayer receives opacity. Winit owns termination/delegate
  behavior, while Bed's close/save path handles native menu Quit/Close.
  Windows uses a retained per-window SetWindowSubclass state instead of the
  original global WndProc pointer. Native caption hit coordinates are physical,
  while ImGui's caption geometry is scaled from winit's logical coordinates.
  Windows hit-test/drawing type checks pass; actual DWM/Snap behavior requires
  native Windows validation. Matching upstream screenshots remain outstanding.
- Muda menus are a requested Bed extension. They install only in the standalone
  macOS host and restore its previous NSApp menu on drop; embedded hosts own
  their menu bars. Per-item native target proxies preserve Muda's original
  target/action and record keyboard origin without replacing a process-global
  event handler. Keyboard Cmd+S retains the original Save+sidebar collision;
  clicking Save performs Save alone. Native menu edits route to the focused
  ImGui/editor/terminal handler; terminal focus disables Undo/Redo/Cut/Select All
  and uses the source's Ctrl+Shift+C/V clipboard shortcuts. Plain terminal
  Ctrl+C/V retain their original raw-control behavior.
- Fonts now select the original FreeType backend with an owned font buffer
  validated by FreeType before atlas loading. Optional `seguiemj.ttf` or
  `Emoji.ttf` merges at the original 70% size with color glyph loading.
  Licensed upstream emoji fonts are bundled; restricted Microsoft emoji is not
  redistributed. ImGui 1.92 loads source glyphs dynamically, so legacy range
  metadata does not exclude Cyrillic or DejaVu's overlapping monochrome emoji.
  Original merge priority is preserved. Exact glyph pixels can differ between
  backend versions. Appending fonts to a host atlas requires its loader to be
  FreeType; incompatible existing loaders are rejected without changing fonts.
  Bed enables ImGui's existing PlutoSVG hooks for the bundled SVG emoji; the
  pinned upstream build enables FreeType but omits those SVG hooks.
- LSP document synchronization keeps upstream's incremental/full/none modes,
  lazy full-text provider, pending-open insertion order and save-presence check
  (including the original `save: false` behavior). Closing a document queued
  before handshake retains that queued open, matching the pinned implementation.
  Diagnostics retain inclusive ends and versionless overwrite behavior. JSON
  protocol text must be valid UTF-8; invalid document/edit bytes return an error
  while the editor buffer keeps its exact bytes. Invalid URI/configuration data
  returns a recoverable error instead of a C++ assertion or uncaught exception.
  Shared Rust diagnostic handles and owned request state replace C++ references.
- The local git adapter retains original HEAD/count timing: opening/editing a
  document updates its cached line diff, while saving does not refresh HEAD.
  Owned HEAD tree IDs replace a retained C++ tree pointer. Raw blob/path bytes
  are retained. The disabled C++ git stub is represented by a runtime enabled
  flag; there are no network git controls or remote-transport dependencies.
- LSP keeps one server per workspace, original configured executable discovery
  (regular files in listed order; no PATH search), original TypeScript/Python
  `--stdio` defaults and logical Ctrl-only shortcut dispatch. ImGui defaults swap
  physical Command/Ctrl on macOS, so logical Ctrl shortcuts use Command there.
  The dashboard
  caches status until explicitly refreshed. Goto uses the shared picker even
  for a single result, shows the original empty label while pending, and passes
  UTF-16 targets to the host after loading. The current single-buffer adapter
  uses the same editor for focused and mouse-hover APIs; separate embedding
  targets remain part of milestone 5. Dashboard path truncation preserves UTF-8
  boundaries instead of splitting a character with C++ byte substring logic.
- JSON-RPC infrastructure has explicit limits: 64 KiB headers, 16 MiB frames,
  1,024 pending requests, eight queued inbound packets, 64 outbound frames and
  a 64 MiB outbound byte budget. Stderr retains its trailing 64 KiB. Invalid
  complete JSON/protocol frames recover up to the original 16 consecutive
  failures; framing errors and EOF stop the session. Numeric IDs/codes and sync
  enums retain finite in-range fractional truncation/custom values; undefined
  out-of-range C++ conversions return errors. Error data uses its protocol
  `error.data` location, correcting upstream's top-level serialization bug.
  Main-thread polling owns callbacks and diagnostic/UI changes. Shutdown waits
  the original 500 ms for its reply, flushes exit within 100 ms, allows 100 ms
  child grace, then kills/reaps and waits at most 100 ms for workers. Workers
  holding descendant-inherited pipes can safely outlive the client with owned
  state. This replaces upstream's EOF spin, blocking destructor and unowned
  detached-thread hazards. Stderr is drained continuously. Explicit stop clears
  process/readiness together so init can restart a stopped server.


- Terminal palette slots and color operations follow the source X11/xterm
  defaults; the fixed bottom panel is transparent for rectangles whose color
  exactly equals its default background. Profile backgrounds show through
  those skipped rectangles. The source has no scrollback buffer: ordinary
  wheel events send Ctrl-Y/Ctrl-E and Shift-wheel sends page sequences.
  Mouse reporting and alternate-screen scrolling retain their original modes.
- Terminal font discovery uses native installed-file candidates and validated
  FreeType collection faces instead of fontconfig; the original four styles,
  regular-only symbol/CJK/emoji merges, TERMINAL_FONT override and metric
  formulas remain. Font sources append before frames and share the host atlas;
  cache invalidation replaces the source's temporary size bump after clearing.
- PTY infrastructure changes only owned child state: working directory,
  environment, session/process groups and signal defaults. Bed leaves the host
  cwd, environment, locale and SIGCHLD handler untouched. Unix children keep
  TERM_PROGRAM=st-imgui; Windows inherits it, matching the original conditional.
  Both use TERM=st-256color, and Windows retains MSYS=enable_pcon. Windows argv
  quoting and environment-key case handling are corrected. Main-thread polling
  parses output even while hidden/minimized so background shells can progress.
- PTY I/O is bounded: each write is at most 1 MiB, queued writes total at most
  4 MiB across 64 commands, and output holds 64 chunks of 16 KiB (plus eight
  intermediate Windows chunks). A poll publishes at most 64 events. Post-exit
  drain and UI shutdown wait at most 500 ms; Unix HUP/TERM grace is 100 ms
  before killing/reaping. Owned cleanup can finish outside the UI after that
  deadline. Queued final output is consumed before the wrapper respawns a shell.
- Translated terminal portions and fixtures retain Neal Mick's Terminal Adapter
  Business Source License 1.1, including its non-commercial additional use
  grant and April 27, 2030 change date; see
  [the copied license](LICENSES/terminal-adapter-BSL-1.1.txt) and NOTICE.

## Dependencies

The manifests of all three Dear ImGui adapters were inspected before selection.
`dear-imgui-wgpu` 0.18 is configured with default features disabled and its
`wgpu-29` backend selected to share wgpu 29 with Bevy 0.19.1. Its Dear ImGui
dependency is `0.18`. `dear-imgui-winit` 0.18 uses winit 0.30.
Cargo.lock contains one compatible version of each GUI dependency. Local Rust
is stable 1.98.1 on macOS arm64.

The windowless Bevy renderer shares the host's instance, adapter, device, queue
and output textures. Device creation uses adapter limits and supported
nonexperimental features, with mappable primary buffers disabled on discrete
GPUs. Bevy's renderer callbacks are restored to the host immediately after
initialization, and captured device loss feeds the existing device-generation
recovery path. SSAO is enabled only when its storage-texture requirements are
available. ImGui retains its original unorm output view while Bevy renders
through an additional sRGB view of the same texture.

| Direct crate | Locked version | Purpose |
| --- | --- | --- |
| `dear-imgui-rs` | 0.18.0 | Context, fonts, custom draw lists and overlay widgets; source build with the FreeType backend |
| `dear-imgui-sys` | 0.18.0, patched path | Enable bundled FreeType in the same compatible native binding; the high-level crate does not forward this feature |
| `dear-imgui-winit` | 0.18.0, patched path | Winit input, focus, cursor, DPI and IME integration plus owned viewport routing |
| `dear-imgui-wgpu` | 0.18.0, patched path | Render ImGui draw data through wgpu 29 with viewport effects/capture hooks |
| `winit` | 0.30.13 | Owned desktop application/event loop and windows |
| `wgpu` | 29.0.4 | Shared host/Bevy GPU device, queue, surface and presentation |
| `bevy` | 0.19.1 | Windowless PBR model renderer sharing the host's GPU and output texture |
| `bevy_stl` | 0.18.0, patched path | ASCII/binary STL loading from document bytes; compatibility patch for Bevy 0.19.1 |
| `arboard` | 3.6.1 | Text-only platform clipboard; default image features disabled |
| `rfd` | 0.17.2 | Native open/save dialogs and unsaved-close prompt |
| `serde_json` | 1.0.151 | Upstream-compatible profiles, keybinds/history and LSP JSON-RPC data |
| `serde` | 1.0.229 | RPC wire types and the LSP JSON visitor retaining duplicate-key rejection |
| `tree-sitter` | 0.26.13, registry | Released native parser and Rust bindings, independent of the original source snapshot |
| `regex` | 1.11.3 | Highlight query predicates and literal byte matching in project search; compatible with the released Tree-sitter runtime |
| `cc` (build) | 1.6.0 | Locate the Windows SDK resource compiler; grammar crates build their own native parsers |
| `resvg` | 0.48.1 | Rasterize original SVG icons; font and raster-image features disabled |
| `git2` | 0.21.0 | Local HEAD baselines, worktree status and gutter markers through bundled libgit2; default HTTPS/SSH features disabled |
| `alacritty_terminal` | 0.26.0 | Terminal grid, VT escape processing, selection and PTY interfaces; default serde feature disabled; Rust1.85 compatible |
| `polling` | 3.11.0 | Portable readiness/wake handling for the PTY worker; version already shared with winit dependencies |
| `unicode-width` | 0.2.2 | Shared Alacritty Unicode width table plus upstream emoji rules, without mutating the host process locale |
| `trash` | 5.2.9 | Native desktop Trash/Recycle Bin for file-tree deletion; std has no equivalent |
| `libc` (Unix only) | 0.2.190 | Unix PTY/process-group/window-size bindings, used on owned worker handles with bounded teardown |
| `windows-sys` (Windows only) | 0.59.0 | ConPTY/process/pipe bindings plus DWM, per-window subclass and native caption hit testing; same family used by Alacritty |
| `muda` (macOS only) | 0.21.0 | Requested native application/menu-bar actions and accelerators; default GTK features disabled |
| `objc2` (macOS only) | 0.6.5 | Typed owned Objective-C objects and native menu/titlebar target callbacks |
| `objc2-foundation` (macOS only) | 0.3.2 | Native object/collection/string/geometry and main-thread bindings for the appearance/menu adapters |
| `objc2-app-kit` (macOS only) | 0.3.2 | Original native material/titlebar controls and per-item menu target/event-origin bridge |
| `objc2-quartz-core` (macOS only) | 0.3.2 | Owned CALayer/CAMetalLayer opacity without changing the titlebar/material layer |

std handles bytes, filesystem I/O, timing and GPU-future waiting. No async
runtime or alternative editor/widget/buffer framework is a direct dependency.
The terminal adapter uses Alacritty 0.26.0; its source compatibility metadata
and GUI/PTY adapters preserve the pinned terminal behavior. Native terminal
acceptance is recorded below as it completes.
Released grammar providers use the runtime's supported ABI range; the cleanup
entry below records their selected versions and compatibility validation.
resvg requires Rust 1.85 and png 1.73,
both compatible with the verified local stable toolchain.

The native font backend uses vendored `dear-imgui-sys` 0.18.0 and
`freetype-sys` 0.23.0 with FreeType 2.13.2, bundled libpng and static zlib
(`libz-sys` 1.1.29). Bed's binding patches select the bundled feature, use
matching target headers and retain native archive linkage; they do not change
the rasterizer. The FreeType License option and original notices are retained.
The exact changes are recorded in `vendor/dear-imgui-sys/BED_PATCHES.md` and
`vendor/freetype-sys/BED_PATCHES.md`. System FreeType/libpng/zlib discovery is
disabled in this build path. ImGui's original SVG glyph hooks use pinned
PlutoSVG 0.0.8 (`fd8a080b3d0bccc41f1ae708d3785e7f08398a58`) and its exact
PlutoVG 1.3.3 submodule (`bbd91f0d06a71491691b36330f29dffa4af87ccf`), built as
static native bindings. Original MIT/stb/FreeType notices remain in the vendor
trees. This supports the original SVG emoji fonts without changing their assets.

The git adapter uses `libgit2-sys` 0.18.8+1.9.7 with vendored libgit2 1.9.7,
reusing the existing static zlib. This replaces the pinned C++ library revision
through a Rust binding; application git algorithms are translated separately.
HTTPS/SSH/OpenSSL features are disabled for the local-only service.

The targeted objc2 family was already compatible with the locked winit/raw
Metal infrastructure; only the adapter's required platform binding features are
enabled. Muda 0.21.0's downloaded Cargo manifest/source confirms objc2 0.6 and
AppKit/Foundation 0.3 compatibility and Rust 1.90 minimum, below the verified
local 1.98.1 toolchain. Its native macOS menu was executed with the real winit
window. Packaging uses Bash/PowerShell and Python 3's standard library, adding
no application crate. License collection retains available resolved Cargo
notices and every vendored grammar/native notice with their path hierarchy.

## Rust embedding

`EditorSession` owns documents and explicitly configured services;
`EditorView` draws an individual custom editor inside the host's existing
container and frame. The former `BedEmbed` workbench facade has been removed.
The host chooses layout, panels, native windows, context, fonts/style, clipboard,
input backends and GPU rendering. No standalone menu or workspace is installed.

Create a session, open/create a document, then create one or more views for its
DocumentId. Each view keeps independent selections, scrolling and transient
find/line-jump state while commands, document history and services remain shared.
Call `view.draw(ui, &mut session, &options)` inside the host frame, and poll
`session.tick()` once per host iteration. Handle returned view actions, session
events and errors explicitly. `session.request_focus(view.id())` lets the host
transfer keyboard focus when its own layout changes.

Default SessionOptions enable no disk/process services and seed no settings.
Consumers can opt into autosave, monitoring, history, Git, highlighting and LSP;
persistent history and LSP require an explicit project root. Close/shutdown use
explicit Save/Discard/Cancel policies. Dropping a view or session performs no
document or history saves. `examples/embed_host.rs` demonstrates two views of
one document with host-owned fonts, style, clipboard, windows and frame/GPU
lifecycle. Document and command APIs contain no window or GPU types.

## Validation

The original core suite was compiled without modifying source using clang++
and the pinned ImGui sources. `scripts/provenance/verify-upstream-model.sh` checks the
recorded commit before building; optional GLFW 3.4 headers are only for original
C++ test compilation and are not a Bed dependency.

For a fresh workspace, retrieve the reference before running upstream tests:

```sh
git clone https://github.com/nealmick/ned.git reference/ned
git -C reference/ned checkout --detach 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff
git -C reference/ned submodule update --init --recursive
mkdir -p reference/deps/GLFW
curl --fail --location https://raw.githubusercontent.com/glfw/glfw/3.4/include/GLFW/glfw3.h --output reference/deps/GLFW/glfw3.h
```

```sh
scripts/provenance/verify-upstream-model.sh
scripts/provenance/verify-upstream-model.sh --with-layer reference/ned/lib/imgui reference/deps
scripts/provenance/verify-upstream-model.sh --with-highlight reference/ned/lib/imgui reference/deps
cargo fmt --all --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --all-targets
cargo run --offline --locked -- --smoke-test --config-dir /tmp/bed-smoke-config README.md
cargo test --offline --locked --lib native_shader_fixtures -- --ignored --nocapture
cargo run --offline --locked -- --lifecycle-smoke --appearance-smoke --effects-smoke \
  --config-dir /tmp/bed-smoke-config . src/main.rs
cargo run --offline --locked -- --effects-smoke --capture-after-frames 30 \
  --capture-frame target/captures/bed-project-stable-retina.ppm \
  --config-dir /tmp/bed-smoke-config . src/main.rs
```

Original core suite: **139 test cases / 1,091 assertions passed** (buffer,
operations, Monaco model, UTF-8, commands, multi-cursor, undo and save). The
`--with-highlight` run also passed all 11 original highlighting cases, for
**150 test cases / 1,225 assertions** across the combined original suite.
All 11 original capture/query/highlight-service cases are translated; differential
capture fixtures were generated with the pinned C++ implementation for every
supported language. Rebuilding those fixtures requires the recorded Tree-sitter
submodules and is automated by `scripts/provenance/update-highlight-fixtures.sh`.
Milestone 2 Rust gate: **231 tests passed** (148 unit, 14 highlighting, 46 model,
11 save/history, 11 disk-monitor integration tests and one view integration test
covering nine original-source fixtures). The native GPU fixture
is ignored by default and was run separately on Metal. Formatting and Clippy with `-D warnings`
passed. The custom-frame regression verifies real ImGui draw vertices,
Unicode input (`hé🙂`), UTF-8 caret movement, requested scrolling, deferred
cursor centering, source focus/input ordering and native glyph/color/run
geometry without a GPU or platform clipboard. Finder/line-jump fixtures verify
Tab focus, Escape/outside dismissal, capped input and Enter suppression.

The original-source view fixtures are regenerated by
`scripts/provenance/update-view-fixtures.sh`. Their fourteen cases compare TextView/Caret/Gutter
measurements and actual draw vertices, packed colors and index order, including
fractional advances, multiple selections/carets, tabs, invalid UTF-8/NUL,
highlight runs, guides, clips, rainbow phases, all four diagnostic severities,
UTF-16 ranges across emoji/tabs, degenerate and multiline ranges, clipped
squiggles, real-repository Git marks and a scrolled four-digit gutter. They
validate those paint leaves; they do not compare full application screenshots,
shaders, production FreeType rasterization, the full Frame layout or tooltip
activation.

The milestone 3 pure LSP partition passes 37 targeted tests: 21 URI,
configuration, request/location and document-sync tests, 10 hover tests, four
diagnostic-store tests and two document-path tests. All 15 original tests for
request state, definition parsing, diagnostics, hover markdown and hover timing
are translated, with additional regressions for UTF-16 synchronization order,
lazy full text, capability save variants, failed sends and stale replies.
Strict library/test Clippy passes. The LSP namespace now passes 37 unit tests,
including eight protocol/client cases and eight additional GUI/parser cases.
Those GUI cases exercise actual ImGui arrows/Enter/Escape, outside dismissal,
pending/empty results, pane clamps, hover rectangles and stale cancellation,
dashboard tables/discovery/cache and editor input blocking through the closing
frame. The five original groups are preserved; some original Catch sections
remain grouped as one Rust test.

Git passes eight unit and 13 temporary-repository integration tests. The six
unchanged original `git_libgit2_test.cpp` cases also passed 22 assertions. The
mock LSP executable requires no installed language server or interpreter; its
19 groups pass framing/fragmentation/batches, UTF-16 edits/save synchronization,
initialization/server requests, main-thread reply/diagnostic ordering, stale
versions, failures/restart, literal argv, reload errors, bounded stderr/shutdown,
descendant pipe ownership and the actual Editor bridge. Its native fixture
answers hover, definition and reference requests and publishes diagnostics.
The milestone 3 gate passed formatting, strict all-target Clippy, the complete
Rust suites and all fourteen unchanged original-source paint fixtures. The
native GPU fixture remains separately verified and ignored by default. The
latest aggregate count, including the terminal implementation, is below.

The milestone 4 package gate passed **360 checks**: 223 unit tests,
11 disk, 13 git, 14 highlighting, 46 model, 11 save/history, two terminal-core,
one terminal-view and one editor-view integration test, plus 19 LSP mock,
13 real-PTY and six original terminal-input harness groups. One separately
verified GPU fixture remains ignored by default. Formatting and strict
all-target Clippy passed. Nine unchanged terminal core fixtures compare every
DrawOp, including colors, real style routing, wide cells, clips/borders and
cursor overlays. The six original input scripts drive actual ImGui events and
a real Bash PTY, checking typing, Backspace/arrows, Unicode, clipboard paste
and Ctrl-C. Eight canvas/pure tests verify focus, reporting, selection, raw
bracketed paste, keyboard lock, modifier routing and blink/color ordering;
wrapper tests verify stable tab IDs, close ordering, font resync and final
DSR-query output/exit draining without dropping following text.
Two native terminal-font tests verify distinct Menlo collection faces, base
braille/CJK/emoji fallbacks and override/host-atlas preservation. Thirteen PTY
groups exercise actual shell I/O, resize, signals, final-output drain, writes,
working directory, environment, backpressure and shutdown. PTY source passes
strict isolated cross-target Clippy for Linux and Windows; those native runtime
checks are pending. The stronger unchanged C++ terminal-state fixture now
passes all 12 cases and 54 transitions, including the source tab/wrap/resize,
OSC, malformed color and default cursor-style behaviors it exposed.

The terminal desktop passed 60 native Metal frames at 2400×1600, Retina scale
2, with a real project-directory zsh prompt, fixed shell tabs and clean exit.
A second 60-frame run passed CRT effects, font reload, resize to 1680×1200,
minimize/restore and focus changes; its GPU readback was inspected. Linux and
Windows native terminal execution and matching whole-app screenshots remain
pending.

The macOS native appearance/menu adapter passed 30 Metal frames with resize
from 2400×1600 to 1680×1200, Retina scale 2, minimize/restore, focus changes and
clean exit. The live AppKit checks retained WinitView identity, dispatched all
three native accessory controls, changed/restored sidebar/terminal/settings,
verified Metal-only opacity 0.35 and disabled/restored blur, and confirmed a
32-point native caption inset. NSMenu's actual Find target/action reached the
inline finder. Native ImGui edit-routing tests prove Cmd+Z/Shift+Cmd+Z undo/redo
once without inserting characters and protect terminal edit ownership.
The terminal-canvas native menu regression verifies Copy updates its selected
text clipboard without sending Ctrl-C and Paste sends exact raw bracketed
content through the input adapter.
The dedicated `--menu-smoke` creates/removes a temporary project/config, types
through ImGui, waits for real autosave, dispatches actual NSMenu Undo/Redo and
asserts exact buffer and disk bytes after each operation. Its 30-frame Metal
run passed at 2400×1600.
The same isolated fixture also passed a locally queued AppKit Cmd+S event:
the per-item proxy classified keyboard origin, original Save+sidebar behavior
ran once, mouse Save retained sidebar state, and no accelerator characters
entered the document. Events stay in that application's own event queue.
The Windows adapter passes strict isolated cross-target
Clippy against real pinned winit/ImGui/Win32 types, with unrelated Settings/Icon
interfaces shimmed; native linking and DWM/Snap interaction remain pending.

The macOS `.app` packaging script passed resource verification, plist lint,
system-only dylib inspection and strict ad-hoc signature verification. Bundles
include config, all redistributable fonts/icons, query files and collected
Cargo/native/grammar notices. Debian and Windows scripts are configured and
syntax/review checks pass where available; their native execution is pending.
The final optimized, signed `Bed.app` also ran from `/private/tmp`, resolving
its bundled resources and passing the 30-frame native-menu/autosave/Undo/Redo
and local AppKit save-shortcut fixture. Its license index records 330 resolved
Cargo packages alongside the native and grammar notices.

Five native font fixtures passed: bounded invalid-font rejection and every
bundled font, original source ordering/sizes and dynamic range behavior,
braille/monochrome smiley priority and a colored SVG rocket glyph drawn through
ImGui, the original missing-main-font fallback without braille/emoji merges,
plus appending to a FreeType host atlas and rejecting an incompatible stb host
atlas without mutation.

Embedding passes seven native ImGui lifecycle tests: unchanged host frames and
style, drawing before/after Bed, retained host-font handles, compatible font
reloads, mismatched/live/replaced context rejection, between-frame validation,
explicit configuration paths and repeat cleanup/reinitialization. Four
Workbench module tests cover canonical duplicate opens, independent documents
and view/history state, failed-close preservation, monotonically increasing
bootstrap IDs, native split docking into the active group and compaction after
the central pane closes. Shared Git-enable changes also propagate to inactive
tabs without reloading host fonts. Four additional native-frame tests check tab-shortcut
Unicode routing, inactive selections, settings input blocking/multi-caret
retention, project-search typing/Escape/focus restoration and restoration of
the host's dashboard padding/border.

Five native ImGui Settings tests check the original centered fullscreen modal,
uploaded close-icon draw command/hover tint/click persistence, macOS opacity
and blur input with immediate persistence/apply, host-palette retention,
embedded title-bar dragging/resize/reopen, and the original distinction between
title-bar X versus Escape/background save-and-unblock. The header case also
verifies that old themes gain all fourteen source color slots while the picker
is collapsed. All tests run the actual native widgets and validate balanced
style/window stacks; they are added acceptance tests because upstream has no
Settings UI test suite.

Five shared-store tests verify file-stack independence across full Editors,
history after a tab closes/reopens, one JSON file containing both tabs, one
project load and targeted external-reload invalidation. The named-document
autosave regression checks standalone and shared stores: automatic saving
retains undo, autosaving the undo retains redo, and both restore caret/disk
bytes. These use real ImGui/docking and filesystem services; they do not
exercise every native dock-drag gesture or compare upstream whole-window
screenshots. Linux/Windows native embedding checks remain outstanding.

The runnable `examples/embed_host.rs` passed 16 actual Metal frames at
2800×2000, Retina scale 2, with two Bed tabs alongside host controls. The example
owns winit/wgpu, uploads Bed assets, draws host widgets before/after Bed, checks
its style/font/frame ownership, reloads Bed fonts and opens/closes Settings.
Its GPU capture was inspected and it exited cleanly. The same host example is
included in native three-platform CI acceptance; no C++ ABI is provided.

The final macOS package gate passed **395 checks**: 241 unit tests, 116 regular
integration cases, 19 mock-LSP groups, 13 real-PTY groups and six unchanged
terminal-input scripts. Formatting, strict all-target Clippy and the optimized
`bed`/`embed_host` build passed. The separately run ignored GPU fixture also
passed on Metal. The reference checkout remains unchanged at the recorded pin.

The final optimized build also passed three separate desktop runs: native
menu/titlebar/material plus lifecycle (30 frames), appearance/font/CRT plus
lifecycle (60 frames), and the updated host-owned embedding example (16 frames,
widgets before and after Bed). The standalone runs resized from 2400×1600 to
1680×1200; the host ran at 2800×2000. All used Metal/Retina scale 2 and exited
cleanly. Appearance and platform fixtures run separately to preserve each
fixture's original-state assertions.

Native macOS arm64 checks passed through the actual Metal surface and renderer:
`Bgra8UnormSrgb`, **2400×1600 pixels**, **Retina scale 2**, with clean shutdown.
The following checks were performed:

- The original two-frame smoke rendered the custom editor. Testing found and
  fixed style-stack ordering and reentrant draw-wrapper ownership errors.
- Lifecycle smoke requested and observed a native resize to **1680×1200**, then
  minimized, observed focus loss, restored, regained focus and resumed rendering.
  The zero-sized configuration guard passed. The first lifecycle/capture run
  completed seven frames; combining appearance changes completed eight frames.
- Appearance smoke reloaded JetBrains Mono at 24 logical pixels, enabled
  Tree-sitter and hid the sidebar, applied the Amber profile, then restored the
  original settings. Managed font-atlas clearing/rebuilding worked with the
  active renderer. Smoke changes do not replace the user's saved profile.
- The final FreeType/PlutoSVG build passed the combined appearance and native
  lifecycle run. A fresh 2400×1600 Metal capture shows the colored rocket,
  DejaVu monochrome smiley, braille and Cyrillic in the editor.
- Enabled CRT rendering passed with offscreen ImGui, two temporal targets and
  final presentation. The ignored `native_shader_fixtures` test was explicitly
  run on Metal: disabled passthrough orientation/alpha, original ineffective
  pixel-width setting, accumulation max/`pow(decay,4)`, one-frame presentation
  lag, resize/history reset, bloom/vignette/scanline/pulse/grid order and sRGB
  transfer all passed deterministic readback assertions.
- The GPU uploaded all 194 decoded/rasterized icon images and the five welcome
  images. Welcome, project tree, syntax colors and minimap were captured and
  inspected. A stable project capture waited 30 frames; the appearance capture
  shows the expected burn-in ghosts from its deliberate font/profile changes.
- Final layout rendering passed 30 native Metal frames at 2400×1600, Retina
  scale 2, with clean shutdown and a GPU readback capture.
- An isolated temporary document was changed after GPU readiness. The host
  detected and reloaded the clean buffer, logged the reload and captured the
  new `after_disk_reload` contents after 90 frames. This validates the live
  host/monitor integration in addition to filesystem fixture tests.
- The local git service rendered `+2 -1` counts, changed-row gutter marks and
  modified-file emphasis in a temporary repository; the 2400×1600 Metal capture
  was inspected. The deterministic LSP server initialized and synchronized a
  document during 30 Metal frames. Its severity-one gutter marker and UTF-16
  diagnostic squiggle rendered in the inspected Retina capture. These native
  checks supplement the process/UI fixtures; exhaustive real-server interaction
  and matching upstream application screenshots remain outstanding.

Screenshots come from GPU readback of Bed's render target, with padded rows and
BGRA/RGBA conversion verified by unit tests. PPM files use std I/O; PNG copies
were encoded locally for inspection. Local generated artifacts include
stable project (historical generated capture),
welcome (historical generated capture),
lifecycle/appearance (historical generated capture) and
external reload (historical generated capture). The final backend is shown
in FreeType/color emoji (historical generated capture).
The latest layout is shown in editor layout (historical generated capture).
Stage 3 captures show git markers (historical generated capture) and
LSP diagnostics (historical generated capture).
Stage 4 captures show the terminal panel (historical generated capture)
and terminal CRT/lifecycle run (historical generated capture).
Stage 5 captures show the native-menu layout (historical generated capture)
and autosave/menu undo fixture (historical generated capture), plus the
final packaged release (historical generated capture),
final platform/lifecycle run (historical generated capture),
final CRT/lifecycle run (historical generated capture) and
host-owned embedding window (historical generated capture). GPU readback
captures Bed's GPU content; it does not capture the OS menu/titlebar or composite
blur. Those native properties/actions are checked programmatically.
They are verification artifacts, not bundled application resources. Earlier
project/welcome/reload captures predate the final FreeType/PlutoSVG backend
conversion. The macOS `screencapture` attempt could not create a window image;
GPU readback requires no Screen Recording permission. OS permissions were not changed.

Surface reconfiguration waits until no acquired frame is alive. Lost-surface
and lost-device recovery paths rebuild resources and reupload image IDs, but
injected device/surface failures, exhaustive native input/clipboard/dialog
gestures and matching upstream screenshots remain unverified.

The CI workflow builds/tests on macOS, Linux and Windows, enforces formatting
and Clippy, builds release packages and runs GPU/lifecycle and embedding-host
fixtures (Xvfb/Openbox on Linux). Mac jobs also exercise the native menu's
autosave/Undo/Redo fixture.
CI has been configured but has not run here. Native visual/input
checks on Linux/Windows and screenshot comparisons remain outstanding. Full
parity must not be declared until those checks and every remaining stage pass.

## Remaining acceptance

The current implementation and macOS validation are recorded in the dockable
workspace revision below. Native Linux/Windows builds, packaged launches,
host-owned embedding, input/clipboard/dialog gestures and viewport recovery
must pass their CI/desktop checks before full desktop acceptance. Native
undocking on pure Wayland is unsupported by the pinned backend; internal docking
and floating panels remain available there. Configured CI is not evidence that
these platform checks ran.

Matching upstream screenshots across all three platforms remain useful for the
retained custom editor, highlighting and Legacy effects. The project picker,
Sharp defaults and application layout deliberately diverge from Ned. The pinned
upstream implements neither completion nor a document/workspace-symbol browser;
these features are outside this port's implemented LSP scope.

## Resolved execution-environment blocker

Initial sandboxed Git/Cargo commands reported DNS failures. The user's terminal
worked. Running the same Cargo search with approved `require_escalated` access
succeeded, and Git clone/dependency retrieval succeeded through the same route.
The cause was sandbox network restrictions, not a machine-wide network outage.
Sandboxed native desktop launches also require access to OS desktop services;
use the approved execution path for native smoke verification when needed.


## Dockable workspace revision — 2026-10-05

The user approved relaxing application parity, retaining the custom editor and
observable document behavior while simplifying layout and embedding. This
revision supersedes the earlier single-buffer standalone/full-workbench embed
scope. No InputTextMultiline or replacement buffer was introduced.

- `editor_session.rs` owns stable DocumentId/ViewId/WorkspaceId identities and a
  single Editor/document-service aggregate per canonical file. Existing commands,
  operations, state and persistent buffer remain unchanged in algorithm. Scoped
  view lending preserves their boundaries; splice records transform sibling
  carets while undo/history stays shared. Service polling is independent of draw.
- `editor_view.rs` draws the same custom editor in a host-selected container.
  Per-view selection/scroll/find/line-jump state and weak event subscriptions
  survive tab moves. It creates no outer windows, font/style configuration,
  native menu, framebuffer or application workspace. No destructor saves.
- `workbench.rs` owns document/tool/terminal tabs, stock docking layout and native
  window group closure. The workspace root stays on the main viewport; ordinary
  tabs can dock anywhere or detach. New views share a document; final-view close,
  project switch and quit preflight saves before destroying any panel/PTY. A
  failed save or cancelled unnamed Save As keeps the group open. Shutdown is
  idempotent so saved layouts are not replaced by the cleared panel registry.
- `workspace_state.rs` atomically stores canonical recent folders and per-project
  ImGui ini plus tab IDs, paths, selections/scroll and terminal cwd. Startup uses
  the picker, not silent last-project reopening. Missing saved files are skipped.
  Restored terminals start fresh shells in captured launch directories; live
  processes, scrollback and shell directory changes are not persisted.
- `file_actions.rs` creates without overwrite, validates entry names/project
  boundaries, renames atomically without replacement (including case-only aliases)
  and uses OS Trash. Symlinks affect the entry. Successful renames rebind history,
  monitoring, highlighting, Git and LSP once per document; successful Trash
  cancels autosave and keeps the buffer for explicit recovery/Save As.
- FileFinder scans now carry generations and rescan on same-root filesystem
  changes, rejecting stale results. Content Search has a controls-only dock body.
- Settings has a dock body and global Sharp/Off/Legacy/Custom effect preset. Sharp
  uses scanlines 0.06, vignette 0.04, bloom 0.025 and zero disruptive/temporal
  effects; existing JSON effect values and unrelated theme/font values remain
  intact. WGSL uses physical texture dimensions. Zero burn-in bypasses history;
  enabled history is allocated lazily per native surface, with current-frame
  ordering and explicit reset on resize/settings/layout changes.
- `workspace_lsp.rs` owns one server per language per workspace. Resolver/startup
  preserves arguments, inherited environment and project cwd, probes rustup
  shims, records bounded stderr/process failures, and exposes explicit retry.
  Document lifecycle/snapshots are registered once, independently of view count.
  Responses carry workspace/server/document generation, version, view and ticket;
  late results are rejected after edit, reload, close or restart. Dashboard and
  navigation results draw in ordinary tool tabs; text context actions capture the
  clicked view and preserve an existing selection when clicked inside it.
- `BedTerminal::new_empty/new_session_at/render_session/close_session_id` supplies
  ordinary panel bodies with stable session IDs and captured cwd. Closing the
  last terminal creates none. Ended sessions retain final cells and require an
  explicit Restart; legacy panel methods remain for translated fixtures.

New direct dependency: `trash = 5.2.9`, native desktop Trash/Recycle Bin semantics;
std has no equivalent. Its locked transitive packages are recorded in Cargo.lock.
The existing ImGui/winit/wgpu dependency family remains 0.18/0.30/30. Local
`vendor/dear-imgui-winit` and `vendor/dear-imgui-wgpu` are exact 0.18 baselines with
small documented extensions: owned viewport window enumeration for event/focus/
close routing and a per-viewport offscreen postprocess hook with independent
surface output format. Stock callback/texture/submission lifecycle remains in
charge. A native shutdown RefCell borrow fix drops the borrowed runtime slot
before teardown clears it. Both crates preserve MIT/Apache notices and have
BED_PATCHES.md; backend unit/strict-lint verification is recorded with validation.
Native detachment is unavailable on Wayland in this pinned backend; internal
floating/docking still works. macOS native Muda menus expose panel commands;
embedding installs no menus.

Validation on macOS arm64, 2026-10-06:

- `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`
  and `cargo test --locked --all-targets` pass on the frozen implementation.
  This includes 270 library tests, 20 Session tests, five embedding tests, seven
  workspace-focus tests, 27 deterministic LSP transport groups and original
  terminal model/input/PTY fixtures. Tests cover UTF-8 multi-view navigation,
  shared undo, failed Save As identity/autosave rollback, disk conflicts after
  monitoring activation, context-menu selection/clipboard, file-tree dispatch,
  guarded reload, rename/Trash failure and workspace restore/group closure.
- Vendored backend suites pass (wgpu: 92; winit: 135), with strict Clippy.
- The opt-in native shader fixtures pass on Metal. A real installed
  rust-analyzer resolves hover and definition in Bed's Cargo workspace; this
  opt-in test requires separately installed rust-analyzer and rust-src.
- `cargo build --locked --release --bin bed --example embed_host` passes.
  The release binary's Metal detached-window smoke renders 30 main frames and
  10 independent secondary presentations, verifies Retina scale 2, resize,
  focus, actual main-window minimization/restoration and group closure. Final
  main and secondary GPU readbacks are `target/captures/bed-release-viewports.ppm`
  and `target/captures/bed-release-viewports-secondary-1.ppm`.
- Native Muda menu typing/autosave/Undo/Redo/keyboard Save passes in the release
  build. Native Settings/appearance, project picker, main-only renderer fallback
  and host-owned embedding also pass; the host example renders two shared
  document views for 16 Metal frames. Sharp text, syntax colors, Unicode/emoji
  and detached-view gutters were inspected in GPU captures.
- `scripts/pack-mac.sh` produces `target/dist/Bed.app` and
  `target/dist/Bed-0.1.0-arm64.zip`. Resource/license checks, system-only dylib
  inspection, Info.plist validation and strict ad-hoc signature verification
  pass. The packaged binary passes 30 Metal frames of native resize, actual
  minimize/restore, focus and menu/titlebar/material checks while loading its
  bundled resources. This local package is ad-hoc signed, not notarized.

Linux and Windows runtime checks must pass native CI before full desktop
acceptance is declared; configuring CI is not proof of a run.

## Rust LSP follow-up — 2026-10-06

The standalone text context menu had its ImGui `selected` and `enabled`
arguments reversed. Initialized language actions appeared checked and were
always disabled. The call now supplies `selected=false, enabled=ready`; a native
mouse regression clicks each of Definition, References and Symbol Info when
ready and verifies each stays disabled before readiness. The analogous legacy
Terminal menu argument ordering was corrected.

The missing-brace report was exercised through the complete Session lifecycle
with the user's existing LSP configuration. A small isolated Cargo project
reports errors after edit/save and clears them after undo/save. Bed's real Cargo
workspace reports an unsaved missing-brace error after 67–126 seconds of
workspace analysis in local checks; source files and user configuration stay
unchanged. The duration varies with analysis/build work and is not a timeout.
Initialization alone does not mean workspace analysis has completed. Standard
LSP work-done progress is now requested, tracked on the polling thread and shown
above the document, in the context menu and in the Language Servers tab. Active
and recent completed phases are bounded and reset on disconnect/restart.

A deterministic mock regression verifies UTF-16 edits, one save notification,
main-thread diagnostic publication and error squiggles/gutter markers in two
shared custom views, then verifies undo/save clears both views. Actual Metal
rendering of the missing-brace Cargo fixture shows the red gutter, underline and
rust-analyzer/rustc messages in `target/captures/bed-real-rust-diagnostics.png`.

## Panel creation, spacing and definition navigation — 2026-10-06

Standalone native titlebar buttons and panel menu clicks create additional
instances; configured keyboard shortcuts continue revealing existing tools.
`workbench.rs` chooses the largest live leaf dock rectangle for new panels and
newly opened files, including an empty central leaf and populated floating dock
groups. Bare undocked windows are not tab groups. Geometry reads bind the
workspace's weak ImGui context and restore the host's current context. A thin
null-safe dock-node ID accessor in the existing dear-imgui-sys shim avoids
copying ImGui's opaque internal layout; all placement policy stays in Rust.
Default initial/reset layout remains a narrow Files group and a bottom terminal.
Search workers, queries and results now belong to each Search tab, including
restored stable tab IDs. Explicit Projects tabs stay open when a file opens.

Document panel padding is zero, Files padding is two logical pixels, and
`gutter_view.rs` reserves the actual line-number digits rather than at least
three. The gutter child has zero padding like the text canvas. This deliberately
changes upstream spacing; source paint fixtures still compare unchanged glyph
and marker output using their recorded gutter width. Shared popup styling adds
eight-by-six logical pixels of padding and six-by-four item spacing to text,
file-tree and recent-project context menus without changing docked panel or
embedding host styles. The native dock tab-list popup also receives this style
inside dockspace submission, where the workspace root otherwise has no padding.

Definition requests wait for their current result without opening References.
A single destination navigates directly in the requesting view; multiple/empty
results use the existing navigation list. Cmd-click on macOS (Ctrl-click on
other platforms) emits an explicit byte-position request from EditorView,
without moving the caret or disturbing selections. The LSP adapter converts
that position to UTF-16 and retains document/view/version/generation routing;
stale replies after edits or closure remain rejected. Hosts can handle the
typed request in ViewResponse and choose their own navigation/layout policy.
The keyboard LSP dispatcher now accepts the editor's Cmd/Ctrl modifiers.

Bed's application artwork uses the supplied orange-to-pink bed icon from
`resources/bEd.icon`, exported as `resources/bEd-iOS-Default-1024@1x.png`.
`resources/icons/bed.png` preserves that 1024px export, including its Display
P3 profile and transparency. `bed.icns` includes macOS standard and Retina
sizes generated with sips and iconutil; `bed.ico` packages its PNG frames
using `scripts/build-windows-icon.py`. macOS, Windows and Debian packaging
use these assets. The previous gold-b concept and prompt remain in
`resources/icons/bed-icon-concept.png` and `bed-icon-concept.txt`.

Follow-up validation on macOS arm64:

- `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`
  and `cargo test --locked --all-targets` pass. The final suite includes 277
  passing library tests, 32 deterministic LSP transport/navigation groups,
  14 native workspace-focus cases, five embedding cases and the original
  document, view-paint and terminal fixtures. New checks cover largest-group
  placement after resize and in floating groups, repeated stable panel IDs,
  context ownership, independent Search workers/restoration, actual context
  menu padding/actions, direct definition navigation, UTF-16 Cmd-click targets
  and rejection of edited/closed-view replies. The opt-in real rust-analyzer
  checks described above were run separately; they remain ignored by default.
- `cargo build --locked --release --bin bed --example embed_host` passes.
  Repeated native titlebar controls and Window items create exactly three
  additional Files, Terminal and Settings tabs each. The release Metal smoke
  verifies Retina scale 2, resize, native minimize/restore, focus recovery and
  redraw. Its CLI test window required explicit activation of only its own
  process to obtain initial focus. It rendered 478 frames and closed cleanly;
  `target/native-release-panels.log` and
  `target/captures/bed-release-multiple-panels.png` record the result. A debug capture
  with a pristine fixture is `target/captures/bed-native-multiple-panels.png`.
- Detached shared views retain the compact gutter and render through their
  independent effects pipeline while the main window is minimized. The debug
  Metal check rendered 30 main frames and six secondary presentations;
  `target/captures/bed-compact-shared-viewports-secondary-1.png` records the view.
  Actual Rust work-done progress appears in the release GPU capture
  `target/captures/bed-rust-progress-retina.png`.
- The local `Bed.app` and arm64 ZIP are rebuilt with the final release binary.
  Packaging retains notices for 335 resolved Cargo packages and passes resource,
  system-dylib, Info.plist and strict ad-hoc signature checks.

No new direct dependencies were required for these follow-ups. Native Linux and
Windows acceptance remains pending CI execution.

## Toolbar and project-search performance — 2026-10-06

The standalone toolbar has seven contiguous, equally sized controls: Files,
Terminal, Settings, Search, Diagnostics, Split Right and Split Down. macOS uses
native SF Symbols with fallback symbol names; Windows uses vector drawing and the
existing bundled Settings icon. The old extra Settings spacer is removed. Both
platforms send workspace commands rather than mutating panel visibility settings.
Split commands use the active document's dock leaf even when Files or another
tool has focus; each split creates another view of the shared document.
Both source and duplicate document tabs are revealed, with focus in the new
view, even when a tool previously covered the source in its dock group.
A null-safe tab-bar accessor in the existing native shim lets Rust queue source
selection independently of keyboard focus; ImGui's focus call alone defers tab
selection until a later frame, by which time the duplicate owns focus.
The translated inline Find UI now scopes autofocus and Enter/Escape to its
owning view and avoids returning focus after an outside click in another pane.
This deliberately replaces upstream's single-editor global focus assumption;
matching, replacement and undo behavior remain unchanged. The split regression
keeps Find open in a source covered by Diagnostics, focuses Files, and verifies
both split directions reveal the source while keeping the duplicate active.

Project Search has a separate cancellable walker in `files/search_files.rs`.
The original FileFinder keeps its full-tree inclusion behavior. Search defaults
to libgit2 ignore rules, including parent-worktree, nested and negated rules,
and always excludes `.git` metadata. Non-Git folders include all regular files;
Include ignored files bypasses ignore filtering. This is an intentional Bed
search-scope change. No directories are excluded merely by names such as
`target`, `vendor` or `reference`.

Search reuses `EditorState::split_lines` instead of constructing an editable
buffer for every file. A byte-literal matcher from the existing `regex`
dependency preserves overlapping matches, ASCII-only folding, byte columns,
BOM/newline handling and truncated-file editor positions. The existing capped
reader rejects binary data after its unchanged 1KiB probe before reading the
remainder. Generation-tagged batches use a bounded channel; traversal, matching
and backpressure remain cancellable. The UI shows discovery/scanning progress,
partial matches and Cancel, with clipped result rendering. Queries stop at
100,000 matches and report the limit rather than allocating unbounded results.
No new direct dependencies or document-buffer algorithms were introduced.

Independent release measurements on this Bed repository used the same no-match
query before and after the change: 38.1415 seconds became 1.2268 seconds (about
31 times faster). A fresh `EditorSession` comparison found exactly the same 69
file/line/byte-column positions as `rg`, in 1.1655 seconds. Cancel plus worker
join after 100ms fell from 9.0818 seconds to 0.0003 seconds. Reproducible probes
are in `target/search-benchmark.rs` and `target/search-match-compare.py`; timings
are local measurements rather than cross-platform performance guarantees.

Validation on macOS arm64:

- Formatting, strict all-target Clippy and the full all-target test suite pass:
  290 library tests, 15 workspace-focus tests and the existing document, LSP,
  embedding and terminal fixtures. The opt-in shader/real-server checks retain
  their existing ignored status. New fixtures cover ignore rules, parent roots,
  cancellation, bounded batches/limits, byte-match parity and early binary reads.
- The Windows portable toolbar tests measure all seven buttons, exercise every
  click action, and verify client-relative hit areas at a nonzero desktop origin
  with 2x DPI. Native Windows execution remains pending.
- Release library, binary and embedding example build. The final live Metal smoke
  runs 12 Retina frames, verifies all native images and exact 26-by-22-point
  buttons with two-point gaps, creates three instances of each of the five tools,
  and checks both split directions against actual dock-leaf geometry. A native
  button subclass removes AppKit's differing optical alignment insets so equal
  constraints produce equal real frames. The final run checks both source-tab
  visibility and duplicate focus with Find left open. macOS monitor enumeration
  temporarily returned no displays, so this final run uses main-window-only
  smoke mode without capture and exits cleanly. Its log is
  `target/native-toolbar-search-final.log`. An earlier 30-frame GPU-content
  capture, before the source-tab/Find focus correction, is
  bed-toolbar-panels.png (historical generated capture).

## Crate extraction (2026-10-06)

Bed is now a nine-package workspace; the root package remains the desktop app.
Existing third-party versions, patches, upstream pins and native sources are
unchanged. Production dependency boundaries are checked by
`scripts/check-crate-boundaries.py` using Cargo metadata, including transitive
GUI dependencies and internal crate edges. Vendored packages stay outside the
Bed workspace, including Tree-sitter's own workspace.

| Package | Existing direct dependencies and purpose |
| --- | --- |
| bed-core | serde_json: original history serialization |
| bed-files | bed-core: byte/line conventions; git2: ignored-file discovery; regex: literal byte search |
| bed-highlight | bed-core: snapshots/edits; tree-sitter and grammar crates: parsing; regex: query predicates; serde_json: theme conversion |
| bed-lsp | bed-core: IDs, changes, diagnostic data and path/index helpers; serde/serde_json: JSON-RPC; libc (Unix): owned process cleanup |
| bed-session | core/files/highlight/lsp: shared document services; git2: Git baselines; serde_json: history/service configuration |
| bed-ui | core/session/highlight/lsp: custom document widgets and LSP presentation; dear-imgui-rs: drawing/input; dear-imgui-sys: bundled FreeType feature forwarding; serde_json: hover/navigation payloads |
| bed-terminal | dear-imgui-rs/sys: custom terminal drawing/fonts and bundled FreeType; alacritty_terminal: grid/PTY interfaces; polling: worker readiness; unicode-width: character widths; libc/windows-sys: owned PTY processes |
| bed-effects | wgpu: effect passes; dear-imgui-wgpu: viewport postprocessing; serde_json: existing effect settings conversion |
| bed | Internal crates plus existing native GUI, clipboard/dialog, icon rasterization, Trash, and platform adapters |

No new third-party dependency was added. PNG decoding was unused after the
project picker replacement and its direct dependency has been removed.
UI and terminal declare the existing sys dependency to preserve bundled
FreeType when either crate is built without the desktop application's manifest.
Terminal manifest licensing points at its retained BSL notice; original notices
and native/grammar/font attribution remain included in release packages.

The document UI no longer owns a second application shell, Files/Search workers,
welcome textures, terminals, native dialogs, native menus or GPU resources.
Workbench remains the single desktop composition root. Explorer presentation
returns actions; bounded reads and monitoring belong to the filesystem/session
crates. Legacy single-buffer lifecycle tests were migrated to EditorSession,
including shared history and external disk changes. Session reload notifications
and view-scroll preservation replace obsolete explorer lifecycle notifications.
No text indexing, editing, undo, save, search or terminal algorithm was replaced.

The public embedding imports are now `bed_session` and `bed_ui`, with no old
`bed_embed` or root embedding facade. `ViewContext` provides commands, view-state
mutation and event subscriptions while withholding mutable document/service
ownership. The existing panic-safe swap/restore and sibling-edit transformation
remain in the session. A new integration regression exercises a panicking
public view scope, restored scrolling, event attribution and sibling undo.
Read-only `ViewPresentation` replaces Workbench's mutable frame access, and
LSP widgets accept explicit appearance/shortcut options instead of Settings.
Document clipboard operations use the consumer's ImGui clipboard backend.
Runtime SessionOptions remain supported; no Cargo service feature matrix was added.

Unused title-strip rendering and redundant module/test-support copies were
also removed. Workbench tabs and native chrome own the current title display.
Generated capture history was removed from the source distribution and is
ignored; future captures belong under target. Local document history and
imgui.ini were preserved and ignored. Differential fixtures, fixture generators,
patched native vendors and upstream/license records remain. Test suites now
live in their owning crates; original fixtures and test adapters stay under the
workspace tests directory without duplication. Packaging retains root resources
and excludes all workspace packages from third-party dependency notice scans.

The migrated disk-change regression exposed a pre-existing session gap: failure
to reload a clean buffer from a binary external replacement returned a monitor
error without retaining a conflict. Session monitoring now retains that conflict
and cancels pending autosave, matching the retired explorer's preservation
policy. The binary and oversized external-change regressions verify the buffer
and generation stay intact and later explicit/idle saves cannot overwrite the
external file until the consumer resolves the conflict.

Validation on macOS arm64:

- `cargo fmt --all --check` and strict workspace/all-target Clippy pass.
- `cargo test --workspace --locked --all-targets --no-fail-fast --offline`
  passes: 431 Rust harness tests, 32 LSP transport cases, six original terminal
  input fixtures and 13 PTY integration groups. The three installed-server
  rust-analyzer tests retain their opt-in status. Workspace rustdoc checks pass.
- `python3 scripts/check-crate-boundaries.py --offline` passes. A separate
  five-crate headless `cargo check --lib` passes without the desktop package.
  All 335 external package versions, sources and checksums match the baseline
  lockfile. The boundary check also runs in CI and can fetch metadata on a fresh
  runner; local cached builds can use `--offline`.
- Release desktop and embedding-example builds pass. The native Metal shader
  readback fixture passes outside the sandbox, where the GPU is accessible.
- Native desktop lifecycle, macOS toolbar/menu/material, shared-document menu
  typing/autosave/Undo/Redo/Save, and detached-window resize/focus/minimize/group
  close checks pass. Both primary and secondary GPU captures were produced.
  The embedding example passes 16 native GPU frames with two views, shared
  text/undo and preserved host ownership. Its first attempt timed out; the
  retry passed without changing the render loop.
- `scripts/pack-mac.sh` produces an ad-hoc signed and verified app/archive with
  runtime resources and notices for all 335 external packages. Captures are
  under `target/crate-extraction-smoke/captures`, and packages under `target/dist`.
  Native Linux/Windows acceptance remains assigned to the configured CI jobs;
  those platforms were not executed locally.

## SSH projects and named workspaces — 2026-10-06

Bed now has eleven workspace packages. The new `bed-headless` binary runs remote
filesystem, search and Git services without ImGui, winit or wgpu. `bed-remote`
owns the versioned length-prefixed JSON protocol, target identity, literal POSIX
argument quoting, bounded frames, request IDs, structured errors and a cloneable
system-SSH transport. The desktop keeps the original custom document engine,
commands, undo, selections and Tree-sitter highlighting locally. Existing local
session/save implementations remain in place to preserve translated behavior;
remote sessions explicitly opt into asynchronous service adapters.

The upstream save implementation and tests, Git service, terminal wrapper,
finder, diagnostics and editor finder implementations were inspected before
extending the corresponding translated Rust boundaries. No upstream buffer or
editing algorithm is replaced. The new remote/headless/workspace code is a Bed
extension, not an additional upstream module conversion.

- `bed-session::EditorSession::with_remote_options` takes explicit target/client
  configuration and an agent-validated root. `request_open_file` delivers remote
  completion through session events; `save_pending` distinguishes queued writes
  from clean no-ops. Remote save snapshots preserve BOM and byte line endings,
  commit path changes only after success, and never clear newer edits. Explicit
  reload/keep operations serialize against saves. Stale generations cannot apply
  old-path errors to a renamed document. Metadata-only changes preserve undo.
- Filesystem requests validate paths on the target and reject escaping the root;
  root rename/removal is forbidden. Saves check content/length/mtime baselines,
  write a sibling temporary file, and commit after a second check. New filenames
  and renames cannot overwrite entries that appear concurrently. Remote explorer,
  file finder, Git HEAD/status and search use target-native paths and background
  queues. The headless search dispatcher reuses Bed's existing Git-aware byte
  search, ignored-files policy, result limits and source/editor row accounting.
- LSP runs remotely through `bed-headless exec`, with executable candidates
  resolved on the target and local document changes synchronized over its stdio
  stream. URI/diagnostic paths remain target-native and the local PID is omitted
  from initialization. Terminal shells run through SSH with a remote PTY while
  the existing local emulator/rendering stays intact. Their initial dimensions,
  resize and control input retain the original transport semantics. LSP and
  terminal streams use separate SSH child processes rather than multiplexing
  interactive output into the filesystem RPC stream.
- `workspaces.json` v2 stores named single-root local/SSH workspaces, recent
  identities and per-workspace layouts. Existing v1 recent projects and layouts
  migrate in memory and persist on the next explicit update. Identity includes
  target host and canonical root; display name is mutable metadata. The helper
  executable is resolved for each connection, not stored as a preference.
  SSH connect/validation, file actions and layout restoration stay off
  the UI thread. Failed connects preserve the previous workspace and dirty views.
  Close/switch never destroys a document with an unacknowledged save.
- Disconnect preserves local documents/undo. Reconnect creates a fresh helper,
  resynchronizes LSP and rechecks file baselines. Helpers, LSP and terminals are
  connection-scoped; terminal restart is explicit. Remote history remains in
  memory, with no remote/local project-history writes from destructors.

Intentional first-version differences: remote editable files larger than 1 MiB
are rejected instead of displaying the local read-only truncation notice; remote
paths must be UTF-8 and remain within one project root. Remote Open/Save As use
an absolute-path modal. Delete asks for permanent removal, not desktop Trash.
Local configuration JSON editing from a remote workspace reports its local path
and requires a local workspace; ordinary Settings controls still work. Closing
during a pending remote save retains the tab/group until the user retries after
completion. Multi-root and persistent remote sessions are not implemented.
Client calls time out after 60 seconds and disconnect.
Production runs an already-built helper: no compiler, toolchain wrapper or
build-on-connect step is installed or invoked. CI uploads the standalone native
helper with license notices separately from desktop packages. Source builds use the standard
Rust/C toolchains; the test server's build prerequisites are GCC/build-essential
and pkg-config, installed at the user's explicit request.

New direct dependencies are internal `bed-remote` edges from the desktop,
session, files, LSP and terminal packages, and internal `bed-remote`/`bed-files`
edges from `bed-headless`. Their purpose is sharing protocol/transport and the
existing search engine. `bed-remote` uses existing Serde 1.0.229 (derive for wire
types) and locked serde_json 1.0.151 (JSON frames); no new third-party package
version is introduced. `bed-terminal` defaults to its `ui` feature; disabling
default features excludes ImGui and exposes the emulator/PTY modules headlessly.
The crate-boundary checker records the new edges and forbids GUI dependencies
throughout the headless service graph.

Validation includes the full locked/offline workspace/all-target suite, strict
workspace Clippy, format checks and crate-boundary checks. New regressions cover
framing/version errors, byte preservation, filesystem conflicts and no-clobber
operations, subprocess teardown, delayed save acknowledgements, Save As, reload,
reconnect/undo, canonical aliases, workspace migration and asynchronous layout
restoration. Live SSH acceptance passes on `bed-test@orb` (Linux aarch64), using
a normal native release build with GCC. Backend tests cover BOM/CRLF reads and
saves, conflict protection, search byte columns, Git HEAD/status, rename and
discovery. The actual Workbench test covers asynchronous connection/open,
shared-view editing, acknowledged saves, rename, external conflicts and
disconnect/reconnect preserving buffers, views and undo/redo. Remote rust-analyzer
receives unsaved text and returns its new symbol; the terminal test verifies
working directory and PTY resize. All 15 remote/backend tests also pass natively
on the Linux server. The test server's missing rust-analyzer component was
installed for LSP acceptance. The temporary Zig download was removed; no Zig
toolchain or compiler wrapper is used. The tested helper is installed at
`/home/benton/.local/bin/bed-headless` on that server; its runtime dependencies
there are the system libc and libgcc runtime, with no compiler needed.

Automatic SSH helper deployment extends this implementation. Desktop packages
carry native Linux x86-64 and ARM64 musl helpers, built with ordinary C compilers
on CI builders. The connection worker probes the remote platform, selects the
matching bundled executable, and uploads it through SSH stdin to a private
versioned cache under the SSH user's home directory. The protocol version,
binary length and content fingerprint select a cache entry; protocol probes
validate executability before reuse and before an atomic install. The fingerprint
identifies cached builds, not an authentication mechanism; SSH authenticates the
transport. Connection setup never compiles on the server, uses sudo or downloads
helper releases from the Internet. The helper is always managed automatically.
Workspace persistence stores the host and canonical root, while sessions, LSP and
reconnect use the resolved remote cache path. Reconnect can update that path
without losing buffers, views or undo. No third-party dependency was added.

Automatic deployment validation: the full locked/offline workspace/all-target
suite (including 80 desktop tests), strict workspace Clippy, formatting and
dependency boundaries pass. Seven packaging regressions validate architecture,
static linking, complete ELF contents, checksums, protocol and notices. Native
ARM64 musl staging passes and its checksum matches the test-server build. Real
SSH tests verify a fresh install, same-inode cache reuse and an actual protocol-v2
RPC connection. The Workbench test connects with only host/root configuration,
persists the automatic preference, and verifies editing, saves, rename, conflicts
and reconnect with undo retained. All test fixtures are cleaned; the normal
managed helper remains cached for reuse. Linux x86-64 static builds and complete
desktop release bundles are assigned to CI, rather than claimed as locally run.

The Projects form now uses one bounded, centered content child so SSH headers,
inputs and buttons share the same edges, including in narrow panels. It has only
SSH host and project path fields, with examples as input hints. The workspace-name
and Advanced executable-path controls are removed. New remote display names come
from the canonical folder; saved names, layouts and file-tree preferences remain
associated with the same host/root identity. Existing JSON executable overrides
are ignored and omitted when the workspace is next recorded.

Remote project paths accept absolute paths, `~` and `~/...`. A bounded background
SSH probe reads the remote account's HOME; the suffix is joined literally, without
evaluating shell variables, substitutions or glob characters. The helper then
canonicalizes and validates the directory before switching workspaces or recording
its identity. Unsupported `~otheruser` and ordinary relative paths are rejected.
Connecting through an equivalent path preserves active buffers, views and undo;
if disconnected, it reconnects the existing session instead of closing documents.
This extends Bed's project infrastructure without changing upstream buffer or
editing algorithms, and adds no third-party dependency.

Projects form validation: the locked/offline workspace/all-target suite (82
desktop tests), strict workspace Clippy, formatting, dependency boundaries and
seven packaging tests pass. Native ImGui tests check wide/narrow control bounds,
the two fields, Connect submission and whole-row recent-project clicks. A native
Metal capture at `target/ui-projects-smoke/projects.png` confirms the rendered
form. Live `bed-test@orb` checks verify remote HOME expansion with literal shell
characters and a `~/.../folder with spaces` Workbench connection, edits, saves,
conflicts, reconnect and retained undo. Fixtures are removed after acceptance.


## Project-scoped file-tree visibility — 2026-10-06

`src/files/file_tree.rs` extends the converted lazy tree with checked row and
background menu controls for hiding Gitignored paths and dot-prefixed names,
manual Hide/Unhide actions, and transient Show Hidden Files mode. Reveal mode
shows filtered labels and icons at 45% opacity while keeping menu text legible.
The project root is protected. Loaded nodes and expansion state survive filtering;
concealed open subtrees are refreshed only when visible. Opening or hiding an
entry does not change document ownership, search, quick-open, undo or save behavior.

`src/util/workspace_state.rs` adds a `file_tree` object alongside each workspace's
layout in the existing v2 store. It records `hide_gitignored`, `hide_hidden`, and
sorted project-relative `hidden_paths`; absent settings default to false/empty.
Workbench explicitly saves changed preferences immediately, including before an
ImGui frame exists, and restores them for local and SSH workspace identities.
Show Hidden Files is not persisted and resets when switching projects. Exclusions
are path-based and do not follow renames. No project configuration file, new
dependency, embedding persistence, or destructor write is introduced.

`crates/bed-files/src/tree_ignore.rs` uses existing libgit2 discovery and index
support to map project-root aliases and preserve tracked files and directories
containing them. Actual ignore classification uses the shared NUL-delimited,
batched Git helper in `crates/bed-remote/src/filesystem.rs`. This deliberately
avoids libgit2 1.9.7 dropping a non-wildcard child negation when its positive rule
comes from a parent `.gitignore`; a regression covers that exact case. Git must
be installed on the project's machine when this filter is enabled. Disabled
filters do not probe Git. Classification errors retain entries as visible and
are reported through existing workbench errors.

Remote `ReadDirectory` gains an explicit `classify_gitignored` request flag;
`DirectoryEntry` gains `is_gitignored`, and directory responses can carry a
classification warning without discarding the listing. The protocol is now v2,
requiring matching desktop/headless binaries. The connection validation probe
requests an ordinary listing without Git classification. No remote paths are
probed on the local filesystem.

Intentional difference from the pinned upstream: `.DS_Store` and `thumbs.db` are
no longer unconditionally discarded, so every entry can be shown and explicitly
hidden. Sorting, lazy loading and retained expansion remain covered by the
translated tests, with the old skip-name expectations updated.

Validation: `cargo test --workspace --offline`, the final focused
`cargo test -p bed-remote --offline`, and
`cargo clippy --workspace --all-targets --offline -- -D warnings` pass.
Native ImGui menu tests
exercise filter toggles in both states, exact-path Hide/Unhide actions, dimmed
vertex alpha, root protection, menu padding and unchanged creation/rename actions.
Service and persistence regressions cover combined filters, reveal/unhide, skipped
refreshes, project switching, immediate persistence, layout preservation,
local/SSH identity separation, rename semantics, nested Git rules, tracked paths,
root aliases, error fallbacks and large batches exceeding pipe capacity. A real
headless subprocess verifies the new directory metadata and version negotiation.
Live SSH, native Windows/Linux GUI acceptance and GPU screenshots were not rerun
for this change.


## Bed repository cleanup — 2026-10-06

Bed now selects released Tree-sitter dependencies independently of ned's recorded
source snapshot. The unmodified `vendor/tree-sitter` and `vendor/grammars` trees
were removed: 212 files totaling 138,689,801 source bytes. `bed-highlight` no longer
has its own grammar build script or direct `cc` build dependency. Released grammar
crates provide their `LANGUAGE` handles and build their native parsers. The custom
document buffer, editing commands, undo, save algorithms and highlighting engine
remain in place.

The runtime is the released `tree-sitter` 0.26.13, supporting grammar ABI 13–15;
selected providers use ABI 14–15. There is one runtime and one
`tree-sitter-language` 0.1.8 in the resolved graph. `regex` moves from 1.11.1 to
1.11.3 to satisfy the released runtime's minimum without adding another regex
version. The 17 new direct grammar dependencies replace native source copies;
each supplies the parser for its corresponding language:

| Grammar crate | Version |
| --- | --- |
| `tree-sitter-bash` | 0.23.3 |
| `tree-sitter-c` | 0.23.4 |
| `tree-sitter-cpp` | 0.23.4 |
| `tree-sitter-c-sharp` | 0.23.1 |
| `tree-sitter-css` | 0.23.2 |
| `tree-sitter-go` | 0.23.4 |
| `tree-sitter-hcl` | 1.1.0 |
| `tree-sitter-html` | 0.23.2 |
| `tree-sitter-java` | 0.23.5 |
| `tree-sitter-javascript` | 0.23.1 |
| `tree-sitter-json` | 0.24.8 |
| `tree-sitter-kotlin-codanna` | 0.3.9 |
| `tree-sitter-python` | 0.23.6 |
| `tree-sitter-ruby` | 0.23.1 |
| `tree-sitter-rust` | 0.24.0 |
| `tree-sitter-toml-ng` | 0.7.0 |
| `tree-sitter-typescript` | 0.23.2 |

TypeScript continues to use the TSX provider for both TS/TSX, preserving the
existing language mapping. The Kotlin codanna provider and maintained TOML
provider avoid introducing
older, conflicting Tree-sitter runtimes. Kotlin retains the original query/node
family and string interpolation behavior; original behavioral fixtures retain
their recorded values.

The four retained vendor forks contain active Bed changes: native FreeType/SVG
fonts and docking accessors, static FreeType/zlib linkage, owned viewport event
routing/teardown, and per-viewport GPU effects/capture. `vendor/README.md` explains
the policy and links the existing patch records. The boundary checker rejects
local third-party Cargo dependencies without a `BED_PATCHES.md`. Unused native
node-info and central-node mutation shim functions were removed.

Runtime configuration and application icons now use Bed names. Existing
`ned.json` configuration imports into `bed.json` while preserving named profiles,
custom settings and the original file. Atomic primary writes detach symlink or
hard-link aliases before changing settings. Selecting Bed after another profile
normalizes the pointer before saving. Public package/About descriptions describe
Bed, and Windows now packages the existing Bed artwork. Source attribution and
licenses remain intact.

Original implementation verifiers and fixture regenerators live under
`scripts/provenance/`, separately from current packaging tools. Their source
hashes and fixture values are unchanged; normal Cargo builds/tests need no
reference checkout. License discovery skips nested build/VCS caches and collects
current dependency notices instead of obsolete source copies.

Sixteen published grammar archives omit their copyright/license text. Their
exact-release notices are retained in `LICENSES/tree-sitter/`, with repository
revision, URL and SHA-256 records. Packaging verifies that provenance and places
the texts alongside each provider's Cargo notice index without changing Cargo
caches or downloading during collection. Runtime, language and HCL packages
already contain their notices; all 19 Tree-sitter packages have actual staged
license text.

Cleanup validation: the full locked/offline workspace/all-target suite passes,
including 91 desktop tests, unchanged exact highlighting captures for all 17
languages, and three new Kotlin declaration/literal/interpolation regressions.
Strict workspace Clippy, formatting, crate/dependency boundaries and all 10 Python
packaging/license checks pass. Release Bed, bed-headless and the embedding example
build successfully. A native Metal launch renders five frames and imports an
isolated customized legacy profile into bed.json, preserving every original byte.
The relocated original model verifier passes 872 assertions in 72 cases. Original
fixture bytes are unchanged. Native Linux/Windows execution and full desktop
release bundles remain CI checks rather than claimed local acceptance.

## Bevy model rendering and STL

The Model Viewer keeps the existing `bed.gltf.*` plugin, panel, viewer and
command IDs for workspace compatibility. Its bounded background importer now
retains glTF metallic/roughness, normal, emissive and occlusion maps, independent
samplers and UV channels for Bevy's PBR materials. Draco meshes and authored skin
poses retain the existing decoder path. ASCII and binary STL load through the
patched `bevy_stl` byte loader with a neutral PBR material; the input is bounded
to 64 MiB and 333,333 facets. Lighting presets, skyboxes, shadows, exposure and
optional screen-space AO are saved alongside the orbit camera. Appearance also
supports wireframe-only, wireframe overlay, normal colors and bounded normal-vector
overlays. These use line-list meshes and unlit materials, without requiring native
polygon-line GPU features. The skybox horizon defaults lower and can be adjusted
independently of geometry and lighting. Two CC0 Poly Haven Radiance HDRIs are
embedded in the binary, decoded as floating-point cubemaps and filtered by Bevy
for lighting and reflections; they require no runtime downloads.

Host/all-target checking and all 21 plugin-host integration tests pass, including
STL routing, shared Hex edits and model appearance/camera restoration. Native
Metal readback verifies sRGB rendering into the host's unorm texture and callback
restoration; both existing effects fixtures pass on wgpu 29. The desktop plugin
smoke passes 70 frames and five secondary presentations, covering model resize,
detached presentation, close/reopen and two GPU recovery cycles. The model capture
at `target/bevy-host-smoke/gltf.png` was inspected. Package license collection
includes the Bevy, patched `bevy_stl` and `stl_io` notices. Native Linux/Windows
acceptance is configured in CI and has not been run locally.

The complete workspace test suite passes with the inspection modes and embedded
HDRIs. Eight native Metal fixtures verify mode transitions, shaded wire overlays,
normal vectors, distinct HDRI reflections and horizon movement without changing
the model's position. Optional `BED_MODEL_CAPTURE_DIR` retains PNG readbacks for
visual review. Poly Haven asset provenance and the CC0 dedication ship under
`LICENSES/polyhaven`.

Changing environment presets recreates Bevy's filtered reflection maps so their
dimensions and mip chain follow the selected source. Native regression coverage
includes 64px procedural to 256px HDRI transitions in both directions and rapid
preset changes before filtering settles.

Skybox blur samples Bevy's filtered environment mips through a separate GPU image
sampler. The background's blur setting is saved with the workspace; the lighting
and reflection maps keep their original samplers. The alias follows regenerated
maps when switching between procedural environments and HDRIs.
Native Metal readbacks verify progressive background blur, preserved model
reflections, sharp restoration and rapid environment changes with blur enabled.
