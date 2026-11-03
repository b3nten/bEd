"""Terminal: New Terminal opens a shell in the workspace folder; what's typed
runs; `+open file:line` in it opens the file in bed. With shell integration
(zsh, bash), each command gets a bar saying how it went; Cmd-clicking a
path in the output opens it. A terminal comes back with its workspace, as a
fresh shell whose `+open` still works."""
import os
import sys
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


def terminal_rect(bed):
    return next(w for w in bed.widgets() if w["role"] == "TextInput"
                and "notes.txt:3" in (w.get("value") or ""))["rect"]


def screen(bed):
    terminals = [w for w in bed.widgets() if w["role"] == "TextInput"]
    return "\n".join(w.get("value") or "" for w in terminals)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "notes.txt").write_text("one\ntwo\nthree\n")
    (root / "other").mkdir()
    (root / "other" / "inside.txt").write_text("inside\n")

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

        # Command blocks: a failed command's bar says so (zsh and bash).
        if Path(os.environ.get("SHELL", "")).name in ("zsh", "bash"):
            bed.type("false")
            bed.key("Enter")
            until(bed, lambda: bed.exists(contains="Exit 1"), "the failed command is marked")

            # Cmd-click a path printed at the top of a cleared screen.
            bed.type("clear; printf 'notes.txt:3\\n'")
            bed.key("Enter")
            until(bed, lambda: screen(bed).startswith("notes.txt:3"), "the path is printed")
            x0, y0, _, _ = terminal_rect(bed)
            at = {"move": {"x": x0 + 14 + 3, "y": y0 + 12 + 3}}
            bed.input({"key_down": "SuperLeft" if sys.platform == "darwin" else "ControlLeft"}, at,
                      {"press": "Left"}, {"release": "Left"},
                      {"key_up": "SuperLeft" if sys.platform == "darwin" else "ControlLeft"})
            until(bed, lambda: bed.exists("notes.txt") and "three" in screen(bed), "the path opens")
            bed.screenshot("terminal-link")
            bed.click(min((w for w in bed.widgets() if w.get("label") == "zsh" or w.get("label") == "bash"),
                          key=lambda w: w["rect"][0]))
            bed.wait(10)

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

        # `+open` alone opens the shell's folder as the workspace.
        bed.type("cd other")
        bed.key("Enter")
        bed.wait(20)
        def open_here():
            bed.type("+open")
            bed.key("Enter")
        bed.relaunched(open_here)
        until(bed, lambda: bed.exists("inside.txt"), "the folder is the workspace")
        bed.screenshot("terminal-open-folder")
