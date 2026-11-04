# bedtime-workbench

One editing session per Bevy World, with optional native and egui presentation.
Install `WorkbenchPlugins` and `UiPlugin` after Bevy setup, in either order.
Applications create layouts and panels; an empty workbench is valid. Workbench
does not install `DefaultPlugins` or create windows or GPU devices.

Layouts, panels and chrome are re-exported from
[bedtime-ui](../bedtime-ui/README.md#layouts). Workbench installs `LayoutsPlugin`
and integrates documents: drafts block panel closure, unsaved work triggers a
close prompt, and `PanelModified` marks unsaved tabs.

| Plugin | Responsibility |
| --- | --- |
| WorkbenchCorePlugins | Group without windows: persistence and settings (`persistence` feature), work, actions, layouts, documents |
| DocumentsPlugin | Documents, unsaved work, closing and quitting, notifications; checks at startup that the parts it needs are installed |
| ClipboardPlugin | Shared text/file clipboard, usable independently |
| WorkbenchNativeWindowPlugin | Window close requests into the quit flow |
| ChromePlugin (bedtime-ui) | Native window chrome: titlebar, background, blur |
| NativeMenuPlugin | Native presentation of the world menu |
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
recent list). Each session has a folder workspace or the default workspace (home folder).
A lock prevents concurrent instances from saving the same workspace.

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

`Root` supplies plugins' working directory. `WorkspacesPlugin` sets it;
applications without workspaces may set it or leave it unset.


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
catalog or built-in project/window manager.

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
last saved. There is **one document per source and model**: opening the same
source as the same model again returns the same document. The same file as
another model (an SVG as text and as an image) is a sibling document: each
keeps its own model and undo, and their content passes between them in memory
as bytes (`ByteModel`) once the edited one goes idle or the focus moves (see
`documents/siblings.rs`). A sibling that can't decode the bytes keeps its
content and shows why; if two siblings were both changed, nothing passes and
the user is told; a save is shared by siblings holding the same content.
Opening a file gives its document the codec (`set_byte_codec`). Models come from
[`bedtime-document`](../bedtime-document/README.md) (`DocumentModel`, with
`TextBuffer` for text); sources implement `DocumentSource<Model>`.

```rust,ignore
let document = world
    .workbench_mut()
    .load_document(MemorySource::new("Notes", TextBuffer::from("hello")))?;
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

## Appearance (`ui` and `persistence` features)

`appearance::AppearancePlugin` (in `WorkbenchPlugins`) applies the appearance
settings to every `LayoutSurface`: the theme (`ColorTheme`, an id in
bedtime-ui's `ThemeFolder`), the font and size, the window's opacity and blur,
and the panels' opacity, corner radius and gap. `ThemesPlugin { folder,
shipped, default }` is an app's themes: it writes the shipped themes missing
from the folder, makes the folder the `ThemeFolder`, and sets the default
theme. `ThemeEditorPlugin` (opt-in) adds the Theme Editor panel
(`appearance.show_theme_editor`): duplicate, rename, recolor, reset a
shipped theme, saved to its file as you edit.

## Status bar (`ui` feature)

`status::body` is a window body (`SurfaceBody(status::body)`): the panels,
with a bar along the bottom. Its left side is the workbench's: what's going
on (`StatusActivity`: a plugin sets what it's doing, `set("git",
Some("fetching"))`, and clears it after; shown with a spinner), then the
latest notification for a few seconds (hover for recent ones). Its right
side is the app's: the items it adds with `app.add_status_item(item)`, the
first added rightmost. Plugins export items for it, such as
`workspaces::status_item` (the workspace's name). `notify_failure(doing,
result)` turns an error into a notification.

## Opening

`open_document(source, OpenOptions)` shows a document in a viewer panel with
the model's `OpenState` (`TextOpenState`: a cursor, a selection). A viewer is
an ordinary panel that shows a document through `DocumentReference`;
registering it pairs the panel type with its model and a label, and that's
what makes it a viewer:

```rust,ignore
app.register_viewer::<TextBuffer, EditorPanel>("Text Editor").formats([TEXT])
   .register_viewer::<Bytes, HexPanel>("Hex").formats([DATA])
   .register_default_viewer::<EditorPanel>();

world.workbench_mut().open_document(source, OpenOptions::at(state))?;
world.workbench_mut().open_path(&path, PathOptions { line: Some((41, 0)), ..default() })?;
commands.open_document(handle, OpenOptions::at(state)); // errors become notifications
```

- A new viewer is `P::default()` with a `DocumentReference`, placed with
  `Place`. The state reaches it as `Reveal<M::OpenState>` (never for models
  whose `OpenState` is `()`): the viewer reads and removes it once its content
  is loaded, and a later open replaces it. Spawning a viewer yourself works
  too: `(ImagePanel::default(), DocumentReference::of(handle), Place::auto())`.
  A `DocumentReference` on a viewer of a different model logs an error.

- `Target` says which panel: `Existing` (default) reuses a viewer already
  showing the document in the current layout (the focused area's, else the
  first), `Panel(panel)` names one (it must show the document), and `New`
  always makes one. Opening never switches layouts.
- `place` says where a new panel goes, as a `Placement`. Without one it goes
  where documents go: an area with this viewer's panels (the focused one,
  else the most recently focused), else an area with any viewer's, else the
  largest, so opening from a file tree's sidebar doesn't land in the sidebar.
  `Placement::Beside(panel)` puts it with panels of its type in another area,
  else splits `panel`'s area (a preview beside its editor).
- `keep_focus` leaves the layout's focus where it is: a file tree's or Git
  panel's preview, or a preview beside the editor you're typing in. The panel
  is still its area's selected tab.
- `preview: true` makes a new panel replace its area's preview. Opening
  without `preview` pins an existing preview, and so does editing its document.
- With the `files` feature (default), files have formats (`formats`
  module). A plugin registers its formats with the names and extensions
  they're known by and what they conform to (`register_format(Format::new(CSV)
  .extensions(["csv"]).conforms_to([TEXT]))`), and viewers the formats they
  show (`.formats([CSV])`, for a `FileModel`). `open_path` detects the file's
  format (by name, extension, a detector, then text or not), or takes
  `PathOptions::format` ("Open As"), and opens it with the requested viewer,
  else the format's default (`set_default_viewer::<P>(format)`; the latest
  call wins), else the viewer whose format is closest in steps of
  conformance, the first registered among equals. It records the format on
  the document (`DocumentFormat`, `document_format`). `viewers_for(path)`
  lists the viewers that can open a file ("Open With"), and
  `viewers_for_document` those for an open document. A file nothing opens is
  an error, which the caller reports.
- A viewer opened from a file keeps the path with the panel (`ViewerFile`,
  saved with persistence). A restored panel with a path but no document gets
  it back from the viewer it is (`reopen_viewers`, in `Update`); one whose
  file can't be opened is closed, with a notification.
- **Untitled documents.** `new_untitled::<M>(options)` opens an empty
  "Untitled" (or "Untitled 2", …) document, marked `Untitled`. Saving one
  (Save, or the close prompt's Save) asks the app where, with the function
  given to `set_save_as` (a file picker); while it's open the document counts
  as saving, so closing waits. The app answers `save_untitled_to(id, path)`
  (it's then that file's, history and all; its viewers get a `ViewerFile`)
  or `cancel_save_as(id)`. Without a picker, saving one is an error.
- A panel is the viewer of its own type only (`PanelInstance::kind`). One
  built from a viewer's parts, like a diff panel that requires an
  `EditorPanel`, isn't taken for that viewer: opening doesn't reuse it,
  restoring doesn't reopen it (it does that itself), and it titles itself.
- `viewer_document::<M>(ui, world, panel)` is how a viewer's `draw` starts: it
  draws a spinner while the document loads, or why it couldn't be read, and
  otherwise returns its model (viewers' models are cheap to clone), handle and
  revision.
- Every panel showing a document is titled after it each `Update`, so tabs
  (even behind others) follow a renamed or moved file.

## Execution and external work

Relationship hooks keep the layout graph consistent as it changes; debug builds
also validate it once per frame (in `Last`), panicking on a broken graph.
Background work is the [`work`](docs/work.md) module (`ManagedWork`,
re-exported), polled in `Last`. Quitting through `CloseRequested` performs an
orderly exit; headless hosts and tests can instead call `shutdown_work()` and
keep updating until `work_finished()`. New document and work admissions are
rejected after shutdown.

## Actions, handlers and keybindings

Actions are the [`actions`](docs/actions.md) module, re-exported at the root.
Workbench makes `ActionFocus` the focused panel, so a panel's
`ActionHandler<A>` wins while it's focused, and shows a handler's error as a
notification (an action that isn't available isn't an error). Document
actions (Save, Undo, Redo) are workbench's own, and so are the focused
panel's: Close Panel (Cmd/Ctrl+W, through the close checks as its tab's ×),
and Zoom In, Zoom Out and Reset Zoom (Cmd/Ctrl+=, - and 0), which step its
`PanelZoom`. A panel with a zoom of its own (an image, a model) gives itself
an `ActionHandler` for each. A plugin's "show this panel" action can use
`Handler::new(show_panel::<P>)`: it focuses the `P` in the active layout, else
opens a new one (`focus_or_open`).

`New` (`document.new`, Cmd/Ctrl+N) runs the focused panel's own handler
when it has one (a terminal makes another terminal: an `ActionHandler<New>`
it requires), else the app's: `app.set_new_document::<TextBuffer>()` makes
an untitled document beside the focused document, else where documents open.

## Menus

Menus are the [`menus`](docs/menus.md) module, re-exported at the root (its
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

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Persist)]
#[require(NotesRuntime)]
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

Settings are the [`settings`](docs/settings.md) module,
`bedtime_workbench::settings`: typed values resolved through layers kept in
stores, and (with the `ui` feature) the pieces of a settings panel. Workbench
installs its plugin and shows settings problems as notifications.
Keybindings are settings too (`Keybinding<A>`); `keys_in`, `keys_at`,
`set_keys_in` and `keys_overridden_above` read and write them by action id.
