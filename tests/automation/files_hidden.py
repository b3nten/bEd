"""Files: hidden and Git-ignored files show (dimmed) until the tree's menu
hides them; the choice is saved with the tree."""
import subprocess
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    subprocess.run(["git", "init", "-q", folder], check=True)
    (root / ".gitignore").write_text("*.log\nbuild/\n")
    (root / "build").mkdir()
    (root / "build.log").write_text("")
    (root / "notes.txt").write_text("notes\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("build.log") and bed.exists(".gitignore"), "everything is listed")
        bed.screenshot("files-hidden-shown")

        # The empty space's menu hides ignored files.
        x0, _, x1, y1 = bed.find("notes.txt")["rect"]
        empty = ((x0 + x1) / 2, y1 + 120)
        bed.click(empty, button="Right")
        until(bed, lambda: bed.exists("Show Ignored Files"), "the menu opens")
        bed.screenshot("files-hidden-menu")
        bed.click("Show Ignored Files")
        until(bed, lambda: not bed.exists("build.log"), "ignored files go")
        assert not bed.exists("build"), "an ignored folder stayed"
        assert bed.exists(".gitignore") and bed.exists("notes.txt")

        # So does the action, for hidden files.
        bed.invoke("files.toggle_hidden")
        until(bed, lambda: not bed.exists(".gitignore"), "hidden files go")
        assert not bed.exists(".git")
        assert bed.exists("notes.txt")
        bed.screenshot("files-hidden-hidden")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Away and back: still hidden.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("notes.txt"), "the tree is restored")
        bed.wait(20)
        assert not bed.exists(".gitignore") and not bed.exists("build.log"), "the tree forgot"
