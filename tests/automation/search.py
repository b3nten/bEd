"""Find in Files: Cmd+Shift+F opens Search; matches are grouped by file, and
Match Case, Whole Word and Regular Expression narrow them. A click opens the
file with the match selected. Replace All changes open documents (undoably,
unsaved) and other files on disk; a regular expression's groups fill in."""
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

        # A click opens the file beside the results, the match selected.
        set_query(bed, "let (\\w+)")
        until(bed, lambda: status(bed) == "1 result in 1 file", "a regular expression")
        results_x = bed.find("let alpha = 1;")["rect"][0]
        bed.click("let alpha = 1;")
        until(bed, lambda: any(w["role"] == "TextInput" and "fn alpha" in (w.get("value") or "")
                               for w in bed.widgets()), "a.rs opens")
        editor = next(w for w in bed.widgets() if w["role"] == "TextInput" and "fn alpha" in (w.get("value") or ""))
        assert editor["rect"][0] > results_x + 100, "it opened over the results"
        assert status(bed) == "1 result in 1 file", "the results are still shown"
        bed.screenshot("search-opened")

        # From the editor, Cmd+Shift+F is Find in Files (not the editor's Find),
        # its selection the query.
        bed.key("Primary+Shift+F")
        until(bed, lambda: query_field(bed).get("value") == "let alpha", "the selection is the query")
        assert not bed.exists("Close find"), "the editor's Find opened too"

        # Replace All, with a group: the open a.rs is edited (unsaved), and
        # b.rs on disk.
        set_query(bed, "alpha\\b")
        until(bed, lambda: status(bed) == "4 results in 3 files", "alpha as a word")
        bed.click("chevron-right")
        until(bed, lambda: bed.exists("Replace All"), "the replace row")
        replace = next(w for w in bed.widgets() if w["role"] == "TextInput" and w["rect"][1] > 105)
        bed.click(replace)
        bed.type("omega")
        bed.click("Replace All")
        until(bed, lambda: status(bed) == "No results", "everything is replaced")
        assert (root / "src" / "b.rs").read_text() == "omega beta alphabet\n"
        assert (root / "src" / "a.rs").read_text() == "fn alpha() {}\nlet alpha = 1;\n", \
            "the open document isn't saved"
        bed.screenshot("search-replaced")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Cmd+W closes the focused panel: Search.
        bed.click(query_field(bed))
        bed.key("Primary+W")
        until(bed, lambda: not bed.exists("Replace All"), "Cmd+W closes Search")
