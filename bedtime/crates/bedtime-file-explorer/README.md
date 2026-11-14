# bedtime-file-explorer

Workbench file explorer rooted at `Root`, shown as a tree or in columns.
Opens files through `open_path`. (It was `bedtime-filetree`; the panel still
saves as `bedtime_filetree::FileTree`, so older layouts open.)

```rust,ignore
app.add_plugins(FileExplorerPlugin);
world.spawn((FileExplorer::default(), InArea(area)));
```

- **Views.** `FileExplorer::mode` (`ViewMode`): `Tree`, `Columns`, or `Auto`
  (the default), which shows columns when the panel is wider than it is
  tall and the tree otherwise. The mode is saved
  with the panel; the footer, each menu's View As, and the `files.view_tree`,
  `files.view_columns` and `files.view_auto` actions set it.
- **Columns.** A column per folder from the root down to the selection
  (`column_folders`, `columns`), each listing its entries; the folder that's
  open in the next column is marked. Clicking a folder selects it (its
  entries fill the next column); clicking a file previews it, as in the tree.
  Up/Down, Home/End and Page Up/Down move within a column; Right goes into a
  folder, Left back out to it; Enter opens a file or goes into a folder;
  typing selects by name in the column. Menus, file operations, dragging,
  dropping and tooltips work as in the tree; a column's empty space is its
  folder. The columns read their own folders, not the tree's open ones.
- **One selection.** Both views share the selection. Switching to the tree
  opens the folders down to it; the columns follow it anyway.
- **Footer.** The folders down to the selection's (clicking one selects it;
  what doesn't fit goes from the root down), and the mode as a switch, or in
  a narrow panel a menu.

- **Root.** The tree shows the `Root` resource (a workspace's folder), resolved
  to its canonical path. Without one it says no folder is open. Every tree in
  the world shows the same root.
- **Opening.** (In the tree; the columns differ as above.) A click on a file opens a preview of it (`open_path` with
  `preview: true`), so clicking through files reuses one tab. Files open
  somewhere other than the explorer (`Placement::From`): the largest other
  area, else the explorer's area split in two. The tree keeps
  focus, so arrow keys keep browsing. Double-click, Enter or the context
  menu's Open pins the tab. Viewers are whatever the app registered
  (`register_viewer`). Errors, including a file nothing opens, produce
  notifications.
- **Open With.** A file's context menu lists `viewers_for(path)`, worked out
  when the menu opens. The menu also has Copy Path and Refresh.
- **File operations.** The context menu has New File…, New Folder… (in that
  folder, or the file's), Rename…, Duplicate, Move to Trash, Copy, Cut and
  Paste; the empty space below the rows has a menu for the root. A name is
  typed in a box under its row (a rename selects the name without its
  extension); Enter does it, Escape or clicking away doesn't. Operations run
  `bedtime-filesystem`'s `execute` on a `ManagedWork` task (`FileOperations`,
  `run_file_operation`): they never replace an existing file, moved and
  trashed files take their open documents along, and viewers' saved files
  (`ViewerFile`) follow moves. When one finishes, what it made is selected
  (its folders opened), a new file opens, and errors become notifications.
  Copy and Cut also put the files on the system clipboard; Paste takes files
  from there too. Cut files move once.
- **Dragging.** Drag a file onto the layout to open it there: in an area,
  beside a tab, or in a new split (a `PanelFactory` that calls `open_path`
  with that area). Drag files or folders onto a folder in the tree (or a
  file in it, or the empty space for the root) to move them there; holding
  Alt/Option or Cmd/Ctrl copies. The folder is outlined; dropping where it
  already is, or into itself, does nothing.
- **From outside.** Files dragged in from Finder onto a folder (or the empty
  space, for the root) are copied there (holding Alt/Option or Cmd/Ctrl moves
  them); dropped elsewhere, the layout opens them (`open_files_factory`).
- **Keys.** Up/Down, Home/End and Page Up/Down move the selection; Right opens
  a folder or moves into it, Left closes it or moves to its parent; Enter
  opens; typing selects the next row starting with what was typed. F2
  renames, Cmd/Ctrl+Backspace or Delete trashes, and Copy, Cut and Paste
  work on the selected row.
- **Hidden and ignored files.** Hidden files (dot files, and those the system
  marks hidden) and files Git ignores (its `.gitignore`s, `.git/info/exclude`
  and global excludes, read with the `ignore` crate when a folder is read;
  only on the local filesystem; what's in an ignored folder is ignored) are
  shown dimmed; hidden files aren't, by default. Each tree's menus have Show Hidden Files and Show Ignored
  Files, also the `files.toggle_hidden` and `files.toggle_ignored` actions;
  trees that haven't chosen follow `FileExplorerDefaults`, which the app sets from
  its settings. A changed `.gitignore` rereads every open folder.
- **Moving.** When rows change, they move (`motion.rs`, bedtime-ui's
  `RowMotion` and `ease`): an opened folder's rows slide out from under it
  and fade in, a closed folder's slide back under it and fade out, and the
  rows below slide to their new places. The first listing doesn't move.
- **Saved.** The mode, which folders are open in the tree, and what an explorer hides, is saved with the panel (feature
  `persistence`, default), relative to the root. The root starts open.

## Reading and watching

`Folders` (a resource shared by every explorer) keeps a listing of each
folder open in a tree or shown as a column, read on a `ManagedWork` task, and
watches each one
(`bedtime-filesystem`'s `DirectoryWatcher`, nonrecursive). A change reported in
a folder, or to an entry in it, reads that folder again; the previous listing
stays on screen until the new one arrives, and a failed reread keeps it. If
watching can't start, every open folder is reread every two seconds and the
tree says it isn't watching. Closed folders are let go (and unwatched); at most
512 folders are kept. Listings sort folders first, then by name ignoring case.
Symbolic links are rows, never followed into.

Drawing does no file I/O except `viewers_for` when a context menu opens. Rows
are worked out again only when a listing or the open folders change. What the
user asks for while the tree draws (open, toggle, refresh, operations) is applied in
`Update` (`FileExplorerSystems`), since opening a file can close the preview panel
being drawn.

## Not yet

Unsupported: multiple selection (operations take one row), operation progress,
operation undo and per-file hiding.

## Tests

```sh
cargo test -p bedtime-file-explorer --all-features
python3 tools/bed.py run tests/automation/files.py tests/automation/files_hidden.py \
    tests/automation/files_columns.py
```
