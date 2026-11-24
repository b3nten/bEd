"""Outside changes: a file open in the editor shows what another program
writes to it, whether written in place or replaced (an atomic save), and
without waiting for input: the window is in the background."""
import os
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

        # Written in place.
        with open(notes, "a") as file:
            file.write("appended elsewhere\n")
        until(bed, lambda: bed.exists(contains="appended elsewhere"), "an append shows")

        # Replaced: a new file renamed over it.
        replacement = Path(folder) / ".notes.txt.tmp"
        replacement.write_text("replaced elsewhere\n")
        os.replace(replacement, notes)
        until(bed, lambda: bed.exists(contains="replaced elsewhere"), "a replace shows")

        # And written in place again: the watch moved to the new file.
        with open(notes, "a") as file:
            file.write("then appended\n")
        until(bed, lambda: bed.exists(contains="then appended"), "an append after a replace shows")
        bed.screenshot("outside_changes")
