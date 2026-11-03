# bedtime-filetree

A file tree panel for workbench apps: the folders under the workbench's
`Root`, opening files with `open_path`.

```rust,ignore
app.add_plugins(FileTreePlugin);
world.spawn((FileTree::default(), InArea(area)));
```

- **Root.** The tree shows the `Root` resource (a workspace's folder), resolved
  to its canonical path. Without one it says no folder is open. Every tree in
  the world shows the same root.
- **Opening.** A click on a file opens a preview of it (`open_path` with
  `preview: true`), so clicking through files reuses one tab. The tree keeps
  focus, so arrow keys keep browsing. A double click, Enter or the context
  menu's Open opens it for good. Which viewer opens it, and what happens when
  nothing can, is the app's (`register_opener`, `set_open_fallback`); the tree
  reports errors as notifications.
- **Open With.** A file's context menu lists `openers_for(path)`, worked out
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
  shown dimmed. Each tree's menus have Show Hidden Files and Show Ignored
  Files, also the `files.toggle_hidden` and `files.toggle_ignored` actions;
  trees that haven't chosen follow `FileTreeDefaults`, which the app sets from
  its settings. A changed `.gitignore` rereads every open folder.
- **Saved.** Which folders are open, and what a tree hides, is saved with the panel (feature
  `persistence`, default), relative to the root. The root starts open.

## Reading and watching

`Folders` (a resource shared by every tree) keeps a listing of each open
folder, read on a `ManagedWork` task, and watches each one
(`bedtime-filesystem`'s `DirectoryWatcher`, nonrecursive). A change reported in
a folder, or to an entry in it, reads that folder again; the previous listing
stays on screen until the new one arrives, and a failed reread keeps it. If
watching can't start, every open folder is reread every two seconds and the
tree says it isn't watching. Closed folders are let go (and unwatched); at most
512 folders are kept. Listings sort folders first, then by name ignoring case.
Symbolic links are rows, never followed into.

Drawing does no file I/O except `openers_for` when a context menu opens. Rows
are worked out again only when a listing or the open folders change. What the
user asks for while the tree draws (open, toggle, refresh, operations) is applied in
`Update` (`FileTreeSystems`), since opening a file can close the preview panel
being drawn.

## Not yet

From the old tree (in git history before bed_old was removed), still to come back
as needed: multiple selection (operations take one row), progress for long
operations, undoing them, hiding chosen files, and row animation.

## Tests

```sh
cargo test -p bedtime-filetree --all-features
python3 tools/bed.py run tests/automation/files.py tests/automation/files_hidden.py
```
