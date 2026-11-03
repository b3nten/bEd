# bedtime-git

Git for workbench apps: typed Git operations through the installed `git`, and
(feature `workbench`, default) the Git panel and diff panels.

```rust,ignore
app.add_plugins(GitPlugin)
   .insert_resource(GitHost { open_folder: Some(..), open_terminal: Some(..) });
world.spawn((GitPanel::default(), InArea(area)));
```

## Operations

GUI-free: call `execute` on a worker, or use the cancellable `Job`. Git owns
index locks, hooks, signing, configuration and commit identity.
`Operation::Commit` takes a message and `CommitMode`: `Commit` creates an
ordinary commit, `Amend` replaces HEAD using the current index, and `Signoff`
adds Git's `Signed-off-by` trailer. Amend can change only the message when
nothing is staged; it never stages working changes implicitly. All modes reject
empty messages and unresolved conflicts and keep the normal Git hooks.
Hunk operations (`StageHunk`, `UnstageHunk`, `DiscardHunk`) carry the diff's
snapshot and are refused if the file changed since.

## The Git panel

One `GitSession` (a resource, made when a Git or diff panel exists, over the
workbench's `Root`) owns the repository's state: it scans the status on a
worker (every 2 s while a panel shows, 10 s otherwise, and after each
operation) and runs one operation at a time.

- **Header:** the branch (a menu to switch branches or create one), view
  options (list or tree; grouped by tracked/untracked, staged/unstaged, or
  not), fetch, pull, push and refresh. Ahead/behind counts, a merge or rebase
  in progress (Continue, Abort…), errors (with Open Git terminal) and Git's
  output show under it.
- **Changes:** each file and folder has a staging checkbox (partly staged
  shows as such); groups too. Clicking a file shows its diff (a preview);
  double-clicking keeps it. Right-click: the unstaged or staged diff, Open
  File, stage, unstage, Discard file changes… (confirmed); for conflicts,
  use the current or incoming file, delete it, or stage the resolution.
- **Commit:** a message (kept with the panel), and Commit Staged, Amend or
  Commit with Signoff.
- **Before an operation** that reads or writes working files, open documents
  of those files are saved (or must be: unsaved edits block a discard);
  editors reload what Git changed.
- `GitHost` (the app's): opening the repository's root as the workspace
  (when the workspace is a folder inside it, changes are read-only), and a
  terminal there. Without them those buttons don't show.

## Diff panels

`DiffPanel` shows one file's working changes or staged changes as a unified
diff with old and new line numbers. Each change has Stage (or Unstage) and
Discard… (confirmed) in its header; Changes/Staged switch sides; the file
opens from the bar. It reads the diff again after every scan. A new diff goes
beside editors or diffs, else in the largest area without the Git panel or a
file tree, else in a split of the Git panel's area.

Unlike the old inline comparison in the editor, the diff isn't editable:
open the file to edit it.

```sh
cargo test -p bedtime-git
python3 tools/bed.py run tests/automation/git.py
```
