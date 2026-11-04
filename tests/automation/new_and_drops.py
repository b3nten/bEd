"""New and drops: Cmd+N makes an untitled document beside the focused editor,
and another terminal beside the focused terminal. Files dropped from outside
open where they land; dropped on a folder in the tree, they're copied in."""
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(300):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def tabs(bed, name):
    return len([w for w in bed.widgets() if (w.get("label") or "") == f"Close {name}"])


with tempfile.TemporaryDirectory() as folder, tempfile.TemporaryDirectory() as elsewhere:
    root = Path(folder)
    (root / "src").mkdir()
    (root / "notes.txt").write_text("notes\n")
    outside = Path(elsewhere) / "outside.txt"
    outside.write_text("from outside\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")

        # Cmd+N beside a focused editor: an untitled document to type in.
        bed.input({"move": {"x": 1, "y": 1}})
        x0, y0, x1, y1 = row(bed, "notes.txt")["rect"]
        bed.input({"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}},
                  {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="notes"), "notes.txt opens")
        editor = next(w for w in bed.widgets() if w["role"] == "TextInput")
        bed.click(editor)
        bed.key("Primary+N")
        until(bed, lambda: tabs(bed, "Untitled") == 1, "an untitled document opens")
        bed.type("scratch")
        until(bed, lambda: any(w["role"] == "TextInput" and w.get("value") == "scratch" for w in bed.widgets()),
              "it's typed in")
        bed.screenshot("new-untitled")

        # Dropped from outside onto the editor area: it opens there.
        width = bed.status()["window"]["width"]
        bed.drop_files([outside], (width - 200, 400))
        until(bed, lambda: bed.exists(contains="from outside"), "the dropped file opens")

        # Dropped on a folder in the tree: copied in.
        bed.drop_files([outside], (row(bed, "src")["rect"][0] + 20, (row(bed, "src")["rect"][1] + row(bed, "src")["rect"][3]) / 2))
        until(bed, lambda: (root / "src" / "outside.txt").exists(), "the file is copied into src")
        assert outside.exists(), "a copy, not a move"

        # Cmd+N in a terminal: another terminal beside it.
        bed.invoke("terminal.new")
        until(bed, lambda: tabs(bed, "zsh") + tabs(bed, "bash") >= 1, "a terminal opens")
        bed.wait(30)
        before = tabs(bed, "zsh") + tabs(bed, "bash")
        bed.key("Primary+N")
        until(bed, lambda: tabs(bed, "zsh") + tabs(bed, "bash") == before + 1, "another terminal opens")
        bed.screenshot("new-terminal")
        assert not bed.status()["errors"], bed.status()["errors"]
