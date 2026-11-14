"""Search Workspace: Cmd+Shift+F opens Search; matches are grouped by file,
and Match Case, Whole Word and Regular Expression narrow them. A click opens
the file with the match selected, where any file opens; dragging a match onto
the layout opens it there. A file's name (not the space after it) closes its
matches. Replace on a hovered match doesn't move the rows. Replace All changes
open documents (undoably, unsaved) and other files on disk; a regular
expression's groups fill in. The arrow keys move through the results, Delete
dismisses a row, Up in the query recalls earlier queries, the results follow
unsaved edits, and F4 opens the next result from anywhere."""
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def status(bed):
    return next((w.get("value") for w in bed.widgets()
                 if w["role"] == "Label" and " in " in (w.get("value") or "")
                 or (w.get("value") or "") == "No results"), None)


def query_field(bed):
    return next(w for w in bed.widgets() if w["role"] == "TextInput" and w["rect"][1] < 110)


def set_query(bed, text):
    bed.click(query_field(bed))
    bed.key("Primary+A")
    bed.type(text)
    time.sleep(0.3)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "src").mkdir()
    (root / "src" / "a.rs").write_text("fn alpha() {}\nlet alpha = 1;\n")
    (root / "src" / "b.rs").write_text("alpha beta alphabet\n")
    (root / "notes.txt").write_text("Alpha\n")

    with Bed(folder, settings={"editor.autosave": False}) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        # Panels open in the focused area: focus the large one, not the
        # explorer's.
        window = bed.status()["window"]
        bed.click((window["width"] * 0.6, window["height"] * 0.5))
        bed.wait(5)
        bed.key("Primary+Shift+F")
        until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "Search opens")
        bed.wait(10)
        bed.type("alpha")
        until(bed, lambda: status(bed) == "5 results in 3 files", "results")
        bed.screenshot("search")

        bed.click("Aa")
        until(bed, lambda: status(bed) == "4 results in 2 files", "Match Case")
        bed.click("ab")
        until(bed, lambda: status(bed) == "3 results in 2 files", "Whole Word")
        bed.click("Aa")
        bed.click("ab")

        # A pattern that doesn't parse says why.
        bed.click(".*")
        set_query(bed, "alpha(")
        until(bed, lambda: any("unclosed" in (w.get("value") or "") for w in bed.widgets()),
              "the regular expression's error shows")

        # A file's name closes its matches; the space after it doesn't.
        set_query(bed, "alpha")
        until(bed, lambda: status(bed) == "5 results in 3 files", "alpha again")
        x0, y0, x1, y1 = bed.find("a.rs")["rect"]
        bed.click((x1 + 120, (y0 + y1) / 2))
        bed.wait(20)
        assert bed.exists("let alpha = 1;"), "the empty space after the name took the click"
        bed.click("a.rs")
        until(bed, lambda: not bed.exists("let alpha = 1;"), "a.rs closes")
        bed.click("a.rs")
        until(bed, lambda: bed.exists("let alpha = 1;"), "a.rs opens again")

        # Replace shows on a hovered match without moving the rows below.
        bed.click("replace")
        until(bed, lambda: bed.exists("replace-all"), "the replace row")
        bed.wait(30)
        bed.screenshot("search-replace-row")
        below = bed.find("alpha beta alphabet")["rect"][1]
        x0, y0, x1, y1 = bed.find("let alpha = 1;")["rect"]
        bed.move(x0 + 10, (y0 + y1) / 2)
        bed.wait(10)
        after = bed.find("alpha beta alphabet")["rect"][1]
        assert after == below, f"hovering moved the rows: {below} -> {after}"
        bed.screenshot("search-hover-replace")
        bed.click("replace")
        until(bed, lambda: not bed.exists("replace-all"), "the replace row hides")

        # Dragging a match onto the far side of the layout opens it there.
        area_right = bed.status()["window"]["width"] - 30
        x0, y0, x1, y1 = bed.find("fn alpha() {}")["rect"]
        bed.drag((x0 + 10, (y0 + y1) / 2), (area_right, y1 + 300))
        until(bed, lambda: any(w["role"] == "TextInput" and "fn alpha" in (w.get("value") or "")
                               for w in bed.widgets()), "the dragged match opens")
        editor = next(w for w in bed.widgets() if w["role"] == "TextInput" and "fn alpha" in (w.get("value") or ""))
        assert editor["rect"][0] > x1, "it opened beside the results"
        assert status(bed) == "5 results in 3 files", "the results are still shown"
        bed.screenshot("search-dragged")

        # A click opens the file, the match selected.
        set_query(bed, "let (\\w+)")
        until(bed, lambda: status(bed) == "1 result in 1 file", "a regular expression")
        bed.click("let alpha = 1;")
        until(bed, lambda: any(w["role"] == "TextInput" and "fn alpha" in (w.get("value") or "")
                               for w in bed.widgets()), "a.rs opens")
        bed.screenshot("search-opened")

        # From the editor, Cmd+Shift+F is Search Workspace (not the editor's Find):
        # a new Search panel in the editor's area, its selection the query.
        bed.key("Primary+Shift+F")
        until(bed, lambda: any(w["role"] == "TextInput" and w.get("value") == "let alpha"
                               for w in bed.widgets()), "the selection is the query")
        assert not bed.exists("Close find"), "the editor's Find opened too"
        # Close it again (it's focused), leaving the first, with its own query.
        bed.key("Primary+W")
        until(bed, lambda: not any(w["role"] == "TextInput" and w.get("value") == "let alpha"
                                   for w in bed.widgets()), "the second Search panel closes")
        assert query_field(bed).get("value") == "let (\\w+)", "the first Search panel was replaced"

        # Keys. Down from the query goes to the results: the first file, then
        # its match; Delete dismisses that (and, its last, the file).
        set_query(bed, "alpha")
        until(bed, lambda: status(bed) == "5 results in 3 files", "alpha once more")
        bed.key("ArrowDown")
        bed.key("ArrowDown")
        bed.wait(5)
        bed.screenshot("search-keys")
        bed.key("Delete")
        until(bed, lambda: status(bed) == "4 results in 2 files", "Delete dismisses the match")
        # The selection is now a.rs: Left closes it, Right opens it, Down and
        # Space open its first match without leaving the results.
        bed.key("ArrowLeft")
        until(bed, lambda: not bed.exists("let alpha = 1;"), "Left closes a.rs")
        bed.key("ArrowRight")
        until(bed, lambda: bed.exists("let alpha = 1;"), "Right opens a.rs")
        bed.key("ArrowDown")
        bed.key("Space")
        bed.wait(10)
        bed.key("ArrowDown")
        bed.key("Delete")
        until(bed, lambda: status(bed) == "3 results in 2 files",
              "the results kept the focus after Space")
        # Searching again doesn't bring back what was dismissed; a new query does.
        last = bed.find("alpha beta alphabet")["rect"]
        bed.click((last[0] + 10, last[3] + 100), button="Right")
        until(bed, lambda: bed.exists("Search Again"), "the results' menu")
        bed.click("Search Again")
        bed.wait(40)
        assert status(bed) == "3 results in 2 files", status(bed)

        # Up in the query: the query used before (a match was opened with it).
        bed.click(query_field(bed))
        bed.key("ArrowUp")
        until(bed, lambda: query_field(bed).get("value") == "let (\\w+)", "Up recalls a query")
        bed.key("ArrowDown")
        until(bed, lambda: query_field(bed).get("value") == "alpha", "Down returns to what was typed")

        # Replace All, with a group: the open a.rs is edited (unsaved), and
        # b.rs on disk. It asks first, and each match shows what it becomes.
        set_query(bed, "alpha\\b")
        until(bed, lambda: status(bed) == "4 results in 3 files", "alpha as a word")
        bed.click("replace")
        until(bed, lambda: bed.exists("replace-all"), "the replace row")
        replace = next(w for w in bed.widgets() if w["role"] == "TextInput" and w["rect"][1] > 105)
        bed.click(replace)
        bed.type("omega")
        bed.wait(10)
        bed.screenshot("search-replace-preview")
        bed.click("replace-all")
        until(bed, lambda: bed.exists("Replace All"), "Replace All asks first")
        assert status(bed) == "Replace 4 matches in 3 files?", status(bed)
        assert bed.exists(contains="changed on disk"), "it says files on disk change"
        assert (root / "src" / "b.rs").read_text() == "alpha beta alphabet\n", "nothing yet"
        bed.screenshot("search-replace-confirm")
        bed.click("Cancel")
        until(bed, lambda: status(bed) == "4 results in 3 files", "Cancel leaves it")
        bed.click("replace-all")
        until(bed, lambda: bed.exists("Replace All"), "asked again")
        bed.click("Replace All")
        until(bed, lambda: status(bed) == "No results", "everything is replaced")
        assert (root / "src" / "b.rs").read_text() == "omega beta alphabet\n"
        assert (root / "src" / "a.rs").read_text() == "fn alpha() {}\nlet alpha = 1;\n", \
            "the open document isn't saved"
        bed.screenshot("search-replaced")
        assert not bed.status()["errors"], bed.status()["errors"]

        # The results follow unsaved edits: type a match into the open a.rs.
        editor = next(w for w in bed.widgets()
                      if w["role"] == "TextInput" and "fn omega" in (w.get("value") or ""))
        bed.click(editor)
        bed.type(" alpha ")
        until(bed, lambda: status(bed) == "1 result in 1 file", "the edit is found")

        # F4 opens the next result from wherever the focus is: b.rs, from the
        # editor.
        set_query(bed, "beta")
        until(bed, lambda: status(bed) == "1 result in 1 file", "beta")
        bed.click(editor)
        bed.key("F4")
        until(bed, lambda: any(w["role"] == "TextInput" and "beta" in (w.get("value") or "")
                               and w.get("value") != "beta" for w in bed.widgets()),
              "F4 opens the next result")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Cmd+W closes the focused panel: Search.
        bed.click(query_field(bed))
        bed.key("Primary+W")
        until(bed, lambda: not bed.exists("replace-all"), "Cmd+W closes Search")
