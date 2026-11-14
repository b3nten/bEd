"""Command palette: Cmd/Ctrl+Shift+P lists what can run, including the
focused editor's own commands; choosing one runs it in that editor, and the
editor has the keyboard again afterwards. Escape closes it without running
anything; F1 opens it too."""
import tempfile
from pathlib import Path
from bed import Bed

FIELD = "Run a command"


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The tree's row (tabs with the same name are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def text(bed):
    editor = next(w for w in bed.widgets() if w["role"] == "TextInput" and w.get("label") != FIELD)
    return editor.get("value") or ""


def palette_open(bed):
    return bed.exists(FIELD, role="TextInput")


with tempfile.TemporaryDirectory() as folder:
    notes = Path(folder) / "notes.txt"
    notes.write_text("one\ntwo\nthree\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        x0, y0, x1, y1 = row(bed, "notes.txt")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="three"), "notes.txt opens")
        bed.wait(5)

        # The palette lists the editor's commands: it asked while the editor
        # had the keyboard.
        bed.key("Primary+Shift+P")
        until(bed, lambda: palette_open(bed), "the palette opens")
        bed.type("move line down")
        bed.wait(5)
        bed.screenshot("command-palette")
        bed.key("Primary+A")
        bed.type("view")
        bed.wait(5)
        bed.screenshot("command-palette-names")
        bed.key("Primary+A")
        bed.type("move line down")
        bed.wait(5)
        assert bed.exists(contains="Move Line Down"), "editor commands are listed"
        bed.key("Enter")
        until(bed, lambda: not palette_open(bed), "choosing closes it")
        until(bed, lambda: text(bed).startswith("two\none\n"), "the line moved down")

        # The editor has the keyboard again: typing goes into it.
        bed.type("x")
        until(bed, lambda: "x" in text(bed), "typing reaches the editor")
        assert text(bed).startswith("two\nxone\n"), repr(text(bed))

        # Escape closes it, runs nothing, and gives the keyboard back.
        bed.key("F1")
        until(bed, lambda: palette_open(bed), "F1 opens it")
        bed.type("move")
        bed.key("Escape")
        until(bed, lambda: not palette_open(bed), "Escape closes it")
        bed.type("y")
        until(bed, lambda: "y" in text(bed), "typing reaches the editor again")
        assert text(bed).startswith("two\nxyone\n"), repr(text(bed))

        # A command that isn't the editor's: Zoom In, from the View menu.
        bed.key("Primary+Shift+P")
        until(bed, lambda: palette_open(bed), "the palette opens again")
        bed.type("zoom in")
        bed.wait(5)
        bed.key("Enter")
        until(bed, lambda: not palette_open(bed), "it closes")
        assert not bed.status()["errors"], bed.status()["errors"]
