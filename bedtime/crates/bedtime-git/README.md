# bedtime-git

Typed operations through the installed `git` executable. The default
`workbench` feature adds Git and diff panels.

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
nothing is staged; it never stages working changes implicitly. Amend with an
empty message keeps the last commit's; the other modes reject empty messages.
All reject unresolved conflicts and keep the normal Git hooks.
Hunk operations (`StageHunk`, `UnstageHunk`, `DiscardHunk`) carry the diff's
snapshot and are refused if the file changed since.

Staging everything leaves conflicted files alone; a resolution is staged by
name, and refused only while it still has an opening and a closing conflict
marker. `Pull` pulls as Git is configured to; `PullRebase` and `PullMerge`
say how, for when the branch and its upstream have both moved.

Git runs without a terminal (its own session), so it, ssh and gpg fail at
once rather than wait for an answer nobody can give; graphical helpers
(a credential helper, an askpass, pinentry) still work. An operation waits
for another of this app's on the same repository (15 s), and a command is
run again for about 3 s while another Git process holds a lock file. A
failed command's `Error::message` says what happened for the common cases
(diverged, rejected push, sign-in, no upstream, lock held, conflicts…);
Git's own words are in `output`. A filename that isn't UTF-8 is left out of
the status.

## The Git panel

One `GitSession` (a resource, made when a Git or diff panel exists, over the
workbench's `Root`) owns the repository's state: it scans the status on a
worker (every 2 s while a panel shows, 10 s otherwise, and after each
operation) and runs one operation at a time. What's asked for meanwhile
waits its turn (files ticked one after another are staged together) and is
dropped if the operation before it fails or is cancelled. A commit checks
that what's staged is still what was shown; other changes in the folder
don't stop it.

- **Header:** the branch (a menu to switch branches or create one), view
  options (list or tree; grouped by tracked/untracked, staged/unstaged, or
  not), fetch, pull, push and refresh. Ahead/behind counts, a merge or rebase
  in progress (Continue, Abort…), errors (with Open Git terminal, and the
  next step where there is one: Pull with rebase or merge, Pull before a
  rejected push, Retry) and Git's output show under it. A failed status
  scan's error goes away with the next good one.
- **Changes:** each file and folder has a chip before it that stages it:
  a file's holds its status letter, outlined, tinted when partly staged, or
  filled (a folder's or group's is ticked, or dashed). The other
  `StagingStyle`, Commit lanes, lists changes and staged files apart (side
  by side from 420 px, else stacked), moved across with arrows; it sets the
  grouping aside. Clicking a file shows its diff (a preview);
  double-clicking keeps it. Right-click: the unstaged or staged diff, Open
  File, stage, unstage, Discard file changes… or Delete untracked file…
  (confirmed); for conflicts,
  use the current or incoming file, delete it, or stage the resolution.
- **Commit:** a message (kept with the panel), and Commit Staged, Amend or
  Commit with Signoff. Amend with no message keeps the last commit's.
- **Before an operation** that reads or writes working files (not creating
  a branch, which keeps them), open documents of those files are saved (or must be: unsaved edits block a discard);
  editors reload what Git changed.
- `GitHost` (the app's): opening the repository's root as the workspace
  (when the workspace is a folder inside it, changes are read-only), and a
  terminal there. Without them those buttons don't show.

## Diff panels

`DiffPanel` is an editor (bedtime-code's, required by the panel and drawn by it:
`draw_editor`) with one file's changes inline: old lines between the new, old
and new line numbers, and above each change Stage (or Unstage) and Discard…
(asked about first; only with the file saved). The working changes are shown
over the file itself, so it can be edited right there and the comparison
follows the edits; the staged changes over a read-only copy of what's staged.
Changes/Staged switch sides; the file opens from the bar.

A change is applied exactly (`StageRange`, `UnstageRange`, `DiscardRange`).
Staging saves the file first, then checks (`StageGuard`) that Git's baseline
and the saved file are what the change was chosen in before applying it.
The comparison is read again after every scan. A new diff goes beside
editors or diffs, else in the largest area without the Git panel or a file
tree, else in a split of the Git panel's area.

```sh
cargo test -p bedtime-git
python3 tools/bed.py run tests/automation/git.py
```
