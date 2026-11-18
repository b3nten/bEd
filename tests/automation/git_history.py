"""Git History: the panel lists commits with the lines of history; a commit
selected shows what it says and changed (under the list when narrow, beside
it when wide); a file clicked there opens with the commit's changes; a new
commit shows up by itself."""
import os
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
    bed.screenshot("git-history-failed")
    raise AssertionError(f"never: {what}")


clock = [int(time.time()) - 400 * 86400]


def git(root, *args, author="Ada Lovelace", step=0):
    if args[0] in ("commit", "merge"):
        clock[0] += step
    date = f"{clock[0]} +0000"
    env = {**os.environ, "GIT_AUTHOR_NAME": author, "GIT_AUTHOR_EMAIL": "a@example.com",
           "GIT_COMMITTER_NAME": author, "GIT_COMMITTER_EMAIL": "a@example.com",
           "GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date}
    return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True, env=env).stdout


def commit(root, name, text, message, **kwargs):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    git(root, "add", ".", **kwargs)
    git(root, "commit", "-q", "-m", message, **kwargs)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    git(root, "init", "-q", "-b", "main")
    day = 86400
    commit(root, "README.md", "# Tide tables\n", "Start the project", step=day)
    commit(root, "src/tides.py", "def height(t):\n    return 0\n", "Add a tide model", step=30 * day)
    for i in range(30):
        commit(root, "src/tides.py", f"def height(t):\n    return {i + 1}\n", f"Tune the harmonic constants, pass {i + 1}",
               author="Grace Hopper" if i % 3 else "Ada Lovelace", step=5 * day)
    git(root, "tag", "v0.1")
    git(root, "switch", "-q", "-c", "moon-phase")
    commit(root, "src/moon.py", "def phase(t):\n    return 'new'\n", "Work out the moon's phase", author="Katherine Johnson", step=20 * day)
    commit(root, "src/moon.py", "def phase(t):\n    return 'full'\n\n\ndef age(t):\n    return 14\n", "Give the moon an age",
           author="Katherine Johnson", step=day)
    git(root, "switch", "-q", "main")
    commit(root, "src/tides.py", "def height(t):\n    return 31\n\n\ndef low(t):\n    return -height(t)\n", "Add low water",
           author="Grace Hopper", step=day)
    git(root, "merge", "-q", "--no-ff", "moon-phase", "-m", "Merge moon-phase: spring and neap tides", step=day)
    git(root, "update-ref", "refs/remotes/origin/main", "HEAD")
    git(root, "mv", "README.md", "ABOUT.md", step=5 * day)
    (root / "ABOUT.md").write_text("# Tide tables\n\nHeights for any port, from harmonic constants.\n")
    (root / "chart.bin").write_bytes(bytes(range(256)))
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", "Say what this is\n\nThe README becomes ABOUT, with a line on what the\ntables are worked out from. A chart goes in beside it.", step=3600)
    clock[0] = int(time.time()) - 7200
    commit(root, "src/tides.py", "def height(t):\n    return 32\n\n\ndef low(t):\n    return -height(t)\n", "Correct the datum", step=0)

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("ABOUT.md"), "the root is listed")
        # In the focused area, the explorer's: narrow.
        bed.invoke("git.history")
        until(bed, lambda: bed.exists("Correct the datum"), "the commits are listed")
        assert bed.exists("Merge moon-phase: spring and neap tides")
        bed.screenshot("git-history-narrow")

        bed.click("Say what this is")
        until(bed, lambda: bed.exists("chart.bin", visible=False), "the commit's files are listed")
        bed.screenshot("git-history-narrow-detail")
        bed.scroll(-400, at=bed.find(contains="files changed"))
        until(bed, lambda: bed.exists("chart.bin"), "the files scroll into view")

        # A file's changes in that commit open in the large area.
        bed.click(max(bed.find_all("ABOUT.md"), key=lambda widget: widget["rect"][1]))
        until(bed, lambda: any((w.get("label") or "").startswith("ABOUT.md (") for w in bed.widgets()),
              "the commit's changes to the file open")
        bed.wait(20)
        bed.screenshot("git-history-diff")

        # Down moves to the next commit; Escape puts the details away.
        bed.click("Say what this is")
        bed.click("Say what this is")
        bed.key("ArrowDown")
        until(bed, lambda: bed.exists("src/moon.py", visible=False), "the merge's files are listed")
        bed.key("Escape")
        until(bed, lambda: not bed.exists("src/moon.py", visible=False), "the details close")

        # A commit made elsewhere shows up.
        commit(root, "src/tides.py", "def height(t):\n    return 33\n", "Round to the centimetre")
        until(bed, lambda: bed.exists("Round to the centimetre"), "the new commit is listed")

        # In the large area: wide, with the details beside the list.
        bed.click(next(w for w in bed.widgets() if (w.get("label") or "").startswith("ABOUT.md (")))
        bed.invoke("git.history")
        until(bed, lambda: len(bed.find_all("Correct the datum")) == 2, "a second panel lists the commits")
        wide = max(bed.find_all("Correct the datum"), key=lambda widget: widget["rect"][2])
        bed.screenshot("git-history-wide")
        bed.click(max(bed.find_all("Merge moon-phase: spring and neap tides"), key=lambda widget: widget["rect"][2]))
        until(bed, lambda: bed.exists("src/moon.py"), "the merge's files are listed")
        bed.screenshot("git-history-wide-detail")
        assert not bed.status()["errors"], bed.status()["errors"]
