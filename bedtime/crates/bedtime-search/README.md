# bedtime-search

Project search and replace for workbench apps.

```rust,ignore
app.add_plugins(SearchPlugin);
// Find in Files: the app decides where the panel goes, then:
bedtime_search::seed(world, panel, selected_text);
```

## The engine

`SearchQuery` is the text and its options: Match Case, Whole Word, Regular
Expression (Rust's `regex` syntax), include and exclude globs (comma
separated; one without a `/` matches a name at any depth, a folder matches
what's in it), and whether files Git ignores are searched too. `matcher()`
makes it one `regex` (escaped unless it's a regular expression, `\b…\b` for
whole words), or says why it can't.

`Matcher::search` walks a folder in path order with the `ignore` crate (Git's
ignore files apply unless asked; `.git` never), skipping files over 16 MiB,
with a NUL near the start, or not UTF-8. A file given in `open` (an open
document's unsaved text) is searched as that. Each file with matches is
handed over as it's found, each match with its line and column (in
characters) and a one-line preview; a search stops at 20,000 matches.

`Matcher::replacement` gives what replaces one match (a regular expression's
groups filled in: `$1`, `${name}`), and checks it still matches there;
`replacements` gives every one, last first.

## The Search panel (feature `workbench`)

`SearchPanel` searches the workbench's `Root` as you type (after 150 ms), on a
`ManagedWork` task; results stream in. The query row has the toggles (`Aa`,
`ab`, `.*`) and a filter button for the include/exclude row; the chevron shows
the replace row. Results are grouped by file (click to collapse), each match
its line number and line with the match marked. Clicking a match opens the
file with the match selected (`PathOptions::select_to`): in the panel already
showing it, else another area showing documents, else the search's area is
split, so the results stay in view.

Replace (on a hovered match) and Replace All change open documents as one
undoable edit each, left unsaved, and other files on disk if they haven't
changed since they were read; then it searches again. The query, the
replacement and which rows show are saved with the panel (`persistence`).

`FindInFiles` (Cmd/Ctrl+Shift+F) is registered; the app handles it.

```sh
cargo test -p bedtime-search --all-features
python3 tools/bed.py run tests/automation/search.py
```
