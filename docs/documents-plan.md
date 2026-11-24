# Plan: documents, saving, closing and quitting

Agreed 2026-11-23. Document handling has been through churn and is more
cautious than it needs to be. The aim is that a document be a general idea,
not a file with extras. `documents.diagram` shows how it works now. Do one
step at a time and review it before the next; update the diagram as each
step lands.

## Where it stands

The core does not depend on the filesystem. `bedtime-document` has
`DocumentModel`, `History` and `DocumentSource`; the workbench's `documents`
feature doesn't depend on `bedtime-filesystem`, and files come in through
`files`. Git's `Committed` and `Staged` are non-file sources.

The coupling is around the core:

- **Where a document lives is on its panels.** `ViewerFile(path)` is a
  component on each panel, and search and git look for it there. `SourceId`
  lasts only for one run, so nothing can name a source across runs.
- **"Document" means two things.** SQLite, video and sound are documents
  with an empty model (`Change = Infallible`, `UNREAD`). They use one only
  for open-by-format, the viewer registry, reusing a panel and reopening.
- **Save As and Untitled are file-shaped.** Saving an untitled document
  picks a path (`save_to(.., &Path)`), and it all needs `files`.
- **No writable source that isn't a file has been tried.** Notes, HTTP
  requests and settings keep and save their own state, without documents.

And it is too cautious:

- Every open file is checked for changes every 500 ms, and each check
  starts a thread (`check_interval`, `start_read`).
- A file that changes on disk while it has unsaved edits becomes a
  conflict, which blocks saving and isn't shown anywhere.
- Quitting:
  - The documents' `ExitGate` asks about every unsaved document.
  - Uncommitted input, such as an IME composition, blocks a quit ("Finish
    typing first").
  - Then every background task is cancelled, and the app waits for each
    one's thread to stop.
- An unsaved document with no panel left showing it is kept, unseen, until
  quitting asks about it (`release_unviewed`).

## Steps

### 1. Sources say when they change; files use the watcher

Done 2026-11-23:
- `DocumentSource::watch(SourceChanged) -> Option<SourceWatch>` replaces
  `check_interval`; `MemorySource` implements it too.
- Files are watched by `bedtime-filesystem/src/file_watch.rs`. A notice
  wakes the app (`UiWaker`), and the document reads again only if the
  version changed. Window focus coming back makes the same check.
- Tested by `tests/automation/outside_changes.py`.

- `DocumentSource` loses `check_interval`. In its place, a source can tell
  the document it changed, for instance with a waker the workbench passes
  in. The generic layer knows nothing about watchers.
- `FsDocumentSource` uses `bedtime-filesystem`'s `DirectoryWatcher`. It
  watches each open file's folder, not recursively, sharing a watch between
  files in the same folder. A change re-reads the documents in that folder.
- Also check again when the app's window regains focus, in case the
  watcher missed something.
- Done when no thread is started per check, and an outside edit shows up
  without polling.

### 2. Conflicts you can see; Save As for every document

Done 2026-11-23:
- Panels can have a bar at the top (`add_panel_header` in bedtime-ui).
  Documents use it for "Changed on disk" (Keep Mine, Reload) and "Deleted
  on disk" (Save puts it back).
- `request_save` is the save someone asked for. It asks where for an
  untitled document, and asks before writing over an outside change
  (Overwrite, Reload Theirs, Cancel). The Save action and the editor's own
  Save use it.
- `settle_document_conflict` settles a conflict without knowing the model.
- Save As (`document.save_as`, Cmd+Shift+S):
  - `save_document_as` asks where through `SaveAs`.
  - `move_document` keeps a document in any source.
  - `save_document_to` keeps it in a file.
- Tested by `tests/automation/conflicts.py`, and by unit tests.

- A conflict shows in the panel: "Changed on disk", with Reload and Keep
  mine (`resolve_document_conflict`).
- Saving over a conflict asks whether to overwrite, rather than refusing
  silently.
- Save As is "save to a source the app gives": a hook that returns a new
  `DocumentSource` for a document (`replace_source` already moves one).
  - Files supply a picker that makes an `FsDocumentSource`.
  - Untitled no longer needs `files`.
- Cmd+Shift+S works for any writable document, not only untitled ones.

### 3. Names for sources that last across runs, then hot exit

Done 2026-11-23:
- Names:
  - `SourceName` (`file:///…`, `untitled:…`) and `DocumentSource::name`.
  - `document_name`, `document_file`, `holder_file` and `document_for_file`
    replace looking for `ViewerFile` on panels; search, git, the code
    editor's language servers, markdown, SQLite, video and the 3D model
    viewer use them.
  - Viewer panels save `ViewerSource(name)`. The old `ViewerFile` is still
    read, and converted, when restoring. The explorer no longer rewrites
    panels after moves.
- Hot exit (`documents/unsaved.rs`, with `persistence`):
  - Backups go in the session store's `unsaved` entry: about a second after
    a change, on a worker, and at once on quit or a workspace switch.
  - A reopened document gets its backup as one undoable step. If the file
    changed meanwhile, that's a conflict.
  - Untitled documents reopen by name (`set_new_document` registers how).
  - Quitting asks only about documents it couldn't keep; with no store,
    as before.
- Tested by unit tests (in-memory store) and
  `tests/automation/hot_exit.py`.
- Backups no document takes are opened at startup (step 4).

- A source has a name that lasts across runs, a URI: `file:///…`,
  `git:staged/…`, `untitled:2`. Plugins register how to reopen a source
  from its name.
- That name replaces `ViewerFile` as the way to find where a document lives
  and to reopen it. Panels save the name; search and git ask the document.
- Hot exit:
  - Unsaved documents, untitled ones included, are backed up in the
    workspace's store under their name, with what was last saved
    (version), so a later outside change is still noticed.
  - Next session they are restored with their unsaved marker.
  - Quitting and switching folders never ask; only closing the last tab of
    an unsaved document still does.

### 4. A much smaller exit

Done 2026-11-23:
- Exits cancel background work and wait at most `STOP_WAIT` (0.5 s) for
  it, not for every thread. Saves are still settled first, by the
  documents' gate.
- `ExitGate::may_start` and the "Finish typing first" block are gone.
- No unsaved document is out of sight:
  - One let go by code is shown again in a new panel (`show_document`).
  - At startup, a backup no restored panel took opens its file, or comes
    back as an untitled document if the file is gone.
- Documents are `InWorkspace`, so switching workspace closes them (after
  keeping their unsaved changes in the old workspace's store).
- The documents' `ExitGate` stays: it waits for saves, and asks about what
  hot exit can't keep (no store, or a document without a name or byte
  form). `closing.rs` is otherwise the close-tab prompt.
- The terminal messages on quit weren't reproduced: a quit with a
  terminal open takes under 0.1 s and prints only a bevy_winit warning.

- Remove the documents' `ExitGate`, the uncommitted-input block on exiting,
  and the unsaved documents kept unseen.
- Quitting:
  - Writes the backups and stores right away.
  - Cancels background work and exits without waiting for threads. Saves
    write a temporary file and rename it, so stopping one partway leaves
    the old file whole.
  - Switching folders does the same for the workspace's work.
- `closing.rs` shrinks to the close-tab prompt. Consider whether `ExitGate`
  is still needed at all, or whether a list of "before exit" hooks will do.
- Find what prints "process closed" style messages to the terminal on quit.
  Not seen with a plain Cmd+Q, so it comes from something open (a terminal,
  a language server); waiting for their threads is the likely cause.

### 5. Documents for files that aren't loaded as bytes (SQLite, video, sound)

Decided 2026-11-23: these stay documents, with no separate "file viewer"
idea. The document interface should be flexible enough for SQLite to be
backed fully by documents, and for video to become editable later. What
doesn't fit today is the file layer, which only reads and writes whole
files as bytes; `UNREAD` and `decode(&[])` work around that. After step 6:

- A file model chooses how it reads and writes its file, with bytes as the
  default:
  - Video probes the file; it's read-only until it can be edited, then an
    edit list that Save renders.
  - SQLite opens a connection and reads what it shows; saving commits.
- `UNREAD` and the empty models go.
- SQLite uses step 6's sources that commit each change:
  - A change is an edit with its inverse (the old values), so undo works.
  - It is never unsaved, never asked about, never backed up.
  - An outside change to the database reloads it.
- Later, for video editing: hot exit and siblings need a byte form; a model
  should be able to give its own backup form (an edit list).

### 6. A writable source that isn't a file: Notes, then HTTP requests

Done 2026-11-23:
- Sources that save every change:
  - `DocumentSource::saves_every_change`: never unsaved (`dirty` is
    false), so never asked about or backed up.
  - "Saving" while a change waits to be written, so exits wait for it.
  - A failed write isn't retried until the next change.
- Store-entry sources:
  - `StoreSource` and `Bedtime::store_document::<M, S>(entry, title)`:
    the same document each time while the store is kept in the same place.
  - Named `store:<store>/<entry>`.
  - Watched through the entry's file (`StoreBackend::file`, then
    `FsDocumentSource::watch_changes`).
  - In memory for the run when the store isn't kept anywhere.
- Notes:
  - `Notes` is a document model (`NoteChange`: insert, remove, set title,
    edit text), kept as RON in the same `notes` entry, so existing notes
    load.
  - Panels get the document from a system.
  - Typing is one undo step until a one-second pause.
  - Inside a text field, Cmd+Z is the field's own undo (its changes become
    edits); elsewhere in the panel it's the document's.
- HTTP requests:
  - `HttpRequests` is a document model (`RequestChange`: insert, remove,
    set a request, set the variables).
  - The panel edits a copy as before; `changes` turns what differs into
    changes, made together. Edits close in time are one undo step.
  - Intents wait for the document to be read.
- Tested by unit tests (store sources, both models' inverses, the request
  comparison, undo) and `tests/automation/notes.py`. That script now also
  undoes a delete and edits `notes.ron` by hand.

- A `DocumentSource` backed by a persistence store entry:
  - `load` reads the entry and `save` writes it;
  - it's named `store:<store>/<entry>`, and is watched like a file.
- Not an in-memory source: their content belongs in the store they're
  already saved in.
- Sources that commit each change as it's applied: saved right after each
  edit, never shown as unsaved, never asked about on closing.
- Notes first, as a model with undoable changes (note text through
  `TextBuffer` edits; adding, removing and renaming notes). That gives Notes
  undo and redo, and lets it notice outside changes to its entry. Then the
  HTTP client in the same way.
