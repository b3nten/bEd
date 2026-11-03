"""Files: a folder's layout 1 starts with the file tree. A click previews a
text file beside it and leaves the tree focused; a double click keeps it
open; binary data opens in hex. Open folders are
saved with the workspace. Files are created from the menu, renamed with F2,
and dragged into folders."""
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

        # Binary data opens in hex.
        bed.click(row(bed, "data.bin"))
        until(bed, lambda: bed.exists(contains="0x0"), "data.bin shows in hex")
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("files")

        # Drag a file from the tree onto the far side of the editor area: it
        # opens in a new split there.
        tab = max(bed.find_all("notes.txt"), key=lambda widget: widget["rect"][0])
        tx0, ty0, tx1, ty1 = tab["rect"]
        area_right = bed.status()["window"]["width"] - 30
        x0, y0, x1, y1 = row(bed, "other.txt")["rect"]
        before = len([w for w in bed.widgets() if (w.get("label") or "").startswith("Close ")])
        bed.drag(((x0 + x1) / 2, (y0 + y1) / 2), (area_right, ty1 + 300))
        until(bed, lambda: len([w for w in bed.widgets() if (w.get("label") or "").startswith("Close ")]) > before,
              "the dragged file opens")
        bed.screenshot("files-drag")

        # A new file in src, from its menu: named in a box, it opens.
        bed.click(row(bed, "src"), button="Right")
        until(bed, lambda: bed.exists(contains="New File"), "the tree's menu opens")
        bed.screenshot("files-menu")
        bed.click(bed.find(contains="New File"))
        until(bed, lambda: any(w["role"] == "TextInput" and not w.get("value") for w in bed.widgets()),
              "the name box shows")
        bed.type("made.rs")
        bed.key("Enter")
        until(bed, lambda: (root / "src" / "made.rs").exists(), "made.rs is created")
        until(bed, lambda: len(bed.find_all("made.rs")) == 2, "made.rs is listed and opened")

        # F2 renames the selected row; the name without its extension is
        # selected, so typing replaces it.
        bed.click(row(bed, "other.txt"))
        bed.key("F2")
        until(bed, lambda: any(w["role"] == "TextInput" and w.get("value") == "other.txt" for w in bed.widgets()),
              "the rename box shows")
        bed.type("renamed")
        bed.key("Enter")
        until(bed, lambda: (root / "renamed.txt").exists() and not (root / "other.txt").exists(), "other.txt is renamed")
        until(bed, lambda: bed.exists("renamed.txt"), "renamed.txt is listed")
        # Its tabs follow, even one behind another.
        until(bed, lambda: not bed.exists("other.txt"), "no tab keeps the old name")

        # Drag it onto src: it moves there. (Rows slide into place first.)
        time.sleep(0.3)
        bed.wait(3)
        x0, y0, x1, y1 = row(bed, "renamed.txt")["rect"]
        sx0, sy0, sx1, sy1 = row(bed, "src")["rect"]
        bed.drag(((x0 + x1) / 2, (y0 + y1) / 2), ((sx0 + sx1) / 2, (sy0 + sy1) / 2))
        until(bed, lambda: (root / "src" / "renamed.txt").exists() and not (root / "renamed.txt").exists(),
              "renamed.txt moves into src")
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("files-ops")

        # Away and back: src is still open.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("main.rs"), "src is open again")
