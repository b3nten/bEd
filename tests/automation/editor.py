"""Editor: a text file opened from Files can be edited and saved, undo works,
the right-click menu cuts and pastes, and the editor comes back with the
workspace."""
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


def wrap(bed):
    """The editor's own Word Wrap: None follows the setting."""
    editor = "bedtime_code::editor::panel::EditorPanel"
    rows = bed.call("world.query", {"data": {"components": [editor]}})
    return rows[0]["components"][editor]["wrap"]


with tempfile.TemporaryDirectory() as folder:
    notes = Path(folder) / "notes.txt"
    notes.write_text("first line\nsecond line\n" + "a long line " * 8 + "end\n")

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

        # The right-click menu: Select All, Cut (the text goes), Paste (back).
        editor = next(w for w in bed.widgets() if w["role"] == "TextInput")
        x0, y0, _, _ = editor["rect"]
        spot = (x0 + 8 + 2 * 9.6, y0 + 6 + 0.5 * 20.5)
        bed.click(spot, button="Right")
        until(bed, lambda: bed.exists(contains="Select All "), "the context menu opens")
        bed.screenshot("editor-menu")
        bed.click(bed.find(contains="Select All "))
        bed.click(spot, button="Right")
        until(bed, lambda: bed.exists(contains="Cut "), "the menu again")
        bed.click(bed.find(contains="Cut "))
        value = lambda: next(w for w in bed.widgets() if w["role"] == "TextInput").get("value") or ""
        until(bed, lambda: value() == "", "cut leaves nothing")
        bed.click(spot, button="Right")
        until(bed, lambda: bed.exists(contains="Paste "), "the menu once more")
        bed.click(bed.find(contains="Paste "))
        until(bed, lambda: "hello first line" in value(), "paste puts it back")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Word Wrap (Cmd+Alt+L) wraps the long line at the editor's width.
        bed.screenshot("editor-unwrapped")
        bed.key("Primary+Alt+L")
        until(bed, lambda: wrap(bed) is True, "the editor wraps")
        bed.screenshot("editor-wrapped")

        # Away and back: the editor shows the file again.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists(contains="hello first line"), "the editor is restored")
        assert wrap(bed) is True, "Word Wrap comes back with the editor"
        bed.wait(30)
        bed.screenshot("editor-restored")
