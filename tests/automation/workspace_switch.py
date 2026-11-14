"""Switching workspace in place: nothing of the old one comes along (its
files, Git state, terminal), the next one starts from its own layout, and
switching back brings the first one's layout back."""
import subprocess
import tempfile
import time
from pathlib import Path
from bed import Bed


def until(bed, check, what, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)


def terminal(bed):
    return any(bed.exists(label) for label in ("zsh", "bash", "Terminal"))


with tempfile.TemporaryDirectory() as repo, tempfile.TemporaryDirectory() as other:
    repo, other = Path(repo), Path(other)
    git(repo, "init", "-q", "-b", "trunk")
    (repo / "notes.txt").write_text("one\ntwo\n")
    (other / "elsewhere.txt").write_text("hello\n")

    with Bed(repo, recent=[other]) as bed:
        # The repository: its files, an editor, Git and a terminal.
        until(bed, lambda: bed.exists("notes.txt"), "the repository is listed")
        bed.click("notes.txt")
        bed.invoke("git.show")
        until(bed, lambda: bed.exists("trunk"), "Git shows the branch")
        bed.invoke("terminal.new")
        until(bed, lambda: terminal(bed), "a terminal opens")
        bed.screenshot("workspace-switch-before")

        # The other folder: none of that comes along.
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("elsewhere.txt"), "the other folder is listed")
        assert not bed.exists("notes.txt"), "the old folder's file came along"
        assert not bed.exists("trunk"), "the old Git state came along"
        assert not terminal(bed), "the old terminal came along"
        # Git there is the new folder's, which isn't a repository.
        bed.invoke("git.show")
        bed.wait(30)
        assert not bed.exists("trunk"), "Git kept the old repository"
        bed.screenshot("workspace-switch-other")

        # Back: the repository's layout, with Git and a terminal.
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("notes.txt"), "the repository is listed again")
        until(bed, lambda: terminal(bed), "the terminal is back")
        # Git shares the terminal's area; its tab shows the repository again.
        bed.click("Git")
        until(bed, lambda: bed.exists("trunk"), "Git is back, on the repository")
        assert not bed.exists("elsewhere.txt"), "the other folder's file came along"
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("workspace-switch-back")
