"""Files in columns: Auto (the default) shows a tree in a tall panel and
columns in a wide one; the footer's switch picks one for good (in a narrow
panel it's a menu). A column per folder down to the selection: a click on a
folder shows its entries in the next column, a click on a file previews it,
and arrow keys move within a column and between them. Back in the tree, the
folders down to the selection are open. The mode is saved with the panel."""
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def item(bed, name):
    """The explorer's row: selectable (unlike the footer's folders), and
    below any tab with the same name."""
    rows = [w for w in bed.find_all(name) if w.get("toggled") is not None]
    return max(rows, key=lambda widget: widget["rect"][1])


def selected(bed, name):
    return any(w.get("toggled") == "True" for w in bed.find_all(name))


def centre(rect):
    return ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "src" / "nested").mkdir(parents=True)
    (root / "src" / "main.rs").write_text("fn main() {}\n")
    (root / "src" / "nested" / "deep.rs").write_text("// deep\n")
    (root / "notes.txt").write_text("first line of notes\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        # The side panel is tall and narrow: a tree (src is indented under
        # the root), and the mode is a menu.
        root_row = min(bed.find_all(root.name), key=lambda widget: widget["rect"][0])
        assert item(bed, "src")["rect"][0] > root_row["rect"][0], "not a tree"
        assert bed.exists("Auto ▾"), "the narrow footer has no mode menu"
        bed.screenshot("files-columns-side")

        # Moved under the editor, it's wide: columns, the root's entries
        # first (src and notes.txt side by side in one column).
        window = bed.status()["window"]
        tab = [w for w in bed.find_all("Files", role="Button") if w["rect"][1] < 80][0]
        bed.drag(centre(tab["rect"]), (window["width"] / 2, window["height"] - 40), steps=20)
        until(bed, lambda: bed.exists("Columns"), "the footer has room for the switch")
        until(bed, lambda: item(bed, "src")["rect"][0] == item(bed, "notes.txt")["rect"][0],
              "the root's entries make the first column")
        assert selected(bed, "Auto")

        # A folder's entries show in the next column, to its right.
        bed.click(item(bed, "src"))
        until(bed, lambda: bed.exists("main.rs"), "src's column shows")
        assert item(bed, "main.rs")["rect"][0] >= item(bed, "src")["rect"][2]
        bed.click(item(bed, "nested"))
        until(bed, lambda: bed.exists("deep.rs"), "nested's column shows")
        bed.screenshot("files-columns")

        # Keys: Right goes in, Left back out, Down within a column.
        bed.key("ArrowRight")
        until(bed, lambda: selected(bed, "deep.rs"), "Right selects deep.rs")
        bed.key("ArrowLeft")
        until(bed, lambda: selected(bed, "nested"), "Left selects nested")
        bed.key("ArrowDown")
        until(bed, lambda: selected(bed, "main.rs"), "Down selects main.rs")
        until(bed, lambda: not bed.exists("deep.rs"), "nested's column goes")

        # A click on a file previews it.
        bed.click(item(bed, "main.rs"))
        until(bed, lambda: bed.exists(contains="fn main"), "main.rs previews")
        bed.screenshot("files-columns-preview")
        # In a split of the editor area, the preview can open over the
        # explorer: bring it back.
        if not bed.exists("Tree"):
            bed.click([w for w in bed.find_all("Files", role="Button")][0])
            until(bed, lambda: bed.exists("Tree"), "the explorer is back")

        # The tree, chosen: src is open, main.rs selected in it.
        bed.click("Tree")
        until(bed, lambda: selected(bed, "Tree") and item(bed, "main.rs")["rect"][1] > item(bed, "src")["rect"][1],
              "the tree opens src")
        row = item(bed, "main.rs")
        assert row.get("toggled") == "True", "main.rs isn't selected in the tree"
        assert row["rect"][0] > item(bed, "src")["rect"][0], "main.rs isn't in src"
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("files-columns-tree")

        # Chosen, it's saved with the panel.
        bed.click("Columns")
        until(bed, lambda: selected(bed, "Columns"), "Columns is chosen")
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("Columns") and selected(bed, "Columns"), "Columns is still chosen")
