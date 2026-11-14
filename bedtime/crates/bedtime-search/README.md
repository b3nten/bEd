# bedtime-search

Project search and replace for workbench apps.

```rust,ignore
app.add_plugins(SearchPlugin);
// Search Workspace: the app decides where the panel goes, then:
bedtime_search::seed(world, panel, selected_text);
```

## The engine

`SearchQuery` is the text and its options: Match Case, Whole Word, Regular
Expression (Rust's `regex` syntax), include and exclude globs (comma
separated; one without a `/` matches a name at any depth, a folder matches
what's in it), whether files Git ignores are searched too, and whether only
the open files are searched. `matcher()` makes it one `regex` (escaped unless
it's a regular expression, `\b…\b` for whole words), or says why it can't.

`Matcher::search` walks a folder on several threads with the `ignore` crate
(Git's ignore files apply unless asked; `.git` never), or goes through the
open files if the query asks for only those. Binary files (a NUL near the
start) are passed over; files over 16 MiB or not UTF-8 are too, and counted
in `Summary::skipped`. `OpenFiles` says which files are open, with their
unsaved text if they have any: that's searched, not the file. Each file with
matches is handed over as it's found, in no particular order, each match with
its line and column (in characters) and a one-line preview; a search stops at
20,000 matches.

`Matcher::replacement` gives what replaces one match (a regular expression's
groups filled in: `$1`, `${name}`), and checks it still matches there;
`replacements_at` does that for the matches found earlier (none, if any no
longer matches), `replacements` for every match in a text, both last first.
`preview_replacement` works out what a match would become from its preview
line, for showing.

## The Search panel (feature `workbench`)

`SearchPanel` searches the workbench's `Root` as you type (after 150 ms), on a
`ManagedWork` task; results stream in, kept in path order. The query field
holds the toggles (`Aa`, `ab`, `.*`); after it, Replace shows the replace row
and Filter the include/exclude fields (with Files Git ignores and Only open
files), all sharing the query's edges. Up and Down in the query go through
the queries used before (saved with the panel); Down past the newest goes to
the results.

Results are a tree like the other trees (`bedtime_ui::tree`): grouped by file
(its name opens and closes it, its matches sliding under it; its folder after
it, shortened from the front), each match its line number and line with the
match marked. Clicking a match opens the file with the match selected
(`PathOptions::select_to`) where any file opens: the panel showing it, else
an area with that kind of viewer, else the largest area. Dragging a file or a
match onto the layout opens it there. A row's menu copies its path or line.

With the focus in the results: Up, Down, Page Up, Page Down, Home and End
move the selection, Left and Right close and open a file, Enter opens a match
(Space too, leaving the focus in the results), Delete dismisses the row and
Escape goes back to the query. `NextResult` and `PreviousResult` (F4,
Shift+F4) open the match after or before the selected one from wherever the
focus is.

The results follow unsaved edits: when an open document changes (or is saved
or closed), the search runs again once editing pauses, and what's shown stays
until the new results are in. Changes other programs make on disk aren't
noticed; Search Again (in the menu) asks.

With the replace row shown, each match shows what it would become. Replace
(on a hovered match) changes one. Replace All asks first, saying how many
files are changed on disk, then changes the matches listed: not ones that
were dismissed (the × on a hovered row), nor ones past the 20,000 where a
search stopped. Open documents are changed as one undoable edit each, left
unsaved; other files on disk, on a worker, if they haven't changed since
they were searched. Then it searches again.

`SearchWorkspace` (Cmd/Ctrl+Shift+F) is registered; the app handles it.

```sh
cargo test -p bedtime-search --all-features
python3 tools/bed.py run tests/automation/search.py
```
