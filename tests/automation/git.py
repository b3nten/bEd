"""Git: the panel lists changes; clicking a file shows its diff, whose change
is staged from its header; a message and Commit Staged commit it."""
import subprocess
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def git(root, *args):
    return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True).stdout


def rightmost(bed, label):
    return max(bed.find_all(label), key=lambda widget: widget["rect"][0])


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    git(root, "init", "-q", "-b", "main")
    git(root, "config", "user.email", "bed@example.com")
    git(root, "config", "user.name", "bed")
    (root / "notes.txt").write_text("one\ntwo\nthree\nfour\nfive\n")
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", "first")
    (root / "notes.txt").write_text("one\nTWO\nthree\nfour\nfive\n")
    (root / "new.txt").write_text("new file\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("notes.txt"), "the root is listed")
        bed.invoke("app.open_git")
        until(bed, lambda: bed.exists("main"), "the Git panel shows the branch")
        until(bed, lambda: len(bed.find_all("notes.txt")) >= 2 and len(bed.find_all("new.txt")) >= 2,
              "the changes are listed")
        bed.screenshot("git")

        # The diff: its change, staged from its header.
        bed.click(rightmost(bed, "notes.txt"))
        until(bed, lambda: bed.exists("Stage"), "the diff shows its change")
        bed.screenshot("git-diff")

        # Edit in the diff: a second change shows inline.
        editor = max((w for w in bed.widgets() if w["role"] in ("TextInput", "MultilineTextInput")),
                     key=lambda widget: widget["rect"][0])
        bed.click(editor)
        bed.key("Primary+ArrowDown")
        bed.key("ArrowUp")
        bed.key("End")
        bed.type(" more")  # on "five"
        until(bed, lambda: len(bed.find_all("Stage")) == 2, "the edit shows as a second change")
        bed.screenshot("git-diff-edited")

        # Stage only the first change: the file is saved first; the edit stays unstaged.
        first = min(bed.find_all("Stage"), key=lambda widget: widget["rect"][1])
        bed.click(first)
        until(bed, lambda: "notes.txt" in git(root, "diff", "--cached", "--name-only"), "the change is staged")
        assert (root / "notes.txt").read_text() == "one\nTWO\nthree\nfour\nfive more\n", \
            (root / "notes.txt").read_text()
        staged = git(root, "show", ":notes.txt")
        assert staged == "one\nTWO\nthree\nfour\nfive\n", staged

        # Commit it.
        message = next(w for w in bed.widgets() if w["role"] == "MultilineTextInput")
        bed.click(message)
        bed.type("Shout two")
        until(bed, lambda: bed.exists("Commit Staged") and not bed.find("Commit Staged").get("disabled"),
              "Commit Staged is ready")
        bed.click(bed.find("Commit Staged"))
        until(bed, lambda: "Shout two" in git(root, "log", "--oneline", "-1"), "it's committed")
        assert "new.txt" in git(root, "status", "--porcelain"), "the untracked file is left alone"
        assert not bed.status()["errors"], bed.status()["errors"]
