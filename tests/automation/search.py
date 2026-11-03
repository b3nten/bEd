"""Search Workspace: Cmd+Shift+F opens Search; matches are grouped by file,
and Match Case, Whole Word and Regular Expression narrow them. A click opens
the file with the match selected, where any file opens; dragging a match onto
the layout opens it there. A file's name (not the space after it) closes its
matches. Replace on a hovered match doesn't move the rows. Replace All changes
open documents (undoably, unsaved) and other files on disk; a regular
expression's groups fill in."""
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

        # From the editor, Cmd+Shift+F is Search Workspace (not the editor's Find),
        # its selection the query.
        bed.key("Primary+Shift+F")
        until(bed, lambda: query_field(bed).get("value") == "let alpha", "the selection is the query")
        assert not bed.exists("Close find"), "the editor's Find opened too"

        # Replace All, with a group: the open a.rs is edited (unsaved), and
        # b.rs on disk.
        set_query(bed, "alpha\\b")
        until(bed, lambda: status(bed) == "4 results in 3 files", "alpha as a word")
        bed.click("replace")
        until(bed, lambda: bed.exists("replace-all"), "the replace row")
        replace = next(w for w in bed.widgets() if w["role"] == "TextInput" and w["rect"][1] > 105)
        bed.click(replace)
        bed.type("omega")
        bed.click("replace-all")
        until(bed, lambda: status(bed) == "No results", "everything is replaced")
        assert (root / "src" / "b.rs").read_text() == "omega beta alphabet\n"
        assert (root / "src" / "a.rs").read_text() == "fn alpha() {}\nlet alpha = 1;\n", \
            "the open document isn't saved"
        bed.screenshot("search-replaced")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Cmd+W closes the focused panel: Search.
        bed.click(query_field(bed))
        bed.key("Primary+W")
        until(bed, lambda: not bed.exists("replace-all"), "Cmd+W closes Search")
