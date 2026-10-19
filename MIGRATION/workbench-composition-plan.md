# Workbench composition: agreed design and implementation record

This document preserves the decisions from the UI/titlebar composition discussion
and is the handoff record if the coding conversation is compacted.

## Accepted design

- Rendering is sequential. UI functions receive `&mut egui::Ui`, `&mut World`,
  and a separate borrowed UI context. Children are ordinary Rust function calls
  inside ordinary egui layout closures. Do not wrap every egui layout method.
- UI renderers are no longer registered Bevy systems. Remove drawing-specific
  `IntoSystem`, `SystemId`, `run_system_with`, and implicit `Local` storage.
  Scheduled updates, input, simulation, and command handlers remain Bevy systems.
- egui owns widget input. Workbench consumes eligible shortcuts from its own
  camera's egui context after drawing, preserving command scope and repeat
  policy. There is no event-ownership router, panel input forwarding, per-owner
  held-key state or routed application stream. Panels/applications reading Bevy
  input own their focus/visibility gating and held-key cleanup. UiCapture remains
  a per-surface observation of keyboard, pointer, text and popup/modal activity.
- `bedtime_ui2::Component` describes a reusable renderer with associated generic
  borrowed props (`type Props<'a>`). Props can be forwarded to child components.
  Rendering context holds UI services and instance identity, not a borrowed Ui.
  Existing useful UiFrame helpers may remain as an ordinary convenience wrapper.
- `bedtime_workbench2::Panel` is a separate drawing contract that also receives
  its panel entity. Panel metadata and instance state belong to workbench. A
  panel composes UI components and can delegate its entire contents to one.
- All workbench main content uses panels, including simple applications with
  one panel. There is no separate non-panel workbench content rendering path.
  Standalone UI2 applications remain independent of workbench and panels.
- WorkbenchShell owns workspace presentation and shell allocation, including
  titlebar, menus, status bar, and panel contents. A root surface renderer calls
  shell and child renderers as ordinary functions.
- WorkbenchSurface::default() on a window provides automatic UI surface, theme
  and shell setup. The application must supply exactly one WorkbenchCamera;
  link it to the unique workbench surface and validate duplicates/targets.
  Never create a fallback camera or select an unmarked UiCamera. The application
  owns the camera and render order. Explicit themes and shell configuration can
  override their defaults.
  Required components supply local defaults; one PreUpdate setup system wires
  related entities before UI initialization, after application Startup. There is
  no attach_surface helper or WorkbenchBinding resource; query the components.
  Retain low-level UI2 surface/camera configuration for standalone applications.
- Keep instance identity and lifetime ownership explicit. Egui ID memory is for
  presentation state; ECS resources/components hold application state. Borrowed
  props cannot alias a simultaneously mutable World borrow.
- Workbench and the application share one main World and RenderApp. Workspaces
  are entities, and workspace-specific state is stored in their components.
  InWorkspace owns editor entities; lifecycle observers and one synchronization
  phase maintain InActiveWorkspace and InRunningWorkspace markers. Ordinary game
  entities need no Workbench membership. Application/plugins own any optional
  isolated game worlds/renderers; Workbench does not manage child Apps.
- Match bed1's configurable automatic edge-to-edge style for a single tiled
  area: no outer gaps, area border, or rounding. One area may have several tabs.
  Provide configurable tab visibility and user editing/creation/closure policy.
  Disable actual gestures, drop targets, pickers, and relevant user commands;
  hiding controls alone is insufficient. Application APIs retain panel/layout
  management capability. Content padding and scrolling remain opt-in.
- Preserve explicit titlebar ownership (Workbench/Application), native measured
  controls reservation, 4-point top padding and 24-point extra control gap,
  reversible macOS native appearance/drag handling, and widget drag behavior.
- Preserve bEd2's compact 20-point titlebar contents and its current controls.

## API direction

```rust,ignore
trait Component {
    type Props<'a>;
    fn draw(ui: &mut egui::Ui, world: &mut World,
            context: &UiContext<'_>, props: Self::Props<'_>);
}

trait Panel {
    fn draw(ui: &mut egui::Ui, world: &mut World,
            context: &UiContext<'_>, panel: Entity);
}
```

Workbench setup/facade should resolve the unique surface instead of requiring
applications to carry an extra host handle. An unrestricted `&mut World` facade
is exclusive; do not claim it composes with independent Query/Res parameters.
Normal systems can query workspace request components for deferred edits;
WorkspaceRequests remains the application-wide lifecycle request resource.
Do not add an unsafe or misleading facade merely to hide this restriction.

## Implementation sequence

1. Add plain UI drawing/context/component APIs and migrate surface/status draws.
2. Replace workbench drawing systems with plain callbacks, explicit shell state,
   and first-class panel rendering; migrate bEd2 and drawing fixtures.
3. Add automatic/default and explicit surface/camera/shell configuration.
4. Add single-area presentation and user interaction configuration.
5. Update docs and run UI2/workbench unit tests, feature checks, and native GUI
   regressions (especially titlebar gestures/ownership and a plain single panel).

## Session constraints and baseline

- Do not spawn subagents; current developer instructions forbid unsolicited
  delegation. Preserve all existing uncommitted edits.
- At start, Cargo.lock, bed2 manifests/docs/main/workbench_demo, UI2 README,
  workbench manifests/docs/lib/frontend/native/macos_background were modified;
  bed2/src/titlebar_smoke.rs was untracked. These contain prior authorized work.
- Previous baseline: 87 workbench unit tests, ui/native feature checks, and
  `cargo run --offline -p bed2 -- --titlebar-smoke-dir ...` passed.
- Inspect applicable AGENTS.md, prefer explicit execution/ownership, and record
  concrete progress below. Continue implementation across compactions.

## Progress

- Plan recorded before implementation. Repository inspected; implementation is
  beginning with the UI2 rendering contract.

- UI2 library compiles with plain UiDraw callbacks and UiContext/Component props.
- Workbench draws and panel factories are ordinary calls. PanelInstance is the
  ECS metadata; Panel is the public drawing trait (including type metadata).
- WorkbenchShell now lives on the window. WorkbenchSurface auto-wires theme,
  root renderer and camera; manual WorkbenchCamera/UiCamera/theme are retained.
- bEd2 setup now tags WorkbenchSurface and registers its titlebar directly;
  custom controls retain their 20-point presentation. Panel drawing migrations
  are in progress. Native ownership lookup follows the shell on the window.
- PanelOptions now covers single-area flush style, tab visibility, and user
  editing/creation/closure policy. UserEdit requests distinguish user actions
  from application edits. Need finish compile/tests/docs/native validation.

- Validation passed: 25 UI2 tests and 90 workbench tests; native titlebar smoke
  passed including ownership handoff and single-area flush/hidden-tab geometry.
- Smoke also confirms user split/close rejection and application creation of a
  second panel under single-panel user policy. Shell lives on the window;
  register_titlebar(world, height, draw) resolves it automatically.
- Remaining: final formatting, feature checks, workspace/gallery native smoke,
  docs audit and review. No rendering-specific SystemId/IntoSystem remains in
  UI2, frontend or panel factory paths. Preserve scheduled command/input systems.

- Full tests passed: 2 bEd2 + 25 UI2 + 91 workbench, plus doc tests.
- Feature checks passed for headless, ui-only, and native-only workbench.
- Native workspace smoke passed with --ui-first --egui-menu, covering switching,
  rendering, docked tools, background suspension and process cleanup.
- Standalone UI2 gallery smoke passed with 17 captures.
- Review fixes: hidden tabs retain enabled empty-area interactions; preserve
  InvocationContext::Panel public name; gallery state now stays in its component
  (no per-frame removal/re-addition), shell Frontend lives explicitly inside
  WorkspaceUi; bEd2 Notes composes a Component with borrowed props.
- Final tests are being rerun after those state-lifetime improvements. Need final
  native reruns of gallery/workspace/titlebar on final code and formatting audit.

## Final status: implemented and verified

All implementation steps above are complete. The final code passed:

- `cargo test --offline -p bedtime-ui2 -p bedtime-workbench2 -p bed2`: 118 tests
  (25 UI2, 91 workbench, 2 bEd2), plus doc tests. The bEd2 tests were checked again
  after removing its redundant host handle.
- Workbench feature checks with no default features, with `ui`, and with `native`.
- Native titlebar smoke, including native drag isolation, titlebar ownership
  handoff, flush single-area bounds, hidden tabs, user restrictions and
  application-created panels: `target/workbench-composition-titlebar-final`.
- Native workspace smoke with `--ui-first --egui-menu`, including render capture,
  workspace isolation, docked tools, background suspension and process cleanup:
  `target/workbench-composition-workspaces-final`.
- Standalone UI2 gallery smoke, including widgets, themes, overlays, resizing,
  icon replacement and 17 captures: `target/workbench-composition-gallery-final`.
- Changed Rust files pass rustfmt checks; `git diff --check` passes. Workspace
  screenshot was visually inspected. No commits were created.

The application entry point now tags the window with WorkbenchSurface and calls
register_titlebar(world, height, draw). No separate host handle, manual UI camera,
or attach_surface call is needed. Explicit setup remains available. UiFrame is
only an optional convenience wrapper, not a system input. Workbench methods still
operate within a selected workspace; no unrestricted parallel SystemParam was
introduced. UI rendering deliberately remains sequential.

## Scheduling cleanup follow-up

The application no longer wires tool delivery between UI drawing and workspace
dispatch, or scene extraction between target resizing and Bevy camera updates.

- `WorkspaceHostRequest::apply(self, host, source)` and
  `Workbench::request_host(request)` retain the source workspace at submission.
  Each workspace owns its pending queue, backed by Bevy's CommandQueue. Core
  delivers snapshots from all workspaces before queued workspace switches,
  after workspace updates, and before runtime retirement in Last. This includes
  suspended and closing workspaces. Requests preserve per-workspace submission
  order; cross-workspace ordering is unspecified. Requests submitted by a
  delivery handler wait for the next delivery phase. Handlers use
  `Workbench::with_workspace(host, source)` for local state and handle rejection
  if that workspace is already closing.
- bEd2 Gallery/Theme editor submit typed ToolRequest values directly; PanelTools
  no longer owns a request Vec and there is no tool_panels::apply system.
- UI2 owns the PostUpdate order SyncViewports -> PrepareTargets -> Bevy camera
  updates. Applications place scene extraction in UiSystems::SyncViewports.
  Routing before resizing ensures a newly selected image receives its camera
  projection refresh in the same frame.
- bEd2 handles close requests in PostUpdate after workspace Update, with no
  explicit WorkspaceSystems dependency. main.rs has no before/after constraints.
- Preserved the user's concurrent removal of DemoWorkbench.surface; its callers
  now resolve the window through WorkbenchBinding, including smoke fixtures.

Validation: 122 tests pass (26 UI2, 94 workbench, 2 bEd2), including source/FIFO
delivery, queued closure, suspended/directly switched workspaces, Update
requests, late requests before retirement, tool edits with same-frame switching,
and real Bevy camera refresh without waiting for asset events. All 69 headless
workbench tests pass; UI-only and native-only feature configurations compile. Native workspace smoke
passed at target/workbench-ordering-workspaces; native titlebar smoke passed at
target/workbench-ordering-titlebar-retry after one initial missed synthetic
gallery-button click. Changed Rust files pass rustfmt and git diff --check.

## Workspace rendering implementation

The user authorized supporting plugin-created subapps and rendering a model/game
entirely outside the parent world. Implement complete App ownership per workspace,
including plugin lifecycle and subapp extraction/update before change trackers
clear. Preserve direct main-world access and provide explicit App access.

UI2 will own an independent shared-GPU output bridge: a windowless child renderer
uses the host device/queue and renders to a texture exported to host GPU image
assets for egui. No scene entities or mesh/material assets are projected into
the parent world, and no CPU readback is used. Reuse CubeGamePlugin (including
its custom shader) in bEd2 workspaces and in its existing standalone example.
Scene hosting/output is independent of Workbench; panels just display UiViewport.
Retain existing workspace pause, shutdown, titlebar and input behavior.

Implemented:

- WorkspaceRuntime owns a full App. `add_plugins` delegates directly, with no
  temporary App or rejection of subapps. Plugin readiness/finish/cleanup spans
  the complete App. After the staged main frame, each named subapp extracts and
  updates before main trackers clear. Workspace suspension stops those updates;
  retirement drops the entire App.
- `WorkspaceRuntime::{app, app_mut}` expose construction-time configuration;
  `Workspaces::app_mut(id)` exposes a live app and its named subapps. The runtime
  continues to own ticking. Panel callbacks already receive the workspace main
  world and can inspect the game scene there directly.
- New UI2 embedded.rs exports ViewportHostPlugin (included in UiPlugin),
  ViewportOutput and EmbeddedRenderPlugin<C>. ViewportOutput allocates a host
  image identity with MAIN_WORLD usage and registers a weak GPU export. It shares
  the host RenderResources with a windowless child renderer. C marks output
  cameras. Child DefaultPlugins omit Winit/log/panic/Ctrl+C and render pipelining,
  preserve existing TimePlugin, and retain normal assets/render/custom shaders.
- Child cameras target ManualTextureViews of the shared texture. An sRGB render
  view and encoded-color sampling view preserve the existing UI2 texture path.
  Parent extraction publishes snapshots to its GPU image assets after ordinary
  image preparation, and retires stale exports. No scene or mesh/material asset
  mirroring and no CPU pixel transport occurs.
- Host UI observations set output visibility and physical size before child
  updates. Resize replaces the GPU texture and refreshes camera projection;
  hidden outputs stop rendering while simulation follows workspace policy.
- bEd2 now installs the same CubeGamePlugin (including custom grade shader) in
  each workspace. viewport_demo.rs only presents UiViewport; the parent scene
  setup/extract system and duplicated angle state are removed. Main has no
  viewport synchronization registration.
- Added demo-game/examples/embedded.rs: a complete child App fills a UI2 window
  with zero Workbench plugins. The ordinary standalone example still uses the
  same game plugin directly. Both examples share their capture/rotation smoke
  fixture in examples/support/mod.rs. UI2 is a dev dependency only of the game
  example; the game library depends only on Bevy/wgpu.

Current limit: one output per embedded App. Child rendering is sequential; host
render pipelining remains supported. This adds rendering/lifecycle support, not
automatic forwarding of arbitrary game input; existing PanelInput hooks remain
the input integration boundary.

Verification:

- 123 unit tests (26 UI2, 95 workbench, 2 bEd2), including child-subapp readiness,
  finish, extraction, update counts, suspension and public App access.
- 70 headless workbench tests; UI-only/native-only feature checks.
- Native workspace smoke at target/workspace-local-render-final: seven captures,
  scene entities absent from parent, per-workspace RenderApps, animated cube
  pixels, live shrinking/restoration, actual game transform suspension, process
  cleanup and output asset retirement. Resized frame visually inspected.
- Native full-window embedded smoke without Workbench at
  target/workspace-render-embedded-fullwindow, and normal standalone game smoke
  at target/workspace-render-standalone-final, both check pixels and rotation.
- Native gallery smoke at target/workspace-render-gallery-final: 17 captures.
- Native titlebar smoke passed at target/workspace-render-titlebar-debug after
  two runs missed a synthetic close-button click. Improved its assertion failure
  detail; no production input changes were made for these retries.
- No commits created. All prior user edits, including removal of the redundant
  DemoWorkbench.surface field, remain intact.

## Shared workspace configuration

The user approved `WorkspaceAppExt::configure_workspaces` on the host App. A
host plugin can install host resources/systems normally, then register ordinary
App configuration for every workspace, including the default Untitled one and
those opened later. No separate workspace plugin framework or implicit resource
sharing is introduced.

Implemented an ordered list of repeatable `Fn(&mut App) + Send + Sync + 'static`
callbacks. Each runtime applies the list once before plugin finish/cleanup and
Startup, after its own pre-open configuration, identity and built-in host-service
handles have been installed. Initial document data, renderer provisioning and
explicit service sharing remain application-owned. Use ordinary workspace
Startup systems when creating content that needs shared installation first.

Registration is accepted before or after WorkbenchCorePlugin, then frozen by the
first Workspaces::open or host PreStartup, whichever comes first. Late calls
panic clearly. The default runtime still exists immediately for core access;
its configuration/startup are deferred to host PreStartup. Async plugins can
delay readiness until later frames without repeating configuration. Dispatch
now skips a pending active runtime, preserving the no-frame-before-startup rule.

bEd registers workbench_demo::configure on its host: this installs common panel
types, session/update systems, menus and status contributions. Its runtime
factory still installs each renderer and explicit PanelTools handles, with an
ordinary Startup system creating that runtime's initial panels. Tool-panel unit
tests also use the shared configuration API.

New tests protect default and future workspace installation from a host plugin,
registration order through plugin finish/cleanup/startup, host Startup observing
the initialized default, local resources and independent updates, per-runtime
data supplied before callbacks, async readiness with one-time configuration, and
late registration rejection at both boundaries. Current suites: 128 tests across
workbench/UI2/bEd and 75 headless workbench tests pass.

UI-only and native-only feature checks also pass. Native bEd workspace smoke
passed at target/workspace-configuration-smoke with UI-first registration and
egui menus: seven captures, independent renderers, animation, resize, workspace
switch/closure, docked tools, background suspension and process cleanup. The
existing screenshot readiness retry handled initially blank after-close frames.
Changed Rust files pass rustfmt; git diff --check passes. No commits created.

## Workspace-owned titlebars

The user approved moving titlebar content into workspaces. This supersedes the
earlier host-titlebar registration and drawing descriptions above.

- `Workbench::{register_titlebar(height, draw), titlebar(), remove_titlebar()}`
  operate on the selected workspace. The old free registration/removal functions
  and explicit host argument are removed. Each workspace owns one optional
  renderer entity with Titlebar (height), UiView and arbitrary application state,
  linked beneath its local shell. Replacement/removal retires that renderer;
  workspace closure drops its state with the App.
- The host renders the active workspace's titlebar in the workspace world,
  with workspace/renderer egui identity and a local UiContext.view. Switching
  selects its renderer/height automatically; inactive state survives. A missing
  renderer reserves only native chrome. Drawing waits for workspace plugin
  initialization and Startup, including titlebar/menu/status/panel callbacks.
- Window geometry, native controls and padding, window drag/double-click,
  appearance and TitlebarOwnership remain on the host WorkbenchShell.
  Application ownership suspends workspace titlebars without deleting state.
- `WorkspaceList` is a host-published read-only snapshot available in workspace
  resources. `entries()` reports WorkspaceInfo metadata. The host publishes it
  before initialization (including delayed Startup) and before drawing, to all
  runtimes. Workspaces::list and the snapshot share one metadata computation.
  `WorkspaceRequest` implements WorkspaceHostRequest, allowing direct
  `Workbench::request_host(WorkspaceRequest::Activate/Close)` submission. Host
  dispatch remains the lifecycle owner and runs these after drawing.
- bEd registers its 20-point compact titlebar in shared configure_workspaces.
  Layout and panel controls operate on the local world; workspace selectors use
  WorkspaceList and host requests. Application-specific NewWorkspace provisions
  the renderer/tool handles on the host when delivered, then queues activation.
  Main attaches the surface explicitly for immediate Startup binding and uses
  an ordinary host Update system to publish tool observations for the next frame.
- Updated native titlebar smoke reads controls/state from workspace entities,
  exercises local renderer ownership and verifies switching between different
  heights and a workspace with no titlebar, plus the existing native gestures,
  input isolation, docked tools, drag/drop and native ownership handoff checks.
- Tests cover default/common registration, per-runtime Startup replacement,
  local state isolation/preservation and closure cleanup, plus workspace-list
  publication and lifecycle requests without direct host access. A delayed
  Startup test protects current metadata publication after async plugin readiness.

Verification: 131 workbench/UI2/bEd tests; 77 headless tests; UI-only and
native-only feature checks; native titlebar smoke at
target/workspace-titlebar-local-smoke, and seven-capture rendered workspace
smoke at target/workspace-titlebar-render-smoke using composed plugins/UI-first.
Final native-menu titlebar smoke also passed at target/workspace-titlebar-local-final.
All changed Rust files pass rustfmt; git diff --check passes. No commits created.


## Single-world workspace migration (implemented and verified; supersedes child-world ownership)

The user authorized replacing workspace Apps with ordinary workspace entities in
one main application World. InWorkspace is a linked-spawn Bevy relationship;
ActiveWorkspace marks the selected root, InActiveWorkspace its members.
WorkspaceRunning/InRunningWorkspace express active/background update policy.
Lifecycle observers collect changed roots/members; one sync operation reconciles
the derived markers from final state before UI/input and ordinary Update systems.
Game/plugin isolation and the independent UI2 embedded renderer remain optional
application responsibilities. Do not spawn agents. Preserve earlier dirty work.
A source snapshot before this migration is /tmp/bed-single-world-baseline/source.tar.

Current changes:
- workspaces.rs replaced App runtimes/configuration/bridges with entity lifecycle,
  request dispatch and cooperative shutdown. Workspace, WorkspaceUi, WorkspaceWork,
  and command/layout request queues are components; Workbench stores selected ID.
- Panel factories/command definitions are registered once in the main App. Context
  invocations retain originating workspace. Layouts/areas/panels own InWorkspace;
  facade mutations reject foreign workspace targets.
- Frontend draws the actual World, native menus use active-member markers, viewport
  proxy entities and host request forwarding are removed.
- bEd app/game converted to shared rendering. CubeGamePlugin installs rendering
  and simulation; spawn_scene/setup create ordinary standalone-compatible game
  entities. The demo owns per-workspace scene roots, render layers, image targets
  and running policy. Game entities do not depend on workbench components.
- Tool panels directly borrow application UI/theme/clipboard state; PanelTools and
  tool request forwarding removed. Status/titlebar state stays workspace-owned.
- New semantic workspace tests replace obsolete App/plugin lifecycle tests; existing
  command/layout/input/geometry/menu tests are being migrated. Process/task tests
  retained. bEd smoke now checks main-world scenes and scoped state.

Final behavior and verification:

- Workbench never creates Apps/SubApps or renderers. Global command/panel type
  definitions install once; workspace state/queues/workers are components.
  Old runtime configuration, world lookup/forwarding, list snapshots, tool
  snapshots and viewport proxy entities are removed from production code/docs.
- Active/running markers have private constructors. Observers collect affected
  members; reconciliation uses current state and is robust to deferred transfers,
  removals, despawns and several switches. Application systems use ordinary queries.
- Panel/layout facade operations reject foreign workspace targets. Free panel
  creation resolves the requested layout/area's owner even while inactive.
  Menu snapshots and popup requests retain their workspace and focused panel.
  Queued structural edits preserve FIFO order across the UI focus-admission phase.
- The frontend resets presentation at the start of actual drawing, so changing
  layouts hides old panels before input routing. It reads the real window shell
  for permissions and passes per-frame geometry explicitly; WorkspaceUi no longer
  stores a copied window descriptor, focus state or redundant shell entity.
- Native titlebar fixture now injects a matching scripted egui/router pointer
  stream after native input conversion. Real desktop cursor/focus changes cannot
  interrupt synthetic clicks/dragging. Production input handling is unchanged.
- `cargo test --offline -p bedtime-workbench2 -p bedtime-ui2 -p bed2`: 120 tests
  pass (93 workbench, 26 UI2, 1 bEd2), plus doc tests. Obsolete App/plugin-lifecycle
  tests were replaced with entity ownership, scoped updates, marker lifecycle,
  command origin and cooperative shutdown tests. Process/task coverage retained.
- Headless workbench: 68 tests pass. UI-only/native-only feature checks and both
  game examples compile. Rustfmt checks and git diff --check pass.
- Final native workspace smoke: target/single-world-workspace-final. Seven
  captures verify main-world scenes, one renderer, visible cube animation,
  resizing, workspace state, docked tools, suspension, output retirement and
  process cleanup. A docked gallery capture was visually inspected.
- Final native-menu titlebar smoke: target/single-world-titlebar-final. Covers
  hit testing, drag isolation, workspace/tool buttons, panel drag, ownership
  handoff, titlebar height switching and single-area layout visibility/permissions.
- Standalone UI2 gallery: target/single-world-gallery-final, seventeen captures.
- Standalone game and independent embedded game: target/single-world-standalone-smoke
  and target/single-world-embedded-smoke, both verify pixels and rotation.
- No commits created; all prior user work retained. A transient concurrent edit
  to main.rs resolved before final validation; it was not overwritten.

## Component-driven surface setup (implemented and verified)

- WorkbenchSurface insertion is the setup API. Removed attach_surface and the
  duplicate WorkbenchBinding resource; consumers query the component entities.
- Required components provide UiSurface, WorkbenchShell and a transparent
  UiBackground immediately, preserving supplied values. The private SurfaceRoot
  records the registered view so repeated setup cannot duplicate it.
- One PreUpdate setup system runs before UiSystems::Configure. Removed the
  PreStartup pass so application Startup can supply its camera before a default
  is generated. Generated entities are available after UI configuration/in Update.
- Explicit conflicting camera targets fail rather than being overwritten. The
  unique surface/camera constraints and window-owned generated entities remain.
- Updated bEd startup, smoke queries, permission lookup and API documentation.
- Verified 122 workbench/UI2/bEd tests, 68 headless tests, UI-only build,
  rustfmt and git diff --check. Native titlebar smoke with UI-first registration
  and composed plugins passed at target/component-setup-titlebar-smoke; the
  captured UI and cube rendering were visually inspected. Multi-window support
  is deferred to the next design discussion. No commits created.

## Explicit workbench camera (follow-up, implemented and verified)

The application now supplies exactly one camera marked WorkbenchCamera whenever
a WorkbenchSurface exists. Removed default camera creation and unmarked UiCamera
selection. The setup system adds UiCamera(surface) to the marked camera when
needed, retaining target validation. Missing and duplicate markers fail clearly.
The application owns camera settings and lifetime; bEd explicitly creates its
Camera2d at render order 100 and links its lifetime to the window with InView.
Updated the setup tests and current API documentation. Verification: 98 workbench
tests and one bEd test pass, UI-only build passes, native titlebar smoke passes at
target/explicit-workbench-camera-smoke. Rustfmt and git diff --check pass.

## egui shortcut consumption (implemented; supersedes earlier input ownership)

The user authorized removing event ownership. Deleted UI2's input.rs and
workbench's input.rs: no InputRouter, RoutedInput, event IDs/claims, owner-held
key/button state, panel captures or register_panel_input API remain. UiContext
and UiFrame no longer borrow an input router. Text edits and viewport widgets
use ordinary egui interaction; panels/applications reading Bevy input own gating.

- Workbench shortcuts.rs collects from the explicit WorkbenchCamera's context
  after Draw and focus observations. consume_shortcut feeds normalized
  BindingInput to the existing headless command resolver. BindingPress has no
  event ID; BindingClaims and input publication phases are removed. Exact
  modifiers, physical key mapping, event order, repeat policy, focused panel
  precedence, conflicts and workspace ownership remain enforced.
- UiCapture is observational per-surface state. Text and modal flags include
  activity before/after the UI pass, protecting Enter ending text focus and
  popup dismissal without a second event-ownership stream. Save remains usable
  while text editing; editing/navigation/clipboard chords remain reserved.
- Workbench camera setup clears egui's default Quit shortcuts so Cmd/Ctrl+Q
  reaches command resolution and the application retains shutdown policy.
- UI2 clipboard input now comes directly from egui raw key events; CopyText
  output still uses the shared Clipboard service. A real CPU UI-pass test checks
  paste, copy and text-focus termination without Bevy event duplication.
- WorkspaceUi.drawing explicitly guards queued activation during rendering,
  replacing the old incidental guard based on a removed router resource.
- Updated tests and current docs, including direct Bevy input responsibility.
  Net Rust change is about 880 fewer lines. No commits created; prior work kept.

Verification: 121 workbench/UI2/bEd tests (98/22/1), 68 headless tests, UI-only and
native-only feature builds, rustfmt and git diff --check pass. Native titlebar
smoke with UI-first/composed plugins passes at target/egui-shortcuts-titlebar-final;
its final assertion verifies Quit executed through the command handler. The
standalone gallery passes seventeen captures at target/egui-shortcuts-gallery-smoke.
Baseline snapshot: /tmp/bed-input-removal-baseline/source.tar.

## Standalone viewport component (implemented and verified)

UiViewport now implements the ordinary UI Component trait with an entity ID as
props. UiViewport::draw fills the supplied egui allocation, records its bounds,
visibility, hover state and physical size, and registers its image directly with
bevy_egui::EguiUserTextures using a weak handle. The component and camera retain
strong image ownership; upstream handles texture ID caching and retirement.
Removed UiFrame::viewport, the runtime image registry/preregistration scan, the
texture map from UiFrame/UiContext, and the unused viewport texture count.
Updated the workbench viewport panel, independent embedded-game example,
fixtures and current UI2 documentation. Camera routing and resize scheduling
retain their existing behavior.

Verification: 123 UI2/workbench/bEd tests pass (24/98/1), including first-draw
registration, stable IDs across resizing, and image/registration retirement
without disrupting application-owned images. Native workspace smoke passes at
target/standalone-viewport-workspace-smoke, including resizing, scene animation
and output retirement. Native embedded-game smoke passes at
target/standalone-viewport-embedded-smoke. Formatting and diff checks pass for
the changed files. Baseline: /tmp/bed-standalone-viewport-before/source.tar.

## Application-owned workspace initialization (implemented and verified)

WorkbenchCorePlugin no longer creates an Untitled workspace. The application
opens its first workspace before workspace-dependent registration, operations
or drawing. The first open selects itself; later opens retain the selection.
The existing active-workspace access point now fails with an instruction to call
Workspaces::open if initialization was omitted. No fallback workspace is created.

bEd explicitly opens Playground before registering shared panels and commands.
Startup populates that workspace and opens Scratch; removed the placeholder
replacement, activation and closure sequence. Updated public documentation and
test fixtures to create their workspaces explicitly. Tests cover creation in
Startup, initial selection, later opens preserving it, missing initialization,
and rejection of closing the final workspace.

Verification: 100 workbench tests and the bEd test pass with default features;
70 headless tests pass. Native workspace smoke passes at
target/explicit-workspace-init-smoke. Native titlebar smoke passes with UI-first
and composed plugins at target/explicit-workspace-init-titlebar-smoke, including
Quit through shortcut consumption. Formatting and diff checks pass for the
changed files. Baseline: /tmp/bed-explicit-workspaces-before/source.tar.

## Minimal bEd2 reset (implemented and verified)

The user requested a clean starting point for iterative development. bEd2 now
contains a single main.rs: Bevy/UI2/Workbench plugin setup, a Startup system
attaching WorkbenchSurface and an explicit WorkbenchCamera, one Main workspace,
one Hello World panel, and a window-close handler. Removed the application demo
and smoke modules, argument parsing, custom titlebar/tools/scenes/background work,
and their direct dependencies. The app has no demo or smoke-test modes.

Updated bEd2 and current foundation documentation, removing obsolete application
smoke commands. The separate bed2-demo-game package remains independent and is
no longer a bEd2 dependency. Library APIs and existing library tests remain intact.
Verification: cargo check --offline -p bed2 passes; cargo run --offline -p bed2
creates the native window and runs without application errors. Stopped the manual
launch with Ctrl+C. Formatting and diff checks pass for changed files. The removed
application source is backed up at /tmp/bed2-minimal-before/source.tar.

## Optional application-owned status renderer (implemented and verified)

WorkspaceUi now starts with no status renderer or status entity. The shell
reserves no bottom row and paints no status separator until the application
calls Workbench::register_status_bar(draw_fn). One UiDraw renderer owns the
row's contents and layout; UiContext::view identifies its workspace-owned
renderer entity. status_bar() returns Option<Entity>; remove_status_bar()
retires the renderer and restores the full panel area. Replacing a renderer
retires its previous state. Workspace switches select their own renderer.

Removed UI2's StatusItem/StatusSide relationship and plugin-contribution system,
including item collapse, per-item tooltip registration, old exports and tests.
Applications compose their own egui widgets/UI2 components and tooltips inside
the provided row. Updated current API documentation. The minimal bEd2 app still
registers no status bar and contains no demo or smoke modes.

Verification: 121 tests pass (20 UI2, 101 workbench). Regression tests cover zero
reserved height without a renderer, adding/removing it without replaying drawing,
inset host bounds, replacement, workspace selection and renderer/state cleanup.
Native bEd2 startup succeeds; stopped the manual launch with Ctrl+C. Formatting
and diff checks pass for changed files. Baseline snapshot:
/tmp/bed-optional-status-bar-before/source.tar.


## Component composition and workspace/layout storage (implemented and verified)

Workspace, Layout, Area and PanelInstance can now be composed through ordinary
Commands. ActiveWorkspace and per-workspace ActiveLayout markers are authoritative;
removed the separate Selection resource and the Workspace active-layout field.
Zero workspaces, zero layouts and empty layouts/areas are valid. The existing
Workbench convenience surface remains available; its strict selection accessors
still require a selection, while operations needing missing areas return errors.
Closing/deleting the final workspace/layout is supported.

Required components supply workspace work/queues/UI state and layout/presentation
state. WorkbenchCamera requires UiCamera and its transitive EguiContext; surface
wiring only resolves the window reference. A single reconciliation implementation
owns cross-entity validation, panel initialization, derived workspace membership,
active/running markers and stale-gesture invalidation. Removed the membership
observer/dirty-set machinery. Application-wide panel registration uses stable
Panel::KEY values and Panel::initialize on already spawned entities, preserving
explicitly supplied state when initializers use entry().or_default().

Added versioned serde WorkspaceSnapshot/LayoutSnapshot/AreaSnapshot/PanelSnapshot
records. capture_workspace/capture_layout produce entity-free data with local
indices; restore_workspace/restore_layout instantiate fresh graphs after full
validation. Unknown types/versions and malformed indices/geometry are rejected
before mutation. replace_layout resets descendants and core data while preserving
the layout root, active marker and application slot components. Returned mappings
identify layouts/areas/panels in snapshot order for application-owned metadata.
WorkspaceChanged coalesces core persisted changes and removals once per frame in
PostUpdate WorkspaceSystems::Notify, including direct component edits. File I/O,
debounce, project data, arbitrary components, active-workspace persistence and
nine-slot template policy remain application-owned.

bEd2 remains a minimal Hello World app. It registers the panel globally, then
composes its graph in a normal Commands startup system without Workbench facade
initialization. No demo/smoke flags or application persistence behavior were added.
Updated current documentation with composition, storage and template examples.

Verification: 133 default-feature UI2/workbench/bEd2 tests pass (20/113/0),
including snapshot round trips, malformed-input atomicity, template resets,
component defaults, empty UI states, direct edits/reparenting and coalesced
notifications. 81 headless workbench tests pass. Native cargo run --offline -p bed2
starts the window and runs without application errors; stopped the manual launch
with Ctrl+C. All-target checks, formatting and diff checks pass. Baseline:
/tmp/bed-component-storage-before/source.tar.
