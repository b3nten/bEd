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
- **Keys.** Up/Down, Home/End and Page Up/Down move the selection; Right opens
  a folder or moves into it, Left closes it or moves to its parent; Enter
  opens; typing selects the next row starting with what was typed.
- **Saved.** Which folders are open is saved with the panel (feature
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
user asks for while the tree draws (open, toggle, refresh) is applied in
`Update` (`FileTreeSystems`), since opening a file can close the preview panel
being drawn.

## Not yet

From the old tree (`bedtime_old/crates/bedtime-filetree`), still to come back
as needed: creating, renaming, moving, copying and trashing files (and their
clipboard and drag-and-drop), dragging a file out as a panel, multiple
selection, hiding files, and row animation.

## Tests

```sh
cargo test -p bedtime-filetree --all-features
python3 tools/bed.py run tests/automation/files.py
```
