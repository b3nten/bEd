"""Notes: + makes a note whose title and text are typed; the list shows it;
the note is saved in the workspace's store, not in the folder; renaming and
deleting go through the list's context menu."""
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    bed.screenshot("notes_failed")
    raise AssertionError(f"never: {what}")


def store(bed):
    files = list(Path(bed.config).glob("workspaces/*/notes.ron"))
    return files[0].read_text() if files else ""


with tempfile.TemporaryDirectory() as folder:
    with Bed(folder, settings={"effects.enabled": True}) as bed:
        window = bed.status()["window"]
        bed.click((window["width"] * 0.6, window["height"] * 0.5))
        bed.wait(5)
        bed.invoke("notes.show")
        until(bed, lambda: bed.exists("New Note", role="Button"), "the empty panel shows")
        bed.screenshot("notes_empty")

        # A new note: the title has the keyboard; Enter goes on to the text.
        bed.click("New Note", role="Button")
        bed.wait(3)
        bed.type("Shopping")
        bed.key("Return")
        bed.wait(3)
        bed.type("milk and eggs")
        until(bed, lambda: bed.exists("Shopping", role="Button") or bed.exists("Shopping"),
              "the list shows the note")
        until(bed, lambda: "milk and eggs" in store(bed), "the note is saved in the workspace")
        assert not any(Path(folder).iterdir()), "nothing is written to the folder"
        bed.screenshot("notes_written")

        # A second note, then rename it from the list's context menu.
        bed.click("New Note", role="Button")
        bed.wait(3)
        bed.type("Ideas")
        bed.wait(3)
        bed.click("Ideas", role="SelectableLabel" if bed.exists("Ideas", role="SelectableLabel") else None,
                  button="Right")
        bed.wait(3)
        bed.click("Rename")
        bed.wait(3)
        bed.key("Primary+A")
        bed.type("Big ideas")
        bed.key("Return")
        until(bed, lambda: "Big ideas" in store(bed), "the rename is saved")
        bed.screenshot("notes_renamed")

        # Delete it, once confirmed: the other note shows.
        bed.click("Big ideas", button="Right")
        bed.wait(3)
        bed.click("Delete…")
        bed.wait(3)
        bed.click("Delete")
        until(bed, lambda: "Big ideas" not in store(bed), "the deletion is saved")
        until(bed, lambda: bed.exists(contains="milk and eggs"), "the other note shows")
        bed.screenshot("notes_deleted")

print("ok")
