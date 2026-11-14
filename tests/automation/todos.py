"""TODOs: the workspace's TODO comments show in a tree of folders and files,
and as a list; clicking one opens its file; a saved edit is found again."""
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
    bed.screenshot("todos_failed")
    raise AssertionError(f"never: {what}")


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "src").mkdir()
    (root / "src" / "main.rs").write_text(
        "fn main() {\n    // TODO: say hello\n    // FIXME(ben): exit code\n}\n"
    )
    (root / "notes.py").write_text("# HACK quick fix\nx = 1\n")
    with Bed(folder, settings={"effects.enabled": True}) as bed:
        window = bed.status()["window"]
        bed.click((window["width"] * 0.6, window["height"] * 0.5))
        bed.wait(5)
        bed.invoke("code.show_todos")
        until(bed, lambda: bed.exists(contains="say hello"), "the TODOs are found")
        assert bed.exists(contains="exit code") and bed.exists(contains="quick fix")
        assert bed.exists(contains="3 in 2 files"), "the summary counts them"
        bed.screenshot("todos_tree")

        # Collapse the file: its items go.
        bed.click("Collapse src/main.rs")
        until(bed, lambda: not bed.exists(contains="say hello"), "collapsing hides the items")
        bed.click("Expand src/main.rs")
        until(bed, lambda: bed.exists(contains="say hello"), "expanding shows them")

        # As a list, then filtered.
        bed.click("Tree")
        until(bed, lambda: bed.exists(contains="src/main.rs:2"), "the list shows where each is")
        bed.screenshot("todos_list")
        bed.click("Filter", role="TextInput")
        bed.type("exit")
        until(bed, lambda: not bed.exists(contains="say hello"), "the filter narrows the list")
        assert bed.exists(contains="exit code")
        bed.key("Primary+A")
        bed.key("Backspace")

        # Clicking one opens its file.
        bed.click(contains="say hello")
        until(bed, lambda: bed.exists("main.rs", role="Tab") or bed.exists(contains="fn main"),
              "the file opens")
        bed.screenshot("todos_opened")

        # A new TODO, saved, is found.
        (root / "notes.py").write_text("# HACK quick fix\n# TODO: brand new\n")
        bed.click("Refresh")
        until(bed, lambda: bed.exists(contains="brand new"), "a refresh finds new ones")

print("ok")
