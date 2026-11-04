"""Panel zoom: Cmd+= and Cmd+- zoom the focused panel's content, not its tab,
the other panels, or the menus it opens; Cmd+0 puts it back. The zoom is saved
with the panel. An image zooms its picture instead."""
import shutil
import tempfile
from pathlib import Path
from bed import Bed

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"
ZOOM = "bedtime_ui::layouts::PanelZoom"


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The tree's row (tabs with the same name are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def tab(bed, name):
    return max(bed.find_all(name), key=lambda widget: widget["rect"][0])


def zooms(bed):
    """Each zoomed panel's zoom."""
    rows = bed.call("world.query", {"data": {"components": [ZOOM]}})
    return sorted(round(r["components"][ZOOM], 2) for r in rows)


def height(widget):
    return widget["rect"][3] - widget["rect"][1]


def open_for_good(bed, name):
    x0, y0, x1, y1 = row(bed, name)["rect"]
    at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
    bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})


def menu_item_height(bed, spot):
    bed.click(spot, button="Right")
    until(bed, lambda: bed.exists(contains="Select All "), "the editor's menu opens")
    item = height(bed.find(contains="Select All "))
    bed.key("Escape")
    until(bed, lambda: not bed.exists(contains="Select All "), "the menu closes")
    return item


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "notes.txt").write_text("some notes to read\nsecond line\n")
    shutil.copy(FIXTURES / "images" / "sample.png", root / "sample.png")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        open_for_good(bed, "notes.txt")
        until(bed, lambda: bed.exists(contains="some notes to read"), "notes.txt opens")
        bed.wait(5)
        editor = next(w for w in bed.widgets() if w["role"] == "TextInput")
        spot = (editor["rect"][0] + 40, editor["rect"][1] + 12)
        tab_before = tab(bed, "notes.txt")["rect"]
        row_before = row(bed, "notes.txt")["rect"]
        menu_before = menu_item_height(bed, spot)

        # Two steps up: 125%, shown for a moment in the corner.
        bed.key("Primary+=")
        bed.key("Primary+=")
        until(bed, lambda: zooms(bed) == [1.25], "the editor zooms")
        bed.screenshot("zoom-editor")
        assert tab(bed, "notes.txt")["rect"] == tab_before, "the tab zoomed"
        assert row(bed, "notes.txt")["rect"] == row_before, "the file tree zoomed"
        assert menu_item_height(bed, spot) == menu_before, "the menu zoomed"
        bed.key("Primary+-")
        until(bed, lambda: zooms(bed) == [1.1], "zoom out steps down")

        # An image zooms its picture instead.
        bed.click(row(bed, "sample.png"))
        until(bed, lambda: bed.exists(contains="· PNG"), "sample.png shows")
        x0, _, x1, y1 = tab(bed, "sample.png")["rect"]
        bed.click((x0 + 200, y1 + 150))
        picture = lambda: bed.find(contains="%")["value"]
        before = picture()
        bed.key("Primary+=")
        until(bed, lambda: picture() != before, "the picture zooms")
        assert zooms(bed) == [1.1], "the image panel zoomed rather than its picture"
        assert not bed.status()["errors"], bed.status()["errors"]

        # Away and back: still zoomed; Cmd+0 puts it back.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: len(bed.find_all("notes.txt")) == 2, "the editor's tab is restored")
        assert zooms(bed) == [1.1], "the zoom wasn't saved"
        bed.click(tab(bed, "notes.txt"))
        until(bed, lambda: bed.exists(contains="some notes to read"), "the editor shows")
        bed.key("Primary+0")
        until(bed, lambda: zooms(bed) == [], "Cmd+0 resets it")

        # A terminal zooms too (it doesn't take the keys for the shell).
        bed.invoke("terminal.new")
        shell = lambda: [w for w in bed.find_all(role="TextInput")
                         if "some notes" not in (w.get("value") or "")]
        until(bed, lambda: shell(), "a terminal opens")
        bed.wait(30)
        bed.key("Primary+=")
        until(bed, lambda: zooms(bed) == [1.1], "the terminal zooms")
        bed.screenshot("zoom-terminal")
        assert not bed.status()["errors"], bed.status()["errors"]
