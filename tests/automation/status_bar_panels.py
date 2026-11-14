"""Status bar: the focused editor shows where its caret is and its language,
and keeps showing them while a panel without status items (Files) has
focus."""
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


def double_click(bed, widget):
    x0, y0, x1, y1 = widget["rect"]
    at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
    bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})


with tempfile.TemporaryDirectory() as folder:
    (Path(folder) / "main.rs").write_text("fn main() {\n    println!(\"hi\");\n}\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("main.rs"), "the root is listed")
        double_click(bed, row(bed, "main.rs"))
        until(bed, lambda: bed.exists(contains="println"), "main.rs opens")
        until(bed, lambda: bed.exists("Rust"), "the language is shown")
        until(bed, lambda: bed.exists(contains="Ln "), "the caret is shown")
        # The double-click's second click can land in the opened editor.
        bed.key("Primary+Up")
        until(bed, lambda: bed.exists("Ln 1, Col 1"), "the caret is at the start")

        bed.key("Down")
        bed.key("End")
        until(bed, lambda: bed.exists("Ln 2, Col 20"), "the caret moves")
        bed.key("Shift+Home")
        until(bed, lambda: bed.exists(contains="characters selected"), "the selection is counted")
        bed.screenshot("status_bar_editor")

        # Files has no items: the editor's stay.
        bed.click(row(bed, "main.rs"))
        bed.wait(5)
        assert bed.exists("Rust"), "the editor's items stay while Files has focus"
        bed.screenshot("status_bar_files_focused")
