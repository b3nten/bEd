"""Editor: a text file opened from Files can be edited and saved, undo works,
and the editor comes back with the workspace."""
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
    notes.write_text("first line\nsecond line\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        x0, y0, x1, y1 = row(bed, "notes.txt")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="first line"), "notes.txt opens")
        bed.wait(5)

        # Opened for good, the editor has focus: typing goes in at the start.
        bed.type("hello ")
        until(bed, lambda: bed.exists(contains="hello first line"), "typing shows")
        bed.key("Primary+S")
        until(bed, lambda: notes.read_text().startswith("hello first line"), "saved")

        bed.key("Primary+Z")
        until(bed, lambda: not bed.exists(contains="hello first"), "undone")
        bed.key("Primary+Shift+Z")
        until(bed, lambda: bed.exists(contains="hello first line"), "redone")
        bed.screenshot("editor")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Away and back: the editor shows the file again.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists(contains="hello first line"), "the editor is restored")
        bed.wait(30)
        bed.screenshot("editor-restored")
