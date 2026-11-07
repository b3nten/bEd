# Context

Working notes for the current direction of bed/bedtime. Keep this current;
delete what's done and no longer useful.

## Goal: compose apps from plugins

Building a bedtime app should mostly mean adding plugins and setting a few
options, instead of writing glue. Installing a viewer plugin should be enough
for its files to open, and adding a panel plugin should make that panel
openable. Apps can still override any default.

Principles:

- **Plugins provide the pieces; the app decides the arrangement.**
  - Plugins provide panels, action handlers, settings (with defaults),
    notifications, status widgets, and default menu items for their own
    actions.
  - The app decides keybindings, the right side of the status bar, and the
    settings panel's layout. It can turn off or rearrange a plugin's menu
    items.
  - A defaults plugin group gives a working arrangement out of the box. Opt
    out of any part with `.disable::<…>()`, as with Bevy's `DefaultPlugins`.
- **Few concepts:**
  - Workspace → Layout → Area → Panel, as entities and relationships.
  - Documents, linked to panels by `DocumentReference`.
  - `Place`: where a new panel goes.
  - Actions: how anything is invoked.
  - Viewer registration: a lookup table, not a trait.
- **Components underneath, calls as shortcuts.** Anything an API call does
  can also be done by spawning or inserting components yourself.
- **Keep ordering knobs to a minimum.** Following plugin order is fine, as
  long as the result is deterministic.

## Crate structure

Three tiers:

1. **bedtime-ui**: egui, theming, fonts, icons, layouts, areas, panels, and
   panel placement.
2. **bedtime-workbench**: desktop-app features (documents, viewers, actions
   and keys, menus, settings, background work, persistence wiring,
   workspaces, clipboard, native integration). Apps should only need
   `UiPlugin` and `WorkbenchPlugins`.
3. **Plugins**: code, terminal, file tree, viewers, git, search, etc.

Some crates stay separate, hidden behind workbench and re-exported by it:

- `bedtime-document` and `bedtime-filesystem`: they don't depend on Bevy.
  That keeps ECS out of the models and their tests fast.
- `bedtime-persistence`: it sits below bedtime-ui, which uses it for layouts.
- `bedtime-clipboard`: adapted from Bevy 0.20's `bevy_clipboard`, so it can
  be swapped for the upstream crate later.

The `bedtime` crate re-exports these and will provide the defaults group.
Cargo features cover compile cost; runtime opt-outs work like Bevy's plugin
groups. A plugin that can be turned off must leave the others working: they
check for its resources instead of assuming them.

## Panels (bedtime-ui)

A panel is an ordinary entity: a `Panel` component plus whatever state
components it needs. **The `Panel` component is what a panel is; state
components and widgets are reusable parts.** One panel can be built from
another's parts. For example, the diff panel requires an `Editor` state
component and draws with `draw_editor`, without being an editor panel.

- **`#[bedtime_ui::panel]` attribute macro** (crate `bedtime-ui-macros`). It
  adds `#[derive(Component)]` and
  `#[require(PanelInstance = PanelInstance::of::<Self>())]`. An attribute
  macro is needed because a separate derive can't add `#[require]` for Bevy's
  `Component` derive. Bevy merges several `#[require]` attributes, so a
  panel's own requires stay next to it. It takes no arguments: `Panel` (label
  and `draw`) is still implemented by hand, since a macro can't implement half
  of a trait. All panels use it.
  ```rust
  #[bedtime_ui::panel]
  #[derive(Reflect, Default)]
  #[require(TreeView)]
  pub struct FileTree { … }
  ```
- **A panel's type is recorded at runtime.** `PanelInstance::of::<P>()` keeps
  `P`'s `TypeId`. That is what "like panels" and "is a viewer" compare, so a
  panel built from another panel's parts is never mistaken for that panel.
- **`Place` decides where a new panel goes:** `Place { at: Placement, focus: bool }`.
  - `Placement` is an enum: `Auto`, `Largest`, `In(area)`, `Beside(panel)`.
    (`LikePanels` was dropped: it would be the same as `Auto`.)
  - `Beside(panel)`: another area of `panel`'s layout holding panels of the
    same type (the focused one, else the most recently focused), else
    `panel`'s area split in half (the new panel on the right), else, too
    small to split, `panel`'s own area. `layouts().area_holding(layout, fits,
    except)` is the shared "area with panels like this" search.
  - `Auto` means: an area that already holds a panel of this type (the
    focused one, else the most recently focused), otherwise the largest area.
  - `Place`'s insert hook resolves it to an area by the next flush: it picks
    the area in the active layout, inserts `InArea`, focuses the panel if
    `focus` is set, and removes `Place`.
  - It uses a hook, not a system, so code that spawns a panel and then acts
    on it (focusing it, splitting its area) never sees it without an area.
  - Placing a new panel with `focus: false` still makes it the selected tab
    in its area (the `InArea` hook does that). Only the layout's focus stays
    where it was.
  ```rust
  commands.spawn((GitPanel, Place::auto()));
  commands.spawn((TerminalPanel::new(options), Place::in_area(area).focus(false)));
  ```
  - A new panel that can't be placed is despawned with an error logged (a
    panel can't exist outside an area). A placed panel that can't be moved
    stays where it is.
- **`InArea` stays, read-only to users.** It's a Bevy relationship and
  records where `Place` ended up. Tab order (`AreaPanels`), despawn cleanup,
  spawning panels together with their area, and entity remapping when a saved
  layout loads all come from the relationship. That's why `Place` doesn't
  absorb it: relationships must be single-entity structs, so an enum can't
  be one, and one component meaning both "request" and "resolved" would
  complicate every query. Users write `Place` (including `Placement::In(area)`)
  and read `InArea`. Inserting `Place` on a panel that's already placed moves
  it. Read-only is a convention for now, not enforced. Document opening uses
  `Place` since 2b; still inserting `InArea` directly: layouts' own code and
  `PanelFactory`, restored layouts, and the bedtime-ui example (the LSP
  references panel and git's diff panel moved to `Place` in step 3). Move
  those, then make `InArea` constructible only inside bedtime-ui.
- **Opening always creates a panel.** It never focuses an existing one.
  Whether to focus an existing panel instead is the caller's choice, using
  `layouts.find::<P>(layout) -> Option<Entity>`.
- **Whether a panel can be opened right now** is the action's job
  (`Handler::available_when`, which also greys out menu items), not the panel
  layer's.
- **Shortcuts:** `commands.open_panel(bundle)` (`PanelsCommandsExt`) and
  `world.open_panel(bundle) -> Entity` (`PanelsWorldExt`) spawn the bundle
  with `Place::auto()` and focus. `world.focus_or_open::<P>()` focuses the
  first `P` in the active layout, else opens `P::default()`.
- **No layout open:** `LayoutFallback` (a resource holding a function) makes
  one; by default a new "Layout 1". `layouts_mut().ensure_active()` uses it.
  Bed's numbered layouts set it to open layout 1.
- **No generated "Open" actions.** Each plugin defines its own action (no
  keys) and registers a handler; for the usual "focus it, else open it" that
  is `Handler::new(show_panel::<P>)` (workbench). The app binds keys. Moved
  so far: `files.show` (bedtime-filetree), `code.show_structure`
  (bedtime-code), `git.show`, `http_client.show`, `mascots.show_bed`, and
  `terminal.new` (always a new terminal). `terminal.new` still has its
  Ctrl+` default key until the keybinding step. Bed's own panels (Settings,
  Workspaces, Language Servers, Theme Editor) keep their actions in bed until
  they move to their crates.

## Documents and viewers (workbench)

- **The `Viewer` trait goes away.** Panels that show documents are ordinary
  panels. Every current `Viewer::new` ignores its document and returns
  `Self::default()`, and only the editor's `apply` does anything.
- **A panel shows a document** through the `DocumentReference`
  relationship, which also keeps the document open while referenced:
  ```rust
  commands.spawn((ImagePanel::default(), DocumentReference::of(handle), Place::auto()));
  ```
- **A position travels as a component, `Reveal<S>(S)`** (for example
  `Reveal<TextOpenState>`):
  - Opening inserts it on the panel, new or existing. The viewer reads and
    removes it once its content is loaded; the editor takes it in `draw`,
    replacing its `pending` field.
  - Inserting it again replaces it, so the latest jump wins.
  - Not inserted for models whose `OpenState` is `()`, so viewers that can't
    move don't collect stale ones.
  - Code that doesn't go through opening can insert it directly, such as
    "go to line" or a test.
  - bedtime-code's editor had its own `enum Reveal` (how far to scroll); it's
    now `editor::Scroll` (`Minimal`/`Center`).
- **Viewer registration** records the model, the panel type, a label, and
  (optionally) which files the viewer suits. Plugins register their viewers;
  the app can override the default:
  ```rust
  app.register_viewer::<Picture, ImagePanel>("Image").files(fit);
  app.register_default_viewer::<EditorPanel>();
  ```
  `register_viewer` returns a `ViewerRegistration`, which derefs to the
  `App` so building continues; `.files(fit)` returns the `App`. Names that
  said "opener" now say "viewer": `ViewerId`, `OpenOptions::viewer`,
  `PathOptions::viewer`, `viewers_for(path)`, `viewers_for_document`,
  `Viewers` (the resource), bedtime-preview's `preview_viewer`.
- **`Fit` is gone; formats replaced it in 4a** (see "Formats and sibling
  documents"). Many places open paths (the file tree, `+open`, search,
  dropped files, LSP, git), so the file-to-viewer choice is central, and made
  per viewer, not per model: CSV and Rust both load as `TextBuffer`, but CSV
  opens in the table viewer.
- **Type checks:**
  - `DocumentReference::of(DocumentHandle<M>)` builds a reference from a
    typed handle, so document IDs can't be mixed up.
  - An observer on `DocumentReference` inserts logs an error if the panel is
    registered as a viewer for a different model. (A hook can't be used: the
    relationship derive owns the component's hooks.)
- **`HostedViewer` is gone.** "Is a viewer" is decided by the panel type
  recorded in `PanelInstance`: the diff panel requires `EditorPanel`, but its
  `PanelInstance` is the diff panel's, so opening, restoring and titling skip
  it. The planned `Editor` / `EditorPanel` split isn't needed for that, so it
  hasn't been done. Do it only if another reason comes up (the diff panel
  being matched by editor-only systems, say).
- **Opening a document:** focus a registered viewer already showing it;
  otherwise spawn the default viewer (or the one requested) with `Place` and
  a `DocumentReference`. Either way, add `Reveal` if a state was given.
  ```rust
  world.workbench_mut().open_document_handle(handle, OpenOptions::at(state))?; // -> Entity
  commands.open_document(handle, OpenOptions::at(state)); // DocumentsCommandsExt
  ```
  - The model type comes from the handle, so the state's type is checked at
    compile time.
  - The `Commands` version returns nothing and reports errors as
    notifications. Which panel ends up showing the document is only known
    when the command runs, so it can't return one.
  - Options: `OpenOptions { state, target, place, keep_focus, viewer,
    preview }` (and the same fields in `PathOptions`). `Target` only says
    which panel: `Existing`, `Panel(panel)` or `New`. `place:
    Option<Placement>` says where a new one goes; `None` is the documents'
    default (below). `keep_focus` leaves the layout's focus where it is (the
    panel is still its area's selected tab), for existing panels too. The
    builder chain sketched earlier (`.at().with().place()`) wasn't added.
  - **Where documents go by default:** an area with this viewer's panels
    (focused, else most recently focused), else an area with any viewer's
    panels, else the largest area. A file tree isn't a viewer, so opening from
    it lands beside it.
  - With no layout open, `Place`'s fallback makes one (it used to be an
    error).
- **`open_path(path, options)`** stays as the untyped entry point and picks a
  viewer. It returns `Result<Entity, String>`. "Nothing opens this file" is
  an error ("nothing here opens this file"), which callers already report, so
  `set_open_fallback` is gone (bed's "bEd can't open X" notification became
  each caller's "Couldn't open X: …"). Git's diff panel used `Ok(None)` to mean
  "binary"; it now shows the error instead (with hex installed that never
  happens). It turns a line into each model's state with
  `DocumentModel::state_at_line`.
- **Preview** opens with `place: Placement::Beside(editor)` and
  `keep_focus`, and no longer needs the panel entity. Previews now gather
  with previews of the same viewer type only (markdown with markdown); before,
  any preview counted. The file tree's and Git's previews use `keep_focus`
  instead of focusing back by hand.
- **Git's diff panels** spawn with `Place::in_area` (focused when pinned).
  Git keeps its own `diff_area` (beside editors or diffs, else a free area,
  else a 2:3 split of the Git panel's area); `Beside` splits in half, so it
  doesn't replace that.

## Formats and sibling documents (workbench, bedtime-document)

A file's *format* (CSV, Rust, SVG, glTF) is separate from the *model* that
holds it in memory (`TextBuffer`, `GltfModel`). Formats replace `Fit`.

Status: 4a (formats) and 4b (sibling documents, the byte codec) are done. Formats live in workbench's `formats` module (`files` feature), not
bedtime-document: detection reads files and the registry is an app resource.

- **Formats are registered by plugins,** with the names and extensions
  they're known by and what they conform to, as in macOS's Uniform Type
  Identifiers. `FormatId("csv")` is the identity; plugins sharing a format use
  the same id, and registering one again adds names, extensions and parents
  (the preview plugin adds `Cargo.lock` to bedtime-code's `toml`):
  ```rust
  app.register_format(Format::new(CSV).extensions(["csv", "tsv"]).conforms_to([TEXT]));
  app.register_format(Format::new(SVG).extensions(["svg"]).conforms_to([IMAGE, FormatId("xml")]));
  app.register_format_detector(|path| language_name(&path.to_string_lossy()).map(FormatId));
  ```
  `DATA` (the root) and `TEXT` are built in. A format without parents
  conforms to `DATA`.
- **Viewers register for a model and the formats they handle:**
  ```rust
  app.register_viewer::<TextBuffer, TablePanel>("Table").formats([CSV]);
  app.register_viewer::<TextBuffer, EditorPanel>("Text Editor").formats([TEXT]);
  app.register_viewer::<Bytes, HexPanel>("Hex").formats([DATA]);
  app.set_default_viewer::<EditorPanel>(CSV);   // an override
  ```
- **Choosing the viewer:** the requested one if it can show the format,
  else the format's default (`set_default_viewer`, for exactly that format;
  the latest call wins, so an app overrides its plugins), else the closest
  viewer in steps of conformance (CSV → text → data: Table, then the editor,
  then hex), the first registered among equals. The rest appear in "Open
  With" (`viewers_for`), the chosen one first. Nobody writes fit functions.
  - Plugins set defaults for their own formats where the closest viewer
    isn't the one to open: the preview plugin makes the editor the default
    for Markdown, JSON and TOML, so the previews are "Open With" only.
  - No ties in bed's formats: SVG conforms to `[IMAGE, xml]`, so the image
    viewer is 1 step away and the editor 2; `.gltf` conforms to
    `[3d-model, json]` the same way.
  - `register_default_viewer::<P>()` stays, for opens that aren't of a file
    (a document handle, a new untitled document): the model's default.
- **Detecting a file's format:** a format's whole file names (`Cargo.lock`),
  then extensions, then detectors (bedtime-code's finds languages with
  `language_name`), then the first bytes: UTF-8 without NUL is `TEXT`, else
  `DATA`. A detector's format that nobody registered is ignored. A format
  that conforms to text but whose file isn't text counts as `DATA`. An
  unknown text file (`.poop`) opens as text.
- **A document records its format** (`DocumentFormat` on the document
  entity, `workbench().document_format(id)`), set by every path open. "Open
  As" is `PathOptions::format`; there's no UI for it yet, and no
  file-association setting yet.
- **Highlighting and LSP read the format.** Every highlighted language is a
  format (`FormatId("rust")`, conforming to `TEXT`). `document_language`
  takes the language closest to the document's format (SVG's is XML,
  `.gltf`'s JSON). A file opened as plain `TEXT` gets none, and a document
  without a format (untitled) uses its title.
- **Each model keeps whatever in-memory form makes its edits cheap.** The
  hex, image, glTF and material editors planned for later each choose their
  own structure. Nothing is stored as bytes for the sake of sharing.
- **One document per (source, model).**
  - Opening a source with a model that's already open reuses that document.
    `load_document` matches on source and model, which fixed the error when
    opening a file already open as another model.
  - Opening it with a different model ("Open With → Text" on a `.gltf`)
    opens a **sibling document**: the same source, decoded as that model.
- **Siblings stay in sync through bytes, in memory.** Not every document has
  a file, so syncing never goes through the disk.
  Implemented in workbench's `documents/siblings.rs` (`sync_siblings`, in
  `Update`).
  - **When:** the one sibling changed since content last passed between them
    has been idle 300 ms, or the layout's focus moved, or it was just saved
    (so its siblings share the save).
  - **How:** it's encoded and each other sibling *replaces* its content with
    the decoded bytes, as one undo step ("Change in Another View"). That's
    not the outside-change path (`session.incoming`): the receiving sibling
    becomes dirty like its source, since the file doesn't have the change
    yet.
  - **Saves are shared.** When one sibling saves, the others still holding
    the same content take that as their saved state (`adopt_save`, with the
    new file version). So they're clean, autosave doesn't save them again,
    and a later save of theirs expects the right version.
  - **A sibling opened later** takes the unsaved content of one already open.
  - **Invalid content stays local.** A sibling that can't decode the bytes
    (half-typed JSON for a glTF) keeps its content and its document's error
    says "Another view's changes can't be shown here: …".
  - **Undo is per document.** Undoing in one view is a change the others then
    take. Sharing an undo history across models would need a common change
    type.
  - **Both changed:** if two siblings changed since they were last in step,
    nothing passes and the user is told once ("… was changed in two views.
    Save one to bring the other up to date."). Saving one changes the file,
    and the other then gets the existing outside-change conflict.
  - **Cost:** one encode and decode per sync point, only while siblings are
    open. Single-view use pays nothing.
  - Videos (`FileModel::UNREAD`) get no codec: they're read from disk by the
    player and have no byte form to pass.
- **The byte codec is `ByteModel`** (bedtime-document): `decode` and
  `encode`. `FileModel` (bedtime-filesystem, which re-exports `ByteModel`)
  builds on it with the file limits (`MAX_LEN`, `UNREAD`). Opening a file
  gives its document the codec (`set_byte_codec`); a model without a byte
  form (a live SQL table) simply can't have siblings.
- **Adapters, later and only if needed:** where one model really is a view
  of another (bytes over text), a shared-storage adapter could replace
  syncing, for example to give hex and text a shared undo history.
- **Format changes:** changing to a format with the same model only updates
  the document's format. Changing to one with a different model opens a
  sibling.

## Menus

- **Plugins add their own menu items** (done in step 10):
  `StandardMenusPlugin { app_name }` (in `WorkbenchPlugins`) builds the
  standard bar at `PreStartup`: App, File, Edit, View, the app's own menus
  (`StandardMenu::Custom("Layout")`), Window, Help. Plugins call
  `app.add_menu_item::<A>(StandardMenu::View, "panels")`.
  - Each menu has fixed groups in the usual order (App: about, settings,
    quit; File: new, open, save, close; Edit: history, clipboard, find,
    code; View: panels, editor, zoom, tools), separated by lines; within a
    group, plugin order. Unknown groups follow, in first-use order. So
    nothing needs an ordering knob, and bed's File menu keeps its order.
  - `add_menu_entry(menu, group, spawn)` adds something else (bed's Open
    Recent submenu); `add_menu_item_labelled` renames ("Settings…").
  - Opting out: `hide_menu_item::<A>()` (app-level). The per-plugin
    `FooPlugin { menu: false }` field wasn't added: plugins are unit structs
    and `hide_menu_item` covers it per item.
  - Empty top-level menus aren't shown (Window and Help, for now).
  - Workbench adds New, Save, Close Panel, Undo/Redo and the zooms; each
    plugin its own (settings panel, Theme Editor, file tree, code and LSP,
    Language Servers panel, git, HTTP client, search, mascots, terminal);
    bed its Quit, folder items, Open Recent, Open Preview, Workspaces and
    the Layout menu. Bed's `compose_menus` is gone.
- **Items enabled only while a panel is focused** already work. A menu item
  is enabled when its action is available, and an action handled by a
  panel's `ActionHandler` is only available while that panel is focused.
- **Menu items and whole menus can belong to a panel** (done in step 10):
  snapshots leave them out otherwise, so the egui and native bars follow.
  - `ForPanel::of::<P>()`: shown while a panel of type `P` is open in the
    active layout.
  - `ForPanel::instance(panel)`: a relationship to one panel
    (`ForPanelInstance`); despawned with it, shown while it's focused.
  - A hidden item between two separators can leave two lines in a row;
    collapsing them isn't done yet.

  Keys are unaffected: shortcuts belong to actions, which are only available
  in the same context anyway.
- Keys stay opt-in (a suggested keymap) while menu items are opt-out. Two
  plugins binding the same key is a silent conflict; menu items can't
  collide.

## Other decisions

- **Settings** (done in step 5):
  - Each setting belongs to the crate whose behaviour it changes, which
    declares and registers it:
    - workbench `appearance` (font, font size, window opacity and blur,
      panel opacity, radius and gap, applied to every `LayoutSurface` by
      `AppearancePlugin`, in `WorkbenchPlugins`; disable it to style surfaces
      yourself). The theme is `AppearanceSettings::theme`, set by the app;
    - workbench documents: `Autosave`, registered by `DocumentSettingsPlugin`
      (in `WorkbenchPlugins`, not the core group, so an app or test using
      only the core sets `DocumentAutosave` itself). Its default is off;
    - bedtime-code: `WrapLines`, `LineNumbers`; bedtime-filetree:
      `ShowHiddenFiles`, `ShowIgnoredFiles` (both with `persistence`).
  - An app overrides a default with `set_setting_default::<S>(value)`,
    identified by type, before or after registration; the latest call wins.
    Bed turns `Autosave` on this way. Once a plugin registers a setting, the
    resource it writes (`EditorDefaults`, `FileTreeDefaults`) follows the
    settings, so inserting that resource directly is overwritten at
    startup: change the setting's default instead (a file tree test did).
  - `settings_panel! { "Section" { SettingA, SettingB } ... }` builds a
    `SettingsLayout`, set with `set_settings_layout`. With a layout, the panel
    shows exactly its sections and settings in order (left out is hidden);
    without one, declared sections. Settings declared without a section can
    be placed by a layout too.
  - `SettingsPanelPlugin` (opt-in) adds the generic `SettingsPanel` and
    `OpenSettings` (`settings.open`, Cmd/Ctrl+, still bound by default until
    the keybinding step): layer tabs, the layer's location (a
    `SettingsLocation` hook; workspaces provides it, and labels its layer
    "Workspace: <name>"), errors (`Stores::error_in`), the settings, then
    keyboard shortcuts.
  - `settings::limit` is the shared "clamp a hand-edited number" helper.
  - The theme moved in step 8: `ColorTheme` and its row are in workbench
    `appearance`. Bed's screen-effects settings stay in bed.
  - Saved keys didn't change, so existing settings files still apply.
- **Status bar** (done in step 6): workbench's `status` module (`ui`
  feature). `status::body` is a window body: the panels with a bar along the
  bottom.
  - Left side, the workbench's: `StatusActivity` (any plugin sets what it's
    doing by source, with a spinner) then the latest notification.
  - Right side, the app's: `add_status_item(item)`, the first added
    rightmost. Plugins export items: `workspaces::status_item` (name) and
    `bedtime_code::lsp::status::status_item` (the focused editor's server and
    its error and warning counts).
  - Language-server progress ("rust-analyzer: indexing", "loading") moved
    from the right-side item to status activity on the left.
  - `Workbench::notify_failure(doing, result)` replaces bed's helper.
- **Keybindings:** plugins provide handlers but don't bind app-level keys.
  The app binds them with a macro that works with settings. Keys inside a
  panel (an editor's cursor movement, paste) stay with the panel. Plugins may
  export a suggested keymap that the app opts into.
- **New** (done in step 7): workbench's `New` (`document.new`, Cmd/Ctrl+N).
  The focused panel's `ActionHandler<New>` runs if it has one (the terminal
  requires one: another terminal in its area); otherwise the app's, set with
  `app.set_new_document::<M>()`: an untitled `M` beside the focused
  document, else where documents open. Bed's save-as picker (rfd) stays in
  bed's `new.rs`; moving it means a native file-dialog dependency in
  workbench, left for later.
- **Moves:**
  - Done in step 8:
    - The Language Servers panel is in bedtime-code
      (`lsp::LanguageServersPanelPlugin`, `code.show_language_servers`, with
      `persistence`). It's opt-in: it makes the servers a setting that builds
      the `ServerCatalog`, and an app (or a test) without it sets
      `LanguageServers` itself. Its layer labels and notes no longer need
      workspaces (the first layer is global).
    - The theme setting, its row and the Theme Editor are in workbench
      `appearance` (the editor needs settings, so bedtime-ui can't hold it).
      `ThemesPlugin { folder, shipped, default }` installs an app's shipped
      themes, sets `ThemeFolder` and the default theme, and keeps the
      shipped list (`ShippedThemes`) for the editor's Reset.
      `ThemeEditorPlugin` is opt-in (`appearance.show_theme_editor`); the
      theme row offers Edit only when it's installed. Bed keeps only its
      theme files and default (`themes::plugin()`).
  - The Dock menu is workbench's (done in step 10): `set_dock_menu([..])` /
    `add_dock_menu_item::<A>()`, `DockMenuPlugin` in `WorkbenchNativePlugin`
    (macOS; elsewhere a no-op). Items run their actions globally; clicks are
    queued on AppKit's side and submitted in `PreUpdate`. Bed's dock.rs and
    its objc2 dependencies are gone; bed adds `NewWindow`.
  - `+open` goes to the defaults layer (not done; see the end of the plan).
  - Workspaces stay required (they provide the working folder). Done in step
    9: `WorkspacesPlugin::new(..).global_settings_only()` drops the
    workspace settings layer, so every setting is global; each folder keeps
    its own store for its layout. (Folders still hash to their own store
    directory; only settings are shared.)

## Open questions

- **Naming:** `Place`/`Placement`, `Reveal`, `register_viewer`.

## Plan

1. ~~Merge crates into workbench~~ (done).
2. **Panels and documents**, split into two parts reviewed separately:
   - **2a, panels:** `Place` and its hook, the panel type in `PanelInstance`,
     `layouts.find`, `open_panel`, the no-layout fallback, and `#[panel]`.
     Delete bed's `panels.rs` and `PanelPolicy`; move its open handlers to
     `find` plus `open_panel`, inside their plugins where that makes sense.
     Saved keybindings for the old `app.open_*` actions are lost; that's
     accepted.
   - **2b, viewers:** remove `Viewer` and `HostedViewer`; add
     `DocumentReference::of`, `Reveal`, `register_viewer`, the `Editor` /
     `EditorPanel` split, and `open_document` / `open_path` spawning their
     viewers with `Place`. Where documents land doesn't change in this step.
3. Change where documents open by default (prefer areas that already show
   viewers), and add `Placement::Beside` (Preview, git diff).
4. **Formats and sibling documents**, in two parts:
   - ~~**4a, formats:** `register_format` and format detection replace `Fit`;
     viewers register for formats; documents record their format
     (highlighting and LSP read it).~~ (done)
   - ~~**4b, siblings:** one document per (source, model); the byte codec
     moves to bedtime-document; siblings sync in memory on idle and on focus
     change.~~ (done)
5. ~~Settings declared by the crates that own them; the generic settings panel.~~ (done)
6. ~~Status bar contributions.~~ (done)
7. ~~Route `New` through the focused panel.~~ (done)
8. ~~Move the LSP panel and theme editor.~~ (done)
9. ~~Single-store workspaces.~~ (done: `global_settings_only`)
10. ~~Plugin menu items (including `ForPanel`), the dock menu, and the
    standard menus.~~ (done)

Do one step at a time, then review it before starting the next.

## What's left

The ten steps are done (2026-11-04, overnight; one commit per step or part).
Not done, or only partly, roughly in order of value:

- **A `bedtime::DefaultPlugins` group.** The goal's "install a handful of
  plugins" isn't packaged yet: an app still adds `UiPlugin`,
  `WorkbenchPlugins`, the opt-in panels (`SettingsPanelPlugin`,
  `ThemeEditorPlugin`, `LanguageServersPanelPlugin`) and its viewer plugins
  one by one, sets `SurfaceBody(status::body)` and adds status items. A
  defaults group in the `bedtime` crate could do all that.
- **Keybindings.** Plugins still declare `DEFAULT_KEYS` (`terminal.new`,
  `settings.open`, the code plugin's, workbench's own). The decided design
  (the app binds keys with a macro that works with settings; plugins may
  export a suggested keymap) isn't built.
- **`+open` in the defaults layer.** It's still bed's (`terminal.rs`), since
  it needs workspace switching.
- **The save-as picker** (rfd) is still bed's `new.rs`; moving it means a
  native file-dialog dependency in workbench.
- **"Open As" UI and file associations.** `PathOptions::format` exists; no
  menu or setting uses it.
- **Siblings changed in both views** are only reported; there's no merge.
- **`InArea` read-only:** `PanelFactory`, restored layouts and the bedtime-ui
  example still insert it directly.
- **Menus:** separators around hidden `ForPanel` items aren't collapsed;
  Window and Help have nothing yet; there's no per-plugin `menu: false`
  field (`hide_menu_item` per action instead).
- **The `.at().with().place()` open builder** wasn't added (`OpenOptions` and
  `PathOptions` do the job).
- **The `Editor` / `EditorPanel` split** wasn't needed (panel-type identity
  covers the diff panel).

bed is one file now (2026-11-06): `bed/main.rs` (about 2,700 lines) has
`main` and `app` first, then each former module as an inline `mod` under a
boxed header (settings, themes, menus, layouts, workspace, files, new,
preview, terminal, git, search, mascots, effects). Notes above that name
`bed/<module>.rs` mean that module's section. `bed/themes/` (the theme files),
`bed/assets/` and `bed/effects.wesl` (the shader) stay separate files.

GUI check (2026-11-04, after step 10): `python3 tools/bed.py run` passes 28
of 29 scripts. `theme_editor.py` fails the same way on `45c8734` (before
this work): the accent field it clicks is only partly scrolled into view
above the status bar, so the typed color never lands. Pre-existing; the
script needs to scroll further.

Renamed action keys (saved custom bindings for these are lost, as agreed):
`app.open_files` → `files.show`, `app.open_structure` → `code.show_structure`,
`app.open_git` → `git.show`, `app.open_http_client` → `http_client.show`,
`app.open_bed` → `mascots.show_bed`, `app.new_terminal` → `terminal.new`,
`app.open_settings` → `settings.open`, `app.new` → `document.new`,
`app.open_language_servers` → `code.show_language_servers`,
`app.open_theme_editor` → `appearance.show_theme_editor`. The automation
scripts in `tests/automation` use the new keys.

## Workspaces in place (second plan, 2026-11-06)

Setup before the App was brittle: `WorkspacesPlugin::new(start, config, app)`
ran before `App::new()` because switching rebuilt the App (`relaunch`, from the
bevy_winit fork) and needed the `app` function and a `Start`. Decided: switch
workspaces inside the running world instead.

- **Persistence directory, set once:** `PersistencePlugin::for_app("bed")`
  (`$BED_CONFIG_DIR`, else the usual config folder) or
  `PersistencePlugin::directory(path)`; without one nothing is saved. It's the
  `PersistenceDirectory` resource: `global/`, `workspaces/<hash>/` and the
  themes folder are under it.
- **A workspace lifecycle in workbench, not behind `workspaces`:** close
  (save, despawn workspace entities, plugins reset their resources) and open
  (point the workspace store and settings layer at the folder, restore its
  layout, plugins' per-workspace setup). Open runs once at startup after
  stores load; without the `workspaces` feature close never runs. Plugins
  write against these instead of `Startup` and never ask whether workspaces
  exist. Workspace state lives on panels or resets on close.
- **`WorkspacesPlugin` without arguments:** it reads the folder from the
  command line; switching is a request (`open_workspace`) that asks about
  unsaved work like quitting. The window title follows `Workspace`. `Start`,
  `WorkspaceSettings::app`, `relaunch` and the bevy_winit fork go.
- **The terminal's `+name` helpers** stay the one check before an App is
  built (they must not open a window). Maybe `bedtime::run(app)` later.

Steps, one at a time:

1. ~~The persistence directory (`PersistencePlugin`), read by workspaces,
   themes and bed's Workspaces panel. `WorkspacesPlugin` picks its workspace
   in `build`.~~ (done)
2. The lifecycle schedules; bed's `open_first` and per-workspace setup and
   `GitSession` move onto them.
3. Switching in place: remove `Start`, `relaunch` and the fork. A GUI script
   switches with a terminal, Git and an editor open and checks nothing
   carries over, then switches back.
4. Maybe `bedtime::run`.

## Log

- 2026-11-03: step 1 done (`6935794`, fast-forwarded onto `main`). actions,
  menus, settings and work are modules of bedtime-workbench, re-exported at
  the root as before (`settings::settings!` still works). actions' `settings`
  feature became workbench's `persistence`. Their READMEs are in
  `bedtime-workbench/docs/`. The `bedtime` facade dropped the `actions`,
  `menus`, `settings` and `work` features (now part of `workbench`). bed
  needed no changes. Pre-existing and left alone:
  `--no-default-features --features persistence` fails to build workbench
  tests (a test panel has no `draw` without `ui`), and `cargo fmt --check` is
  not clean repo-wide.
- 2026-11-04: step 2 designed (panels, `Place`, viewers as registration
  only, plugin menu items). Not started. Decided to keep `InArea`, add
  `ForPanel::of` / `ForPanel::instance`, and replace `Fit` with formats plus
  sibling documents synced in memory (step 4).
- 2026-11-04: step 2a done. `Place`/`Placement` and their hook,
  `PanelInstance::kind`, `layouts().find`, `open_panel`, `focus_or_open`,
  `LayoutFallback` and `ensure_active` in bedtime-ui (`layouts/placement.rs`,
  with tests). New `bedtime-ui-macros` crate for `#[bedtime_ui::panel]`, used
  by every panel. Workbench's `show_panel::<P>` handler. Plugins now own
  their "show" actions (keys renamed from `app.open_*`; the automation tests
  are updated). Bed lost `panels.rs`, `PanelPolicy`, `http_client.rs` and its
  duplicate open actions.
- 2026-11-04: step 2b done. `Viewer`, `Opener`, `HostedViewer` and
  `set_open_fallback` are gone; `register_viewer::<M, P>` / `Viewers` /
  `ViewerId`, `Reveal<S>`, `DocumentReference::of`, the model-mismatch
  observer and `Commands::open_document` replace them. New viewers spawn with
  `Place`. `open_path` returns `Result<Entity, String>`. Every viewer crate
  migrated; the editor reads `Reveal`. Not done: the `Editor` / `EditorPanel`
  split (not needed), the open builder, enforcing `InArea`. Noticed:
  `work::tests::tasks_are_cancelled_with_their_owner_and_by_shutdown` is
  flaky under the full parallel test run (passes alone); unrelated.
- 2026-11-04: step 3 done. `Placement::Beside` and `layouts().area_holding`
  in bedtime-ui. `Target` is now `Existing | Panel | New`; open options gained
  `place` and `keep_focus`. New documents go to an area with this viewer,
  else any viewer, else the largest. Bed's preview, the file tree's and Git's
  previews, `New`, git's diff panels and the LSP references panel moved onto
  `Place` / `keep_focus`.
- 2026-11-04: step 4a done. Workbench `formats` module: `Format`, `FormatId`,
  `DATA`/`TEXT`, `Formats` (detection, conformance distance),
  `register_format` (merging), `register_format_detector`,
  `set_default_viewer::<P>(format)`, `DocumentFormat`. Viewers register with
  `.formats([..])`; `Fit` and `text_fit` are gone. `PathOptions::format` is
  "Open As"; `workbench().format_of(path)`. Every viewer crate registers its
  formats (audio, font, image + SVG, 3D model + glTF/GLB/STL, video, CSV,
  Markdown/JSON/TOML); bedtime-code registers a format per language plus a
  detector, and highlighting, Structure and LSP use `document_language`. Bed
  has a test of which viewer each kind of file opens with.
- 2026-11-04: step 4b done. `ByteModel` in bedtime-document; `FileModel`
  builds on it (every model's impl split in two). Documents are keyed by
  source and model. `documents/siblings.rs` passes content between siblings
  in memory (idle, focus move, save), shares saves, catches up a sibling
  opened later, and reports both-changed. Store gained the codec and sync
  bookkeeping. Tests cover both directions, a later sibling, undecodable
  content, both changed, and a shared save.
- 2026-11-04: step 5 done. Settings defaults can be overridden
  (`set_setting_default`); `settings_panel!` / `SettingsLayout` lays out the
  panel; generic `SettingsPanel` + `OpenSettings` + `SettingsLocation` in
  workbench (`SettingsPanelPlugin`); workbench `appearance` module and
  `AppearancePlugin`; `Autosave` in workbench documents; editor and file tree
  settings in their crates. Bed's settings.rs is its theme setting, its
  defaults and its layout. `app.open_settings` is now `settings.open`
  (automation scripts updated). Workbench gained a `system-fonts` feature.
- 2026-11-04: step 6 done. Workbench `status` module (body, `StatusActivity`,
  `StatusItems`/`add_status_item`), `notify_failure`, workspaces' status item;
  bedtime-code's `lsp::status` (item and activity report). Bed's status.rs is
  gone; main adds the two items.
- 2026-11-04: step 7 done. `New` + `set_new_document` (workbench, `files`
  feature), the terminal's own New handler; bed's `New` action is gone
  (`app.new` is now `document.new`). Also fixed a dead-code warning from 4a
  without the `files` feature.
- 2026-11-04: step 8 done. Language Servers panel moved to bedtime-code
  (opt-in plugin); Theme Editor, theme setting and row, `ThemesPlugin`,
  `ShippedThemes` moved to workbench `appearance`. Bed lost
  language_servers.rs and theme_editor.rs; themes.rs is its theme files.
  Action keys: `code.show_language_servers`, `appearance.show_theme_editor`.
- 2026-11-04: step 9 done. `WorkspacesPlugin::global_settings_only()`, with
  a test that a folder has two settings layers by default and one with it.
- 2026-11-04: step 10a done (`ForPanel`), 10b done (standard menus and
  contributions; bed's items moved into its plugins), 10c done (the Dock
  menu in workbench native). The plan is done; see "What's left".
- 2026-11-06: workspaces-in-place step 1 done. `PersistencePlugin` has a
  `directory` (`::for_app("bed")` reads `$BED_CONFIG_DIR`, `::directory(p)`;
  default none) and inserts `PersistenceDirectory`. `WorkspacesPlugin::new(start,
  app)` no longer takes the config folder and chooses its workspace in
  `build` (add it after `PersistencePlugin`); `WorkspaceSettings` only holds
  `app` now, and `WorkspacesPlugin::workspace()` is gone. `ThemesPlugin`'s
  `folder` may be relative to the persistence directory (bed: `"themes"`).
  bed calls `config_directory` nowhere.
