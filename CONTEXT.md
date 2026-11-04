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
- **`Fit` stays until formats replace it** (see "Formats and sibling
  documents"). It ranks how well each viewer suits a file. Many places open
  paths (the file tree, `+open`, search, dropped files, LSP, git), so the
  file-to-viewer choice has to be central, and made per viewer, not per
  model: CSV and Rust both load as `TextBuffer`, but CSV should open in the
  table viewer.
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

- **Formats are registered by plugins,** with their extensions and what they
  conform to, as in macOS's Uniform Type Identifiers:
  ```rust
  app.register_format(Format::new("csv").extensions(["csv", "tsv"]).conforms_to(TEXT));
  app.register_format(Format::new("svg").extensions(["svg"]).conforms_to([XML, IMAGE]));
  ```
  `DATA` is the root format, which everything conforms to.
- **Viewers register for a model and the formats they handle:**
  ```rust
  app.register_viewer::<TextBuffer, TablePanel>("Table").formats([CSV]);
  app.register_viewer::<TextBuffer, EditorPanel>("Text Editor").formats([TEXT]);
  app.register_viewer::<Bytes, HexPanel>("Hex").formats([DATA]);
  app.set_default_viewer(CSV, EditorPanel);   // app override
  ```
- **The default viewer is the most specific match.** CSV → text → data, so
  Table (exact) beats the editor (one level up), which beats hex (the root).
  The rest appear in "Open With". `Fit`'s levels become distances in the
  format hierarchy; nobody writes fit functions. On a tie (SVG is as close to
  `IMAGE` as to `XML`), the app chooses the default with
  `set_default_viewer`; until it does, the first registered viewer wins.
- **Detecting a file's format:** first the extension, then a look at the
  file's first bytes (for example "it's UTF-8, so text"; that check moves
  out of `text_fit`). An unknown text file (`.poop`) opens as text.
- **A document records its format,** and the format can be changed: "Open
  As…", a setting mapping file patterns to formats (like VS Code's
  `files.associations`), or choosing a format for an untitled document.
  Syntax highlighting and the language-server language ID should read the
  document's format, so one association drives all three.
- **Each model keeps whatever in-memory form makes its edits cheap.** The
  hex, image, glTF and material editors planned for later each choose their
  own structure. Nothing is stored as bytes for the sake of sharing.
- **One document per (source, model).**
  - Opening a source with a model that's already open reuses that document.
    That fixes today's error when opening an open text file as text.
  - Opening it with a different model ("Open With → Text" on a `.gltf`)
    opens a **sibling document**: the same source, decoded as that model.
- **Siblings stay in sync through bytes, in memory.** Not every document has
  a file, so syncing never goes through the disk.
  - **When:** when the changed document goes idle (as autosave does), and
    when focus moves to another panel.
  - **How:** the changed document is encoded to bytes, and each sibling
    receives them through the same path as an outside change to the source
    (`session.incoming`). Siblings reload, or raise a conflict, exactly as if
    another app had saved the file.
  - **Invalid content stays local.** If a sibling can't decode the bytes
    (half-typed JSON for a glTF), it keeps its last good state and shows that
    the other view has errors.
  - **Undo is per document.** Undoing in one view produces new content,
    which syncs to the others as one more change. Sharing an undo history
    across models would need a common change type.
  - **Conflicts:** editing one sibling while another has unsynced changes
    syncs the other first. If both have really diverged, the existing
    conflict handling applies.
  - **Cost:** one encode and decode per sync point, only while siblings are
    open. Single-view use pays nothing.
- **Models need a byte codec to have siblings.** `FileModel`'s `decode` and
  `encode` already provide one, but it lives in bedtime-filesystem and is
  tied to files. Move the codec to a trait in bedtime-document that doesn't
  require a file; `FileModel` builds on it. A model without a byte form (a
  live SQL table) simply can't have siblings.
- **Adapters, later and only if needed:** where one model really is a view
  of another (bytes over text), a shared-storage adapter could replace
  syncing, for example to give hex and text a shared undo history.
- **Format changes:** changing to a format with the same model only updates
  the document's format. Changing to one with a different model opens a
  sibling.

## Menus

- **Plugins add their own menu items by default.** Workbench defines the
  standard menus (File, Edit, View, Window, Help), and plugins add to them.
  Within a menu, items follow plugin order, with optional groups separated by
  a line.
- **The app can opt out** with a field on the plugin, such as
  `FooPlugin { menu: false }`, and can place the same actions wherever it
  wants.
- **Items enabled only while a panel is focused** already work. A menu item
  is enabled when its action is available, and an action handled by a
  panel's `ActionHandler` is only available while that panel is focused.
- **Menu items and whole menus can belong to a panel**, and workbench adds or
  removes them automatically, in both the egui snapshot and the native menu
  bar:
  - `ForPanel::of::<P>()`: shown while a panel of type `P` is open in the
    active layout.
  - `ForPanel::instance(panel)`: a relationship to one panel. It's created
    with the panel, goes away with it, and is shown while that panel is
    focused.

  Keys are unaffected: shortcuts belong to actions, which are only available
  in the same context anyway.
- Keys stay opt-in (a suggested keymap) while menu items are opt-out. Two
  plugins binding the same key is a silent conflict; menu items can't
  collide.

## Other decisions

- **Settings:** each plugin declares its settings, namespaced, with defaults.
  An app overrides a default by registering the same setting again,
  identified by type. A `settings_panel!` macro lays out the panel: sections,
  grouping, order. Leaving settings out of the panel is fine (an app may
  hard-code some). Keep saved keys explicit (namespace and name), never
  derived from type names. Settings move to the crate that owns the resource
  they write: appearance settings to bedtime-ui or workbench, editor settings
  to bedtime-code.
- **Status bar:** the left side is for notifications, which any plugin can
  post, including ongoing work such as "indexing…". The right side belongs to
  the app; plugins can export widgets for it.
- **Keybindings:** plugins provide handlers but don't bind app-level keys.
  The app binds them with a macro that works with settings. Keys inside a
  panel (an editor's cursor movement, paste) stay with the panel. Plugins may
  export a suggested keymap that the app opts into.
- **New:** a workbench action that goes to the focused panel's handler
  first, then to the app's default.
- **Moves:**
  - The LSP panel goes to bedtime-code; the theme editor to bedtime-ui.
  - The dock menu becomes `set_dock_menu([actions])` in workbench's native
    module.
  - `+open` goes to the defaults layer.
  - Workspaces stay required (they provide the working folder), with a
    single-store mode for apps that don't need a separate workspace settings
    layer.

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
4. **Formats and sibling documents:** `register_format` and format detection
   replace `Fit`; viewers register for formats; documents record their
   format (highlighting and LSP read it); one document per (source, model);
   the byte codec moves to bedtime-document; siblings sync in memory on idle
   and on focus change.
5. Settings declared by the crates that own them; the generic settings panel.
6. Status bar contributions.
7. Route `New` through the focused panel.
8. Move the LSP panel and theme editor.
9. Single-store workspaces.
10. Plugin menu items (including `ForPanel`), the dock menu, and the
    standard menus.

Do one step at a time, then review it before starting the next.

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
