# bedtime-automation2

Optional application control over stdin/stdout. The host keeps its normal Bevy
runner, window, rendering and update schedules. Test scenarios and assertions
live in external scripts.

`bed2` enables it with one feature-gated plugin registration:

```sh
cargo build --offline -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
```

The smoke script drives the real UI and lifecycle behavior, and writes PNGs,
`transcript.jsonl`, `stderr.log` and
`summary.json` under `target/automation2-smoke`. It uses only Python's standard
library. Run it in a desktop session with access to the native window server.

For another application:

```rust,ignore
#[cfg(feature = "automation")]
app.add_plugins(bedtime_automation2::AutomationPlugin::stdio());
```

The default crate feature `workbench` supplies UI and Workbench adapters.
`default-features = false` provides the independent Bevy transport, input,
inspection, resizing and screenshots; `features = ["ui"]` adds visible text
observations and viewport capture without Workbench. Compiling the crate does
not activate it: the host must install `AutomationPlugin`. `manual()` exposes
the same scheduling/protocol through the `Client` resource without process I/O.

## Wire protocol

One JSON object per line, UTF-8, at most 1 MiB. stdout is reserved for protocol
messages; application logs belong on stderr (Bevy's default). Every response is
flushed. The startup event follows the first completed application update:

```json
{"event":"ready","protocol":1,"frame":1,"capabilities":["describe","inspect"]}
```

The actual capabilities list contains the compiled operations. `ready` means the
protocol is usable; it does not promise that every asset or pipeline has loaded.
`inspect.render` reports render frames and pending pipelines; UI diagnostics and
visible text let clients wait for application-specific readiness.

```json
{"id":1,"op":"inspect"}
{"id":1,"frame":3,"result":{"windows":[],"ui":{"frame":3,"surfaces":[],"texts":[]}}}
{"id":2,"op":"unknown"}
{"id":2,"frame":4,"error":{"code":"operation_failed","message":"Unknown operation: unknown"}}
```

IDs are strings or unsigned integers, unique among pending requests. Operations
execute sequentially. Results follow completion, rather than queue admission;
normal mutations wait through a subsequent frame, command invocation waits for
the actual command receipt, workspace closure waits for despawning, and capture
waits for GPU readback and successful PNG writing. Protocol/validation errors may
be returned immediately. Every response includes the application frame.

Each request accepts `timeout_ms` (default 15000, range 1–120000). Timeout releases
held input and abandons pending capture/command observations; it does not roll
back an already applied application transition. Queue capacity is 128. Incoming
stdin wakes an idle Winit app, and pending operations keep it updating. I/O and
PNG writing run off the app thread. A slow/disconnected stdout consumer cannot
block the app thread; a full response queue ends the automation process with an
error exit.

EOF drains accepted requests, flushes their responses, then exits successfully.
`exit` also waits for its response to flush. Outstanding requests after `exit`
receive errors. Host application crashes remain process failures, available in
stderr and the exit status.

Entity IDs are opaque decimal **strings** containing Bevy's complete entity bits,
including generation. Inspection returns them; commands reject stale IDs. IDs
belong only to this process. Session persistence and stable restore identities
are application-owned; the adapter provides no workspace/layout snapshot format.

## General operations

Arguments appear alongside `id` and `op`.

| Operation | Arguments / result |
|---|---|
| `describe` | Supported operations, input step types, custom operations and protocol version. |
| `inspect` | Optional `entities: true` includes entity names and component type names. Always returns windows; compiled adapters add UI, viewports and workbench state. |
| `entity.inspect` | `entity`: live ID. Returns component type names. |
| `configure` | Optional `fixed_dt`: seconds per update, or `null` to restore automatic time. Optional `exclusive_input`: bool (default true). With UI support, `clipboard`: text installs a memory clipboard containing that text. |
| `wait_frames` | `frames`: 0–10000 ordinary application updates. This does not pause the app between requests. |
| `window.resize` | `size: [width,height]` in logical pixels, each 1–8192. Optional `window` ID; otherwise the primary/unique window. |
| `input` | `steps`: sequence below. Optional `window` ID. |
| `capture` | `path`: PNG output. Optional `window`, `panel` (visible content crop), `viewport` (UiViewport image), or `crop: [x,y,width,height]` in physical pixels. |
| `exit` | Clean exit after the receipt flushes. |

Window snapshots report logical and physical sizes and scale factors. UI surface
snapshots report egui `pixels_per_point` and `zoom`. Panel/workbench geometry is
in egui points: multiply by `zoom` for logical input coordinates, or by
`pixels_per_point` for screenshot pixel coordinates. `ui.texts` rectangles are
already converted to **logical window coordinates**. They include only text
whose layout bounds intersect its paint clip. They are observations of painted
text, not guarantees that the underlying widget is enabled or uncovered.

PNG capture reports `width`, `height`, `hash`, `nontransparent_pixels` and
`capture_frame` (the update requesting the rendered frame). Captures contain
application rendering; native titlebar buttons and the OS cursor are outside the
image. Hidden panels are not specially rendered. Request `inspect` first when
choosing a panel or viewport target.

### Input sequences

Each primitive step is applied on a separate frame before Bevy's input systems
and normal egui conversion. Held buttons/keys survive steps within the sequence
and are released before its completion receipt, including on timeout.

```json
{"id":3,"op":"input","steps":[
  {"type":"move","position":[300,200]},
  {"type":"button","button":"Left","pressed":true},
  {"type":"wait","frames":3},
  {"type":"button","button":"Left","pressed":false},
  {"type":"scroll","delta":[0,-4]},
  {"type":"key","code":"ControlLeft","pressed":true},
  {"type":"key","code":"KeyA","pressed":true},
  {"type":"key","code":"KeyA","pressed":false},
  {"type":"key","code":"ControlLeft","pressed":false},
  {"type":"text","text":"Hello"},
  {"type":"key","code":"Enter","pressed":true}
]}
```

`key` uses Bevy `KeyCode` names and does not insert printable text. Use `text`
once for text insertion. On macOS, use `SuperLeft` for Command shortcuts.
`motion` with `delta: [x,y]` emits raw relative `MouseMotion` for gameplay controls.
`button` uses Bevy `MouseButton` names (`Left`, `Right`, `Middle`). Scroll deltas
are lines, or physical wheel pixels when `pixels: true` is supplied.

Conveniences expand to the same input events across frames:

```json
{"id":4,"op":"input","steps":[{"type":"click","position":[300,200]}]}
{"id":5,"op":"input","steps":[{"type":"click_text","text":"Light preset","index":0}]}
{"id":6,"op":"input","steps":[{"type":"drag","from":[480,300],"to":[550,300],"frames":12}]}
```

`click` optionally accepts `button`. `click_text` uses the last observed exact
text match in the target window; `index` disambiguates duplicates. `drag` uses
the left button and defaults to 12 movement frames. Sequences are limited to
1024 steps and 10000 expanded frames. With `exclusive_input` enabled, native
keyboard/pointer events are filtered so the user's mouse cannot redirect the
scripted pointer; window lifecycle events continue normally. Injection targets
Bevy's application input path, rather than generating OS-wide native events.

## Workbench operations

All use the real main-world entities and existing public workbench transitions.
`workspace` defaults to the active workspace where applicable; `layout` defaults
to that workspace's active layout. Explicit selection may be empty.

| Operation | Arguments |
|---|---|
| `workspace.create` | `name`, optional `with_layout` (true), `activate` (false), `background_updates` (true). Returns workspace and optional layout/area IDs. |
| `workspace.activate` | `workspace`: ID, or `null` to clear selection. |
| `workspace.configure` | Optional `workspace`, `name`, `background_updates`. |
| `workspace.close` | Optional `workspace`, `replacement`. Waits for workspace workers to finish and ownership teardown to complete. |
| `layout.create` | `name`, optional `workspace`, `activate`. Creates a layout with one empty area. |
| `layout.activate` | `layout`: ID, or `null` with optional `workspace` to clear its selection. |
| `layout.delete` | Optional `layout`. |
| `layout.move` | `layout`, `workspace`. Moves its ownership subtree; rejects conflicting active layout selections. |
| `panel.focus`, `panel.close` | `panel`. |
| `panel.move` | `panel`, `area`, optional `index` within the destination tabs. |
| `area.split` | `area`, `axis`: `horizontal` (x) or `vertical` (y), optional `fraction` (0.5). Returns the new area. |
| `area.close` | `area`. Uses normal geometry-preserving close behavior. |
| `invoke` | `command`: registered key; optional `workspace`, `panel`, or `global: true`. Returns the actual handler outcome. |
| `surface.configure` | Optional `window`, partial `settings`, `panels_visible`, `titlebar_height`, `titlebar_ownership` (`workbench`/`application`), `blur`. |
| `surface.detach`, `surface.attach` | Optional `window`. Detach remembers WorkbenchSurface settings; attach reinstalls them (or defaults). Normal lifecycle owns generated themes and egui sessions. |
| `theme.set` | Optional `window`, `preset` (`dark`/`light`), partial `theme` object. Validates colors/metrics before writing. |

Surface `settings` accepts the actual WorkbenchSurface fields, including
`rounding`, `padding`, `panel_gap`, `hit_size`, `drag_corners`, `tab_bar`
(`always`/`multiple_tabs`/`hidden`), `single_area_edge_to_edge`, `layout_editing`,
`panel_creation`, `panel_closure` and `debug`. `panel_creation` gates creation
drags, not a built-in type picker. Theme patches recursively merge
`palette`/`metrics` fields; inspect returns their complete current values.
Unknown configuration fields are errors.

```json
{"id":7,"op":"surface.configure","settings":{"drag_corners":false,"tab_bar":"hidden"}}
{"id":8,"op":"theme.set","preset":"dark","theme":{"palette":{"accent":[0.2,0.9,0.5,1]}}}
```

There is no generic `panel.open`, `workspace.capture`/`workspace.restore`, or
`layout.capture`/`layout.restore`/`layout.replace` operation. Applications expose
concrete creation and session operations through their own UI or custom operations;
descriptive panel keys are not a Workbench construction registry. `capture` in the
general operations table means a rendered PNG, not session serialization.

Inspection includes command availability, workspace and layout selection, area
tiling, inherited active/running markers, panel and
tab/close bounds, surface settings, themes and shell allocations. Derived state
is observed; automation does not maintain a second workspace model.

## Application-specific control

Register an `app.*` handler for application state that the built-ins cannot
describe. The handler runs on the app thread, can use normal application APIs,
and returns JSON or an error. It shares ordinary ordering, IDs, timeouts and the
subsequent-frame completion boundary. For asynchronous application work, return
its normal receipt/identity and poll it through an inspection handler.

```rust,ignore
use bedtime_automation2::AutomationAppExt;
app.register_automation_command("app.inspect_game", |world, _arguments| {
    Ok(serde_json::json!({"score": world.resource::<Game>().score}))
});
```

The external Python `Application` client supplies `request`, `inspect`,
`wait_until`, `input`, `click_text` and `capture`, and retains the transcript and
stderr. It raises on request errors, malformed stdout, timeouts and premature
process termination. The command-line relay also accepts JSON operation lines:

```sh
python3 bedtime2/tools/automation.py --output target/session -- target/debug/bed2
```

For transport-only hosts, the `headless` example installs MinimalPlugins and the
same stdio plugin without a window or renderer:

```sh
cargo run --offline -p bedtime-automation2 --no-default-features --example headless
```

`python3 bedtime2/tools/smoke_protocol.py` exercises that built example's real
stdio transport, including piped startup, EOF draining, malformed/oversized
requests and broken stdout.
