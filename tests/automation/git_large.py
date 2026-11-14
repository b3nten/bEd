"""Git diffs of large files stay usable: a file with thousands of changes, a
rewritten one, megabyte-long lines and a deleted one open in each other's
place; the diff is scrolled, edited and wrapped, and survives the file
changing on disk under it."""
import subprocess
import tempfile
import time
from pathlib import Path
from bed import Bed

LINES = 60_000


def until(bed, check, what, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def git(root, *args):
    return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True).stdout


def row(bed, name):
    """The file's row in the Git panel (left of any diff tab with its name)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def editor(bed):
    return max((w for w in bed.widgets() if w["role"] in ("TextInput", "MultilineTextInput")),
               key=lambda widget: widget["rect"][0])


base = [f"line {i} {'x' * (i % 60)}\n" for i in range(LINES)]
files = {
    # name: (committed, working)
    "hunks.txt": ("".join(base), "".join("CHANGED\n" if i % 10 == 0 else line for i, line in enumerate(base))),
    "rewrite.txt": ("".join(base), "".join("new " + line for line in base)),
    "long.txt": ("".join("a" * 200_000 + "\n" for _ in range(10)), "".join("b" * 200_000 + "\n" for _ in range(10))),
    "gone.txt": ("".join(base), None),
}

with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    git(root, "init", "-q", "-b", "main")
    git(root, "config", "user.email", "bed@example.com")
    git(root, "config", "user.name", "bed")
    for name, (committed, _) in files.items():
        (root / name).write_text(committed)
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", "first")
    for name, (_, working) in files.items():
        if working is None:
            (root / name).unlink()
        else:
            (root / name).write_text(working)

    with Bed(folder) as bed:
        bed.invoke("git.show")
        until(bed, lambda: bed.exists("main") and all(bed.exists(name) for name in files),
              "the changes are listed")

        # Each diff opens in the last one's place, whether or not that one
        # had finished reading.
        for name in list(files) * 2:
            bed.click(row(bed, name))
        for name in files:
            bed.click(row(bed, name))
            until(bed, lambda: bed.exists(f"{name} (Changes)") and bed.exists("Stage"),
                  f"{name}'s diff shows")
        bed.screenshot("git-large-deleted")

        # Thousands of changes: scrolled, edited at both ends, wrapped.
        bed.click(row(bed, "hunks.txt"))
        until(bed, lambda: bed.exists("hunks.txt (Changes)") and len(bed.find_all("Stage")) > 1,
              "the changes show")
        bed.click(editor(bed))
        for _ in range(5):
            bed.scroll(-3000, at=editor(bed))
        bed.key("Primary+ArrowDown")
        bed.type("end")
        bed.key("Primary+ArrowUp")
        bed.type("start")
        bed.invoke("editor.toggle_word_wrap")
        bed.key("Primary+ArrowDown")
        bed.key("Primary+ArrowUp")
        bed.invoke("editor.toggle_word_wrap")
        for _ in range(2):
            bed.key("Primary+z")
        bed.screenshot("git-large")

        # Megabyte lines, wrapped and not.
        bed.click(row(bed, "long.txt"))
        until(bed, lambda: bed.exists("long.txt (Changes)") and bed.exists("Stage"), "the long lines show")
        bed.click(editor(bed))
        bed.key("End")
        bed.invoke("editor.toggle_word_wrap")
        bed.key("Primary+ArrowDown")
        bed.invoke("editor.toggle_word_wrap")

        # The file changes on disk under its diff: shorter, then gone.
        bed.click(row(bed, "rewrite.txt"))
        until(bed, lambda: bed.exists("rewrite.txt (Changes)") and bed.exists("Stage"), "the rewrite shows")
        (root / "rewrite.txt").write_text("".join(base[: LINES // 3]))
        bed.wait(60)
        bed.click(editor(bed))
        bed.key("Primary+ArrowDown")
        (root / "rewrite.txt").unlink()
        bed.wait(60)
        until(bed, lambda: bed.exists("Stage"), "the deleted file's diff shows")
        assert not bed.status()["errors"], bed.status()["errors"]
