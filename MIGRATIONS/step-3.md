# Stage 3: the workbench foundation

Bedtime's foundation is desktop, UI and workbench. Applications choose what to
open and where to persist it; workbench owns the transitions that make those
choices happen safely. This extends the architecture recorded in
[the original migration overview](../MIGRATION/overview.md).

## Agreed ownership

| Concern | Workbench owns | Application or plugin supplies |
| --- | --- | --- |
| Workspaces | Active context, scoped state, activation and retirement | Selection, identity and storage |
| Layouts | Splits, tabs, focus, switching and restoration | Initial arrangements and layout selection UI |
| Panels | Instances, lifecycle and ImGui windows | Content drawing and presentation state |
| Documents | Canonical content, history, dirty state and save lifecycle | Content model, edits and save binding |
| Settings | Definitions, validation, overrides and resolution | Definitions and optional custom settings pages |
| Persistence | Scoped snapshots, pending writes and acknowledgements | Reading and writing snapshots |
| Commands | Registration, context, availability and shortcuts | Handlers and default bindings |

There is always an active workspace and layout. A temporary workspace is a
normal workspace, with in-memory state. Persistence and permission to override
settings are independent. A single-workspace application can disable workspace
overrides. Explicit writes to that disabled scope fail rather than changing
global settings instead.

Settings resolve per definition: workspace override, global override, default.
Writes name their scope; reset removes an override. Ordinary plugin data has an
explicit scope and no inheritance. An optional settings panel uses the same
public definitions and mutation API that a replacement application UI would use.

Workbench produces structured snapshots of layouts, panel records and namespaced
plugin data. Plugins encode their own records. Application storage need not
understand every plugin. Records for missing plugins should survive round trips.
Async operations capture their workspace/destination when admitted.

Layouts retain panel instances when inactive. Inactive panels do not draw or
receive shortcuts. Panels and documents have distinct lifetimes. Closing a
viewer does not close a document used elsewhere. Last-view close, workspace
replacement and quit share save/discard/cancel handling. A cancelled decision or
failed required save leaves the current context usable.

Documents have one canonical content instance, one authoritative history, any
number of independent viewers, a creator-supplied persistence binding, and a
checkpoint acknowledged only after saving succeeds. History uses transactions
and saved state identity, so undoing back to the saved state becomes clean.
Save operations receive snapshots (typed where useful), run asynchronously and
have defined ordering per destination. Runtime documents and save bindings are
distinct from persistent restoration descriptions.

Workspace, layout, panel and document instances are entities with references.
Controlled owners perform consequential transitions. Immediate widget layout,
animation interpolation and transient ImGui bookkeeping remain UI-local.
Workbench owns its fullscreen host and panel windows; plugin systems draw the
content inside them.

Commands build on the existing typed registration and queued dispatcher.
Routing considers modal UI, focused controls, panels and global bindings, avoids
duplicate native/keyboard activation, and captures the target at admission.

Process execution belongs in an optional service, outside the foundational
workbench. Workbench can present tasks and coordinate shutdown; the execution
service owns spawn, streams, cancellation and exit status. PTY and debugger
semantics remain separate consumer requirements. Remote implementation is deferred.

## Implementation slices

1. **Workspace, layout and panel lifecycle.** Always-present workspace, entity
   references, retained layouts, tabs, splitting/resizing, focus and panel close.
   Move the gallery and cube viewport into real workbench panels. Keep the UI-only
   gallery usable without workbench. No document-aware close policy yet.
2. **Documents and history.** Two independent viewers, typed transactional edits,
   undo/redo and asynchronous save bindings.
3. **Scoped data and settings.** Memory and application-owned file storage,
   definitions, inheritance and an optional settings panel.
4. **Restoration and interaction completion.** Persistent layouts/panels,
   shortcut routing, and save/discard/cancel across close, workspace switch and quit.

The first slice is an in-memory foundation, not a migration of the v1 application.
Applications create/delete/select workspaces and layouts through controlled APIs.
The demo must exercise those same APIs. Headless lifecycle tests protect ownership
and retention; native checks exercise drawing, input, camera targets and composition.

## Slice 1 implementation

Implemented in `bedtime2/crates/bedtime-workbench2`. Its default build remains
headless. The optional `ui` feature attaches to an existing UI surface and does
not install Bevy plugins, windows or a renderer.

- `Workspace`, `Layout`, `Area` and `Panel` are entities. Owning relationships
  form workspace → layouts → areas → panels (`InWorkspace`, `InLayout`, `InArea`).
  Deleting an owner retires its dependants.
- Area rectangles tile a normalized integer plane. ECS stores the canonical
  rectangles, tab order and selected panel; there is no parallel live layout
  tree. Temporary pure geometry snapshots calculate edits. Switching layouts or
  workspaces retains instances. The last layout cannot be deleted, and deleting
  the active workspace requires selecting another first.
- Rendered panel instances are registered Bevy system entities. Their systems
  receive a borrowed `UiFrame` inside workbench-managed content windows, plus
  normal queries/resources. State is attached directly to the panel entity.
- Drawing queues changes. A controlled FIFO stage applies them after drawing and
  command dispatch. Gesture requests capture layout identity, structural revision
  and workspace/layout activation, rejecting stale edits even after switching
  away and back. Geometry is planned before a controlled ownership commit.
- The frontend ports v1's Blender-style bordered areas, corner splitting, partial
  joins and area relocation, connected border resizing, tab reordering and
  grouping, and four edge drop targets. A last-tab move fills the vacated area;
  closing the last tab retains an empty area. Preview and drop share the same
  final geometry plan, including gap filling and panel border insets.
- Transient gestures, scrolling, interpolation and content fades are UI-local.
  Border resizing previews locally and commits on release. Escape, right click,
  focus loss, layout activation or bounds changes cancel a gesture.
- Panel definitions register typed factories as Bevy system entities. The area
  picker and `panel_drag_source` use those same factories. Content widgets get
  priority over the workbench's fallback panel-type drop target. Tabs support
  icons, dirty markers, preview metadata, middle-click close and close context
  menus. Its flat host does not override dialog or floating-window theme styles.
- `bed2` supplies application chrome for workspace/layout creation, deletion and
  selection. Playground has Split and Reading layouts; Scratch has editable local
  notes. The widget gallery and game viewport are ordinary panels.
- Hidden panels stop drawing. The application adapter deactivates the game camera
  while its viewport is hidden, and still supports the full-window scene/HUD.

Run `cargo run -p bed2`. Drag tabs along the strip to reorder, into a panel's
center to group, or toward an edge to split. Drag an area's corner inward to
split, across an adjacent border to join, or farther into another area to relocate.
Drag connected borders to resize. Use the area's + button (or right-click an
empty area) to create a panel; the toolbar's **Drag notes** demonstrates factory
drops. Switch layouts/workspaces to inspect retention.

State is currently in memory. Documents, document-aware close decisions,
settings, persistence, shortcut routing, cross-layout moves and OS floating panels
remain later work. There is no generic file-drop integration yet; the current
workbench payload creates panels from registered types.

Verification commands:

```sh
cargo test --offline -p bedtime-workbench2 --lib --no-default-features
cargo test --offline -p bedtime-workbench2 -p bedtime-ui2 -p bedtime-desktop2 --lib --features bedtime-workbench2/ui
python3 scripts/check-crate-boundaries.py --offline
cargo run --offline -p bed2 -- --workbench-smoke-dir target/workbench2-smoke
cargo run --offline -p bedtime-ui2 --example gallery -- --smoke-dir target/workbench2-gallery-regression
```

The native workbench check covers spinning output, divider input and target
resizing, full-window alpha composition, actual close/recreation, retained
workspace/layout switching and tab selection. Its interaction sequence also
exercises reordering, all four tab edges, corner split/join/relocation, connected
T-junction borders, cancellation, stale gestures, factory drops, content drop
priority and retaining an empty area after the final tab closes. Unit regressions
compare drag outlines with committed panel surfaces, including last-tab gap
filling and T-junctions. The standalone gallery check protects the UI-only path.

Native verification also observed intermittent bad captures: all-black output
(also observed during stage 2) and one capture missing part of a table column.
Later runs passed, including a new pixel assertion for that column after tab
regrouping. Temporary diagnostics showed valid native table clipping and no UI
errors or target-size mismatches on the successful diagnostic run. The cause is
not established; these observations must not be described as fixed.
