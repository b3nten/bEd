"""The editor's own Find: it opens with the keyboard, counts what matches,
narrows with Match case, replaces the next match, and closes."""
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The tree's row (tabs with the same name are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


with tempfile.TemporaryDirectory() as folder:
    notes = Path(folder) / "notes.txt"
    notes.write_text("first line\nsecond Line\nthird line\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        x0, y0, x1, y1 = row(bed, "notes.txt")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="first line"), "notes.txt opens")
        bed.wait(5)

        # Find and Replace opens with the keyboard in the find field.
        bed.key("Primary+Alt+F")
        until(bed, lambda: bed.exists("Close find"), "Find opens")
        assert bed.exists("Replace next"), "with Replace"
        bed.type("line")
        until(bed, lambda: bed.exists("3 matches"), "three lines match")
        bed.screenshot("editor-find")

        bed.click("Match case")
        until(bed, lambda: bed.exists("2 matches"), "Match case leaves two")

        # The replacement: nothing, so the next match goes.
        bed.click("Replace next")
        until(bed, lambda: bed.exists("1 matches"), "one is replaced")

        bed.click("Close find")
        until(bed, lambda: not bed.exists("Close find"), "Find closes")
        assert not bed.status()["errors"], bed.status()["errors"]
