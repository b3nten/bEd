# bedtime-workbench

One editing session per Bevy World, with optional native and egui presentation.
Add WorkbenchPlugins and UI's UiPlugin after the application's normal Bevy setup,
in either order. Applications compose concrete layout and panel entities;
an empty workbench is valid. Workbench never installs
DefaultPlugins or creates a window or GPU device. Layouts,
panels and the window's chrome are bedtime-ui's
[`layouts`](../bedtime-ui/README.md#layouts), re-exported; workbench installs
`LayoutsPlugin` and adds documents to them: drafts block closing their panels,
closing a panel that would lose unsaved work asks first (a close hook), and
tabs of unsaved documents show it (`PanelModified`).

| Plugin | Responsibility |
| --- | --- |
| WorkbenchCorePlugins | Group without windows: persistence and settings (`persistence` feature), work, actions, layouts, documents |
| DocumentsPlugin | Documents, unsaved work, closing and quitting, notifications; checks at startup that the parts it needs are installed |
| ClipboardPlugin | Shared text/file clipboard, usable independently |
| WorkbenchNativeWindowPlugin | Window close requests into the quit flow |
| ChromePlugin (bedtime-ui) | Native window chrome: titlebar, background, blur |
| NativeMenuPlugin (bedtime-menus) | Native presentation of the world menu |
| WorkbenchNativePlugin | Group of native window/chrome/menu plugins |
| WorkbenchUiPlugin | The unsaved-changes prompt, in the modal layer |
| WorkbenchPlugins | `WorkbenchCorePlugins` plus clipboard, native and UI, by feature |
| WorkspacesPlugin (`workspaces` feature) | Workspaces; added by the app, which says how to rebuild itself |

Default features are ui, native and clipboard. With defaults disabled, the core
compiles without a renderer, window backend, native menu library or OS clipboard.
Features control compiled dependencies; plugins control installed systems.
Both groups work like Bevy's `DefaultPlugins`: replace a part with `.set()`,
drop one with `.disable()`.

~~~rust,ignore
app.add_plugins((WorkbenchPlugins, UiPlugin));
// Headless (a server, tests):
app.add_plugins(WorkbenchCorePlugins);
~~~

## Workspaces (`workspaces` feature)

A workspace is a folder with its own store (`WorkbenchStore`, under
`<config>/workspaces/<hash>/`) and settings layer, over the app-wide
`GlobalStore` (`<config>/global/`, with the global settings layer and the
recent list). There's always one: the folder the app was opened at, or the
default workspace (the home folder). A lock stops two instances saving the
same workspace.

~~~rust,ignore
fn app(start: Start) -> App {
    let workspaces = WorkspacesPlugin::new(start, config_directory, app);
    let title = workspaces.workspace().name(); // for the window
    let mut app = App::new();
    app.add_plugins((/* DefaultPlugins with the title */, UiPlugin, WorkbenchPlugins, workspaces));
    app
}
~~~

`WorkspacesPlugin::new` reads the global store (the recent list, whether to
reopen the last workspace: the `ReopenLastWorkspace` setting), chooses and
claims the workspace, and remembers it. Adding it installs the stores and
layers, and the `Workspace`, `Root` and `RecentWorkspaces` resources.
`workspaces::switch(world, start)` ends the session the way quitting does
(`request_exit`), then rebuilds the App in the same window with the function
the plugin was given; that's `bevy::winit::relaunch`, from the bevy_winit fork,
so apps using this feature need the same `[patch.crates-io]` entry.

`Root` is the folder plugins work in (a file tree's start, a terminal's
directory). The workspaces plugin sets it; an app without workspaces sets its
own or leaves it unset.


On macOS with `ui`, the native window adapter also supplies live file-drag
coordinates. Winit 0.30.13's `HoveredFile`/`DroppedFile` events carry paths but no
pointer movement, so ordinary egui mouse position can be stale during Finder
drags. The adapter samples the AppKit content-view position on hover frames and
the final drop frame, converts to Bevy logical coordinates (including scale-factor
overrides), and emits ordinary cursor messages before egui input processing. It
requests redraws while hovering and retires hover state on cancellation/drop or
window closure. Drop-only automation injection retains its supplied cursor; it is
not a Finder-drag test.

When using native window integration, configure Bevy WindowPlugin with
close_when_requested: false. Workbench handles `CloseRequested` itself; see
[Closing and quitting](#closing-and-quitting).

## Operations

Workbench operations are methods on two views of the `World`, from
`WorkbenchWorldExt`:

```rust,ignore
use bedtime_workbench::WorkbenchWorldExt;

// Reads: a `&WorkbenchRef`.
let layout = world.workbench().active_layout();
let ids = world.workbench().document_ids();

// Changes: a `&mut Workbench`, which also has every read.
let workbench = world.workbench_mut();
workbench.focus_panel(panel)?;
workbench.save_document(id)?;
```

Changes that have to keep the workbench's rules (membership, persistence,
unsaved work, shutdown) go through `Workbench`; plain reads stay on their
resources and components (`Settings::get`, `Layout::focused_area`). There are no
free functions taking a `World`. A system queues a closure, which Bevy runs as a
command:

```rust,ignore
commands.queue(move |world: &mut World| {
    let _ = world.workbench_mut().save_document(id);
});
```

## Application-owned sessions

The host owns project/session admission, application storage and process services. Document sources own document I/O.
Workbench provides composition, checked structural transitions and optional
[reflected persistence](#optional-reflected-persistence). It has no panel factory
catalog or built-in project/window manager. See the
[world/session decision](../../../MIGRATION/workbench-world-session-plan.md).

## Closing and quitting

Workbench owns document lifetime, autosave and the unsaved-changes prompt, so
applications don't implement them.

- **Lifetime.** `DocumentReference` is a Bevy relationship from any entity to a
  document. On a panel it is the document the panel shows; on other entities (a
  child of a panel showing several documents, a plugin's holder) it just keeps
  the document open. Once a document has been referenced, losing its last
  reference releases it at the end of the frame (replacing a reference within
  one frame keeps it). Unsaved documents are only released once saved or
  discarded; otherwise quit asks about them. Documents never referenced are left
  to whoever opened them.
- **Closing panels** (closing a tab or area, or merging areas, via
  `Workbench::request_panel_close` or user edits) happens immediately unless it
  would release an unsaved document. Closing one of several views never asks.
- **Autosave.** Set the `DocumentAutosave` resource (default `Never`). With
  autosave on, a close that would release unsaved documents saves them and
  closes once the saves succeed.
- **Prompt.** With autosave off, or when a save fails, workbench asks: Save
  (Retry after a failure), Discard or Cancel. A failed save never closes
  anything. `close_prompt` and `resolve_close` drive this headlessly; the `ui`
  feature draws it.
- **Quit.** `CloseRequested` asks the same about every unsaved document, waits
  for saves, shuts down managed work, then sends `AppExit`. Hook `AppExit` to
  flush application state.
- **Other exits.** `Workbench::request_exit(name, then)` ends the session the
  same way but runs `then` instead of sending `AppExit`; bed switches workspace
  with it. `name` titles the prompt ("Switch workspace with unsaved
  changes?") and is `ClosePrompt::exit`. Cancel leaves the session running.
  One exit at a time; a quit during another exit is ignored.

Raw despawn of panels bypasses the prompt; it is not a user-facing close.

## Documents

A document is an entity owning one source's content, undo history and what was
last saved. There is **one document per source**: opening the same source again
returns the same document. Models come from
[`bedtime-document`](../bedtime-document/README.md) (`DocumentModel`, with
`TextBuffer` for text); sources implement `DocumentSource<Model>`.

```rust,ignore
let document = world
    .workbench_mut()
    .open_document(MemorySource::new("Notes", TextBuffer::from("hello")))?;
// Loads on a worker; world.workbench().inspect_document(document.id())?.status says when it's Ready.
world.spawn((NotesViewer, DocumentReference::new(document.id()), InArea(area)));
```

- **Editing.** `edit_document` / `edit_document_transaction` take the revision
  the caller saw, and apply all of their changes or none. Undo groups merge
  consecutive edits from one holder. `document_changes_since` lists recent
  commits, so views can update positions instead of rebuilding.
- **Dirty** means the content differs from what was last loaded or saved, not
  that edits happened: undoing back to the saved content is clean.
- **One source task at a time.** Each document runs either a read or a save,
  never both, so saving and outside changes can't race. A save writes a
  snapshot and edits made meanwhile stay unsaved. A read never starts during a
  save, and a read still running when a save starts is dropped.
- **Outside changes** are picked up on `refresh_document`, or periodically for
  sources that report `check_interval` and a cheap `version`. A clean document
  takes the change as an undoable edit. A dirty one gets a conflict, which
  blocks saving until `resolve_document_conflict` (keep local, accept incoming,
  or a merged model).
- **Failures.** A failed save keeps everything, records `DocumentInfo::error`
  and schedules a read, so a save refused because the source changed
  (`SourceError::Changed`) becomes a conflict. A source that disappears marks the
  document `missing`; its content is kept and saving is refused. A failed first
  load gives `DocumentStatus::Failed`; `refresh_document` tries again.
- **Drafts.** `DocumentDraft` on a holder (e.g. an IME composition in progress)
  blocks saving, undo, outside changes and closing until removed.
- `close_document` / `discard_document` close a document and the panels showing
  it. Usually you don't call them: documents are released with their last
  reference (see [Closing and quitting](#closing-and-quitting)).

## Notifications

`Workbench::notify(Notification::error(text))` tells the user something that
needs no answer: a setting or layout that couldn't be saved, an action whose
handler failed. Each is logged and kept in the `Notifications` resource (the
last 50; a repeat of the latest refreshes its time instead of adding another).
Workbench doesn't draw them; the application shows `Notifications::latest()`,
in a status bar say. Workbench sends its own store and settings failures here,
and every action handler's `Err`. An action that isn't available (a shortcut
pressed where it does nothing) isn't a failure and isn't reported. Anything
that could lose work, such as a document that won't save, asks with a dialog
instead.

## Execution and external work

Relationship hooks keep the layout graph consistent as it changes; debug builds
also validate it once per frame (in `Last`), panicking on a broken graph.
Background work is [`bedtime-work`](../bedtime-work/README.md) (`ManagedWork`,
re-exported), polled in `Last`. Quitting through `CloseRequested` performs an
orderly exit; headless hosts and tests can instead call `shutdown_work()` and
keep updating until `work_finished()`. New document and work admissions are
rejected after shutdown.

## Actions, handlers and keybindings

Actions are [`bedtime-actions`](../bedtime-actions/README.md), re-exported.
Workbench makes `ActionFocus` the focused panel, so a panel's
`ActionHandler<A>` wins while it's focused, and shows a handler's error as a
notification (an action that isn't available isn't an error). Document
actions (Save, Undo, Redo) are workbench's own.

## Menus

Menus are [`bedtime-menus`](../bedtime-menus/README.md), re-exported (its
egui drawing is under `bedtime_workbench::ui`). Workbench draws the menu bar
set with `set_menu` inset by `WorkbenchSurface::panel_gap`, and installs the
native menu bar with `WorkbenchNativePlugin`.

## Persistence (`persistence` feature)

Saving is [`bedtime-persistence`](../bedtime-persistence/README.md), re-exported
as `bedtime_workbench::persistence`: stores, the world snapshot, and persisted
resources. Workbench defines `WorkbenchStore` and saves the world snapshot in
it; the app decides where it lives by giving it a backend.

```rust,ignore
use bedtime_workbench::persistence::{DirectoryStore, ReflectPersist, PersistenceAppExt, WorkbenchStore};

#[derive(Component, Reflect, Default)]
#[reflect(Component, Persist)]
#[require(PanelInstance = PanelInstance::of::<Self>(), NotesRuntime)]
struct Notes { text: String }

app.register_type::<Notes>()
   .add_store::<WorkbenchStore>(DirectoryStore::new(config_dir));
```

- **Panels.** Save a panel's concrete component; its `PanelInstance` and runtime
  state come back through `#[require]` and hooks. A panel with nothing persisted
  is left out of the save. `PanelInstance` itself isn't saved (titles come from
  `Panel::LABEL` or are set at runtime).
- **Restoring.** Layouts are only restored into a workbench that has none.
  Which tab was selected and which area focused are put back afterwards, and a
  restore that leaves the workbench invalid is undone.
- **Failure.** Store failures become notifications.
- Data that belongs somewhere else (shared by every workspace, say) is a
  persisted resource in another store: `app.persist::<RecentFiles, GlobalStore>()`.
- **Layout templates.** `capture_layout(layout)` saves one layout's
  areas and panels as a `LayoutTemplate` (RON text, reflectable, so an app can
  keep templates in a persisted resource). `instantiate_layout(&template)`
  builds a new, inactive layout from it, after the existing ones; failure adds
  nothing. A panel that refers to an entity outside the layout (a document,
  say) belongs to one workspace's data, so it's left out of templates.

## Settings (`persistence` feature)

Settings are [`bedtime-settings`](../bedtime-settings/README.md), re-exported as
`bedtime_workbench::settings`: typed values resolved through layers kept in
stores, and (with the `ui` feature) the pieces of a settings panel. Workbench
installs its plugin and shows settings problems as notifications.
Keybindings are settings too (`Keybinding<A>`); `keys_in`, `keys_at`,
`set_keys_in` and `keys_overridden_above` read and write them by action id.
