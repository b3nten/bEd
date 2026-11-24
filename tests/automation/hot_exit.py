"""Hot exit: quitting with unsaved edits doesn't ask; they're kept, and come
back unsaved next time, the file itself untouched. A new untitled document
comes back too. (Autosave off, as it would save the edits first.)"""
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what, seconds=10):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(f"never: {what}")


def quit(bed):
    """Cmd+Q; bed may be gone before it answers."""
    try:
        bed.key("Primary+Q")
    except (ConnectionError, OSError):
        pass
    bed.process.wait(20)


with tempfile.TemporaryDirectory() as folder, tempfile.TemporaryDirectory() as config:
    (Path(config) / "global").mkdir()
    (Path(config) / "global" / "settings.ron").write_text('{\n    "appearance.cat": false,\n    "editor.autosave": false,\n}\n')
    notes = Path(folder) / "notes.txt"
    notes.write_text("first line\n")

    bed = Bed(folder, config=config)
    until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
    x0, y0, x1, y1 = min(bed.find_all("notes.txt"), key=lambda w: w["rect"][0])["rect"]
    at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
    bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
    until(bed, lambda: bed.exists(contains="first line"), "notes.txt opens")
    bed.wait(5)
    bed.type("kept ")
    until(bed, lambda: bed.exists(contains="kept first line"), "typing shows")
    bed.key("Primary+N")
    until(bed, lambda: bed.exists("Untitled"), "a new untitled document")
    bed.wait(5)
    bed.type("a draft")
    until(bed, lambda: bed.exists(contains="a draft"), "typing in it shows")
    quit(bed)
    assert bed.process.returncode == 0, bed.process.returncode
    assert notes.read_text() == "first line\n", "the file wasn't saved"

    bed = Bed(folder, config=config, build_first=False)
    # The draft's tab is selected; then the file's (right of the tree's row).
    until(bed, lambda: bed.exists(contains="a draft"), "the draft comes back")
    until(bed, lambda: len(bed.find_all("notes.txt")) > 1, "the file's tab is back")
    bed.click(max(bed.find_all("notes.txt"), key=lambda w: w["rect"][0]))
    until(bed, lambda: bed.exists(contains="kept first line"), "the edit comes back")
    bed.screenshot("hot_exit")
    quit(bed)
    assert notes.read_text() == "first line\n"
