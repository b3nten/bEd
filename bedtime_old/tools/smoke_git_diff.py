#!/usr/bin/env python3
"""Exercise editable inline comparisons and actual Git chunk buttons."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time

from automation import Application
from smoke_bed import open_panel, prepare_demo_layout, panels
from smoke_editor import primary
from smoke_git import git, settled, control, capture_checked
from smoke_workspace_policy import clean_quit


def wait_diff(app, side="Unstaged", *, entity=None, controls=True):
    deadline = time.monotonic() + 30
    last = None
    while time.monotonic() < deadline:
        last = app.request("app.inspect_git")
        for diff in last["diffs"]:
            if diff["descriptor"]["side"] == side and (entity is None or diff["entity"] == entity):
                if diff["attached"] and not diff["loading"] and (diff["controls"] or not controls):
                    return diff
        app.request("wait_frames", frames=2)
    raise AssertionError(f"Comparison did not settle: {last}")


def click_rect(app, rect):
    zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
    app.input({"type": "click", "position": [(a+b)*zoom/2 for a,b in zip(rect["min"],rect["max"])]})


def chunk(app, entity, action, index):
    value = wait_diff(app, "Staged" if action == 2 else "Unstaged", entity=entity)
    button = next(b for b in value["controls"] if b["action"] == action and b["chunk"] == index)
    click_rect(app, button)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/git-diff-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    captures = []
    previous = os.environ.get("XDG_CONFIG_HOME")
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as temporary:
            fixture = Path(temporary)
            project = fixture / "project"
            project.mkdir()
            os.environ["XDG_CONFIG_HOME"] = str(fixture / "config")
            git(project, "init", "-b", "main")
            for key, value in [("user.name", "Bed Test"), ("user.email", "bed@example.invalid"), ("commit.gpgsign", "false")]:
                git(project, "config", key, value)
            path = project / "a.txt"
            baseline = "one\ntwo\nthree\nfour\nfive\nsix\n"
            path.write_text(baseline)
            large = project / "large.txt"
            large.write_text("".join(f"line {i}\n" for i in range(100000)))
            git(project, "add", ".")
            git(project, "commit", "-m", "Initial")
            path.write_text("ONE\ntwo\nthree\nfour\nFIVE\nsix\n")
            large.write_text(large.read_text().replace("line 1\n", "changed first\n").replace("line 99998\n", "changed last\n"))
            command = [str(Path(args.binary).resolve()), str(project)]
            with Application(command, output=output, timeout=60) as app:
                prepare_demo_layout(app, viewport=False)
                app.request("configure", fixed_dt=1/60, clipboard="")
                git_panel = open_panel(app, "Git")
                settled(app)
                control(app, "file:a.txt")
                working = wait_diff(app)
                working_id = working["entity"]
                tab = next(p for p in panels(app.inspect()) if p["entity"] == working_id)
                assert tab["preview"], tab
                assert len([b for b in working["controls"] if b["action"] == 1]) == 2
                captures.append(capture_checked(app, "01-inline.png"))
                app.request("app.navigate_editor", panel=working_id, revision=working["revision"], start=0, end=3, anchor=0, head=3, focus=True)
                app.request("wait_frames", frames=2)
                app.input({"type":"text", "text":"LIVE"})
                working = wait_diff(app, entity=working_id)
                assert not next(p for p in panels(app.inspect()) if p["entity"] == working_id)["preview"]
                chunk(app, working_id, 1, 0)
                settled(app)
                assert git(project, "show", ":a.txt") == baseline.replace("one\n", "LIVE\n", 1)
                assert path.read_text() == baseline.replace("one\n", "LIVE\n", 1).replace("five\n", "FIVE\n")
                wait_diff(app, entity=working_id)
                captures.append(capture_checked(app, "02-partial-stage.png"))
                app.click_text("Open file")
                app.request("wait_frames", frames=10)
                code = next(p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(path))
                assert code["document"] == working["document"]
                app.request("panel.focus", panel=code["entity"])
                primary(app, "KeyZ")
                primary(app, "KeyZ", shift=True)
                primary(app, "KeyS")
                app.request("panel.focus", panel=git_panel)
                control(app, "file:a.txt", button="Right")
                app.click_text("Open staged diff")
                staged = wait_diff(app, "Staged")
                assert staged["document"] is None
                captures.append(capture_checked(app, "03-staged.png"))
                chunk(app, staged["entity"], 2, 0)
                settled(app)
                assert git(project, "show", ":a.txt") == baseline
                app.request("panel.focus", panel=working_id)
                wait_diff(app, entity=working_id)
                chunk(app, working_id, 3, 1)
                app.request("wait_frames", frames=3)
                app.click_text("Discard chunk")
                settled(app)
                assert path.read_text() == baseline.replace("one\n", "LIVE\n", 1)
                captures.append(capture_checked(app, "04-discard-chunk.png"))
                app.request("panel.focus", panel=git_panel)
                control(app, "file:large.txt")
                huge = wait_diff(app)
                assert huge["rendered_lines"] < 100 and huge["laid_out_bytes"] < 20000, huge
                captures.append(capture_checked(app, "05-large.png"))
                # Pin through a second click/Enter, then persist and restore both sides.
                app.request("panel.focus", panel=git_panel)
                control(app, "file:large.txt")
                app.input({"type":"key","code":"Enter","pressed":True}, {"type":"key","code":"Enter","pressed":False})
                clean_quit(app)
            with Application(command, output=output / "restore", timeout=60) as app:
                restored = wait_diff(app, controls=False)
                assert restored["descriptor"]["root"] == {"platform": "unix_bytes", "units": list(os.fsencode(project))}
                captures.append(capture_checked(app, "06-restored.png"))
                clean_quit(app)
    finally:
        if previous is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous
    (output / "summary.json").write_text(json.dumps({"ok":True,"captures":captures}, indent=2)+"\n")
    print(json.dumps({"ok":True,"captures":len(captures),"output":str(output)}))


if __name__ == "__main__":
    main()
