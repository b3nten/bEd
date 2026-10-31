"""Files: a folder's layout 1 starts with the file tree. A click previews a
text file beside it and leaves the tree focused; a double click keeps it
open; a file nothing opens is reported in the status bar. Open folders are
saved with the workspace."""
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
    root = Path(folder)
    (root / "src").mkdir()
    (root / "src" / "main.rs").write_text("fn main() {}\n")
    (root / "notes.txt").write_text("first line of notes\nsecond\n")
    (root / "other.txt").write_text("the other file\n")
    (root / "data.bin").write_bytes(bytes([0, 1, 2, 255]))

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")

        # A folder opens in place.
        bed.click(row(bed, "src"))
        until(bed, lambda: bed.exists("main.rs"), "src opens")

        # A click previews it beside the tree; the next click replaces it.
        bed.click(row(bed, "notes.txt"))
        until(bed, lambda: bed.exists(contains="first line of notes"), "notes.txt shows")
        tab = max(bed.find_all("notes.txt"), key=lambda widget: widget["rect"][0])
        assert tab["rect"][0] > row(bed, "notes.txt")["rect"][2], "not beside the tree"
        bed.click(row(bed, "other.txt"))
        until(bed, lambda: bed.exists(contains="the other file"), "other.txt shows")
        assert not bed.exists(contains="first line of notes"), "the preview wasn't replaced"

        # The tree kept focus: Up moves its selection.
        bed.key("ArrowUp")
        until(bed, lambda: row(bed, "notes.txt")["toggled"] == "True", "Up selects notes.txt")
        bed.screenshot("files-preview")

        # A double click keeps it, so the next preview opens beside it.
        x0, y0, x1, y1 = row(bed, "notes.txt")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="first line of notes"), "notes.txt shows again")
        bed.click(row(bed, "other.txt"))
        until(bed, lambda: bed.exists("other.txt") and len(bed.find_all("other.txt")) == 2, "other.txt previews")
        assert len(bed.find_all("notes.txt")) == 2, "the double-clicked file was replaced"

        # Nothing here opens binary data.
        bed.click(row(bed, "data.bin"))
        until(bed, lambda: bed.exists(contains="can't open data.bin"), "data.bin is reported")
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("files")

        # Away and back: src is still open.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("main.rs"), "src is open again")
