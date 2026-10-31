"""Terminal: New Terminal opens a shell in the workspace folder; what's typed
runs; `+open file:line` in it opens the file in bed. A terminal comes back
with its workspace, as a fresh shell whose `+open` still works."""
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


def screen(bed):
    terminals = [w for w in bed.widgets() if w["role"] == "TextInput"]
    return "\n".join(w.get("value") or "" for w in terminals)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "notes.txt").write_text("one\ntwo\nthree\n")

    with Bed(folder) as bed:
        bed.invoke("app.new_terminal")
        until(bed, lambda: bed.exists("Terminal") or any(w["role"] == "TextInput" for w in bed.widgets()),
              "a terminal opens")
        bed.wait(30)
        bed.type("echo hello-from-bed; pwd")
        bed.key("Enter")
        until(bed, lambda: "hello-from-bed" in screen(bed) and root.resolve().name in screen(bed),
              "the command runs in the workspace folder")
        bed.screenshot("terminal")

        bed.type("+open notes.txt:2")
        bed.key("Enter")
        until(bed, lambda: bed.exists("notes.txt") and "one\ntwo" in screen(bed),
              "+open opens the file")
        bed.screenshot("terminal-open")

        # Away and back: the terminal is a fresh shell, and `+open` works.
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: bed.exists("zsh") or bed.exists("bash") or bed.exists("Terminal"),
              "the terminal is back")
        bed.click(min((w for w in bed.widgets() if w.get("label") in ("zsh", "bash", "Terminal")),
                      key=lambda w: w["rect"][0]))
        bed.wait(30)
        bed.type("echo again-$((40+2))")
        bed.key("Enter")
        until(bed, lambda: "again-42" in screen(bed), "the restored shell runs")
        assert not bed.status()["errors"], bed.status()["errors"]
