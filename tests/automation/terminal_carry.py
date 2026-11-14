"""`+open` from a terminal in the default workspace takes its shells along:
the folder's workspace opens with them still running (their variables,
output and folder), and `+open` from them still works. Where the folder's
workspace has terminals of its own, the carried shells take their places.
(Leaving a folder's workspace carries nothing: workspace_switch.py.)"""
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


SHELLS = ("zsh", "bash", "sh", "Terminal")


def terminal_tabs(bed):
    return [w for w in bed.widgets() if w.get("label") in SHELLS]


def terminal_tab(bed):
    return min(terminal_tabs(bed), key=lambda w: w["rect"][0])


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder).resolve()
    (root / "notes.txt").write_text("one\ntwo\nthree\n")
    (root / "other").mkdir()
    (root / "other" / "inside.txt").write_text("inside\n")

    # Effects keep bed updating continuously: otherwise, unfocused with a
    # terminal open, it can sleep through automation requests.
    with Bed(settings={"effects.enabled": True}) as bed:
        # The default workspace, with a shell that has some state.
        bed.invoke("terminal.new")
        until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "a terminal opens")
        bed.wait(30)
        bed.type(f"export CARRIED=yes-$((6*7)); cd '{root}'; echo before-the-switch")
        bed.key("Enter")
        until(bed, lambda: "before-the-switch" in screen(bed), "the shell runs")

        def open_here():
            bed.type("+open")
            bed.key("Enter")
        bed.switched(open_here)
        until(bed, lambda: bed.exists("notes.txt"), "the folder is the workspace")

        # The same shell, in the folder's workspace.
        until(bed, lambda: "before-the-switch" in screen(bed), "the terminal came along, with its output")
        bed.click(terminal_tab(bed))
        bed.wait(10)
        bed.type("echo carried-$CARRIED in-$(basename \"$PWD\")")
        bed.key("Enter")
        until(bed, lambda: f"carried-yes-42 in-{root.name}" in screen(bed), "it's the same shell")
        bed.screenshot("terminal-carry")

        # Its `+open` answers the new panel.
        bed.type("+open notes.txt:2")
        bed.key("Enter")
        until(bed, lambda: "one\ntwo" in screen(bed), "+open from the carried shell opens the file")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Back to the default workspace, which restores a fresh terminal, and
        # `+open` again: the folder's own terminal (saved last time) gives its
        # place to the carried shell.
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        until(bed, lambda: terminal_tabs(bed), "the default workspace's terminal is back")
        bed.click(terminal_tab(bed))
        bed.wait(30)
        bed.type(f"export AGAIN=twice; cd '{root}'; echo second-trip")
        bed.key("Enter")
        until(bed, lambda: "second-trip" in screen(bed), "the fresh shell runs")
        bed.switched(open_here)
        until(bed, lambda: bed.exists("notes.txt") and "second-trip" in screen(bed),
              "the shell came along again")
        until(bed, lambda: len(terminal_tabs(bed)) == 1, "one terminal: the carried shell, in place")
        bed.wait(20)
        assert len(terminal_tabs(bed)) == 1, [w["label"] for w in terminal_tabs(bed)]
        bed.click(terminal_tab(bed))
        bed.wait(10)
        bed.type("echo still-$AGAIN")
        bed.key("Enter")
        until(bed, lambda: "still-twice" in screen(bed), "it's the carried shell")
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.screenshot("terminal-carry-replaced")
