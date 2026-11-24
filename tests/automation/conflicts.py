"""Conflicts: a file changed on disk while it has unsaved edits says so at
the top of its panel; Save asks before writing over the change, and
Overwrite does. A file deleted on disk says so too, and Save puts it back."""
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what, seconds=5):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError(f"never: {what}")


with tempfile.TemporaryDirectory() as folder:
    notes = Path(folder) / "notes.txt"
    notes.write_text("first line\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        x0, y0, x1, y1 = min(bed.find_all("notes.txt"), key=lambda w: w["rect"][0])["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="first line"), "notes.txt opens")
        bed.wait(5)

        # Unsaved edits, then a change on disk: a conflict.
        bed.type("mine ")
        until(bed, lambda: bed.exists(contains="mine first line"), "typing shows")
        notes.write_text("theirs\n")
        until(bed, lambda: bed.exists(contains="Changed on disk"), "the conflict shows")
        bed.screenshot("conflict")

        # Save asks; Overwrite writes mine over theirs.
        bed.key("Primary+S")
        until(bed, lambda: bed.exists("Overwrite"), "Save asks")
        bed.screenshot("conflict-overwrite")
        bed.click(bed.find("Overwrite"))
        until(bed, lambda: notes.read_text().startswith("mine first line"), "mine is saved")
        until(bed, lambda: not bed.exists(contains="Changed on disk"), "the conflict is gone")

        # Deleted on disk: says so; Save puts it back.
        notes.unlink()
        until(bed, lambda: bed.exists(contains="Deleted on disk"), "the deletion shows")
        bed.screenshot("deleted")
        bed.click(bed.find("Save"))
        until(bed, lambda: notes.exists() and notes.read_text().startswith("mine first line"), "it's back")
        until(bed, lambda: not bed.exists(contains="Deleted on disk"), "the bar is gone")
