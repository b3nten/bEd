#!/usr/bin/env python3
"""Native launch-and-quit, exclusion, unsaved decisions and restoration."""
import argparse
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
from automation import Application, AutomationError
from smoke_bed2 import ready, center


def wait(app, predicate):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        app.request("wait_frames", frames=2)
    raise AssertionError("Session operation did not settle")


def folder_result(app, root):
    app.request("app.open_folder", path=str(root))
    return wait(app, lambda: (value if not value.get("pending") else None)
                if (value := app.request("app.inspect_folder_open")) else None)


def capture(app, filename):
    # Replacement windows can take native focus from the retained parent.
    # Refocus and observe fresh rendered UI before requesting the capture.
    app.input({"type": "move", "position": [4, 4]})
    frame = app.inspect()["render"]["frames"]
    app.wait_until(lambda state: state["windows"][0]["focused"]
                   and state["render"]["frames"] >= frame + 2
                   and state["render"]["pending_pipelines"] == 0
                   and bool(state["ui"]["surfaces"])
                   and all(surface["diagnostics"]["indices"] > 0
                           for surface in state["ui"]["surfaces"]))
    app.request("wait_frames", frames=12)
    result = app.capture(filename)
    assert result["nontransparent_pixels"] > 0, filename
    return result


def quit_app(app, save=False):
    try:
        app.request("invoke", command="bed.application.quit", **{"global": True})
    except AutomationError:
        if app.process.poll() == 0:
            return
        raise
    if save:
        app.wait_until(lambda state: any(text["text"] == "Save All and quit" for text in state["ui"]["texts"]))
        app.click_text("Save All and quit")
    assert app.process.wait(timeout=20) == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/single-session-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = str(Path(args.binary).resolve())
    captures = []
    child = None
    previous = {key: os.environ.get(key) for key in ["HOME", "XDG_CONFIG_HOME", "SHELL"]}
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as directory:
            fixture = Path(directory)
            home, config, project, other = [fixture/name for name in ["home", "config", "project", "other"]]
            for path in [home, project, other]:
                path.mkdir()
            source = project/"notes.txt"
            source.write_text("original\n")
            os.environ.update(HOME=str(home), XDG_CONFIG_HOME=str(config), SHELL="/bin/sh")
            with Application([binary, str(project)], output=output/"parent", timeout=60) as app:
                snapshot = ready(app)
                assert "workspaces" not in snapshot["workbench"]
                app.request("app.set_settings", values={"autosave": "Never"})
                layout = next(row["entity"] for row in snapshot["workbench"]["layouts"] if row["active"])
                opened = app.request("app.open_file", layout=layout, path=str(source))["panel"]
                def preview():
                    return next((p for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == opened and p["editor_rect"] and not p["loading"]), None)
                panel = wait(app, preview)
                primary = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
                app.input({"type":"click", "position":center(panel["editor_rect"], zoom)},
                          {"type":"key", "code":primary, "pressed":True},
                          {"type":"key", "code":"KeyA", "pressed":True},
                          {"type":"key", "code":"KeyA", "pressed":False},
                          {"type":"key", "code":primary, "pressed":False},
                          {"type":"text", "text":"UNSAVED"})
                before = preview()
                assert before["content"] == "UNSAVED" and before["document_info"]["dirty"]
                captures.append(capture(app, "01-unsaved.png"))
                app.request("app.select_layout", slot=2)
                second_layout = next(row["layout"] for row in app.request("app.inspect_layouts")["layouts"] if row["active"])
                second = app.request("app.open_file", layout=second_layout, path=str(source))["panel"]
                shared = wait(app, lambda: next((p for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == second and not p["loading"]), None))
                assert second != opened and shared["document"] == before["document"] and shared["content"] == "UNSAVED"
                app.request("app.select_layout", slot=1)
                assert folder_result(app, project) == {"reused": True}
                launched = folder_result(app, other)
                child = launched["pid"]
                app.wait_until(lambda state: any(text["text"] == "Save All and quit" for text in state["ui"]["texts"]))
                captures.append(capture(app, "02-replacement-quit-prompt.png"))
                app.click_text("Cancel")
                app.wait_until(lambda state: not any(text["text"] == "Quit bEd 2?" for text in state["ui"]["texts"]))
                # The launched automation-build child is a normal GUI instance.
                time.sleep(2)
                os.kill(child, 0)
                after = preview()
                assert after["document"] == before["document"] and after["content"] == "UNSAVED"
                assert app.request("app.inspect_filetree")["project"]["root"] == str(project)
                assert source.read_text() == "original\n"
                captures.append(capture(app, "03-cancel-preserves-parent.png"))
                with Application([binary, str(project)], output=output/"duplicate", timeout=60) as duplicate:
                    state = wait(duplicate, lambda: (v if v["startup_error"] else None) if (v := duplicate.request("app.inspect_filetree")) else None)
                    assert state["project"] is None and "already open" in state["startup_error"]
                    captures.append(capture(duplicate, "04-duplicate-rejected.png"))
                    quit_app(duplicate)
                # A failed replacement still requests the same normal quit.
                rejected = folder_result(app, other)
                assert "already open" in rejected["error"], rejected
                app.click_text("OK")
                app.wait_until(lambda state: any(text["text"] == "Save All and quit" for text in state["ui"]["texts"]))
                app.click_text("Save All and quit")
                assert app.process.wait(timeout=20) == 0
                assert source.read_text() == "UNSAVED"
            with Application([binary, str(project)], output=output/"restored", timeout=60) as app:
                ready(app)
                restored = wait(app, lambda: next((p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source) and not p["loading"]), None))
                assert restored["content"] == "UNSAVED" and not restored["document_info"]["dirty"]
                viewers = wait(app, lambda: (rows if len(rows) == 2 and all(not p["loading"] for p in rows) else None)
                    if (rows := [p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source)]) else None)
                assert len({p["document"] for p in viewers}) == 1, viewers
                captures.append(capture(app, "05-restored.png"))
                quit_app(app)
            os.kill(child, signal.SIGTERM)
            child = None
        assert all(capture["nontransparent_pixels"] > 0 for capture in captures)
        (output/"summary.json").write_text(json.dumps({"captures": captures, "passed": True}, indent=2))
        print(f"single-session native smoke passed: {len(captures)} captures")
    finally:
        if child is not None:
            try:
                os.kill(child, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == "__main__":
    main()
