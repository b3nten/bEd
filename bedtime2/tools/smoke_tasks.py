#!/usr/bin/env python3
"""Exercise session-owned tasks and processes through the real bed2 UI."""
import argparse
import json
import re
import sys
from pathlib import Path

from automation import Application, AutomationError
from smoke_bed2 import open_panel, prepare_demo_layout


def state(app, panel):
    return next(p for p in app.request("app.inspect_tasks")["panels"]
                if p["entity"] == panel)


def wait_state(app, panel, predicate, *, frames=600):
    for _ in range(frames):
        value = state(app, panel)
        assert value["error"] is None, value
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AutomationError(f"Task panel did not reach expected state: {value}")


def replace_text(app, current, text):
    app.click_text(current)
    modifier = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
    app.input(
        {"type": "key", "code": modifier, "pressed": True},
        {"type": "key", "code": "KeyA", "pressed": True},
        {"type": "key", "code": "KeyA", "pressed": False},
        {"type": "key", "code": modifier, "pressed": False},
        {"type": "text", "text": text},
        {"type": "key", "code": "Enter", "pressed": True},
        {"type": "key", "code": "Enter", "pressed": False},
    )


def send(app, panel, text):
    replace_text(app, state(app, panel)["input"], text)
    assert state(app, panel)["input"] == text


def jobs(app):
    # Session preference I/O is independent of panel-owned tasks.
    return [job for job in app.request("app.inspect_tasks")["jobs"] if job["owner"] is not None]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/tasks-smoke")
    args = parser.parse_args()
    output = Path(args.output)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []

    with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
        prepare_demo_layout(app, viewport=True)
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        snapshot = app.wait_until(lambda s: bool(s["viewports"]))
        layout = next(l["entity"] for l in snapshot["workbench"]["layouts"] if l["active"])
        cube = snapshot["viewports"][0]["entity"]
        app.request("panel.close", panel=cube)

        # Open through the application's titlebar menu, not a construction endpoint.
        open_panel(app, "Tasks & Processes")
        app.wait_until(lambda s: any(t["text"] == "Start task" for t in s["ui"]["texts"]))
        panel = app.request("app.inspect_tasks")["panels"][0]["entity"]
        assert state(app, panel)["task"] is None
        assert state(app, panel)["process"] is None
        captures.append(app.capture("01-idle.png"))

        # Both types of work coexist, own the panel identity, and leave UI responsive.
        app.click_text("Start task")
        app.click_text("Start process")
        value = wait_state(app, panel, lambda p: p["task"]["progress"] > 0
                           and "Ready." in p["process"]["stdout"])
        assert value["process"]["status"] == "Running"
        assert all(j["owner"] == panel for j in jobs(app))
        assert len(jobs(app)) == 2
        app.click_text("Send")
        wait_state(app, panel, lambda p: "echo: Hello from bEd\n" in p["process"]["stdout"])
        send(app, panel, "stderr")
        wait_state(app, panel, lambda p: "A message from stderr." in p["process"]["stderr"])
        captures.append(app.capture("02-working.png"))

        # Nonzero exit is visible independently of successful process supervision.
        send(app, panel, "fail")
        failed = wait_state(app, panel, lambda p: p["process"]["status"] == "Finished")
        assert failed["process"]["exit_code"] == 7
        assert failed["process"]["success"] is False
        assert "Requested failure." in failed["process"]["stderr"]
        captures.append(app.capture("03-exit-seven.png"))

        # Restart gets a fresh process/log, and queued input arrives before EOF.
        old_pid = failed["process"]["pid"]
        app.click_text("Start process")
        wait_state(app, panel, lambda p: "Ready." in p["process"]["stdout"])
        assert state(app, panel)["process"]["pid"] != old_pid
        send(app, panel, "queued before EOF")
        app.click_text("Close stdin")
        ended = wait_state(app, panel, lambda p: p["process"]["status"] == "Finished")
        assert ended["process"]["stdin_closed"]
        assert ended["process"]["exit_code"] == 0
        assert ended["process"]["success"] is True
        assert "echo: queued before EOF\n" in ended["process"]["stdout"]
        assert "stdin closed." in ended["process"]["stdout"]
        assert ended["process"]["stderr"] == ""

        completed = wait_state(app, panel, lambda p: p["task"]["status"] == "Finished"
                               and p["task"]["result"] is not None)
        assert completed["task"]["progress"] == 100
        assert completed["task"]["result"] == 5050
        app.wait_until(lambda s: any(t["text"] == "Result: 5050" for t in s["ui"]["texts"]))
        captures.append(app.capture("04-completed.png"))

        # Cancellation returns no partial result; both controls can restart.
        app.click_text("Start task")
        wait_state(app, panel, lambda p: 0 < p["task"]["progress"] < 100)
        app.click_text("Cancel task")
        cancelled = wait_state(app, panel, lambda p: p["task"]["status"] == "Cancelled")
        assert cancelled["task"]["result"] is None
        assert cancelled["task"]["progress"] < 100
        app.click_text("Start process")
        wait_state(app, panel, lambda p: "Ready." in p["process"]["stdout"])
        app.click_text("Stop process")
        wait_state(app, panel, lambda p: p["process"]["status"] == "Cancelled")
        captures.append(app.capture("05-cancelled.png"))

        # Hidden panels still collect results and drain output. A separately
        # created panel starts fresh without sharing live worker handles.
        replace_text(app, "5.0", "30")
        assert state(app, panel)["seconds"] == 30
        app.click_text("Start task")
        app.click_text("Start process")
        wait_state(app, panel, lambda p: "Ready." in p["process"]["stdout"])
        before = state(app, panel)["task"]["progress"]
        open_panel(app, "Settings")
        wait_state(app, panel, lambda p: p["task"]["progress"] > before)
        fresh = app.request("layout.create", name="Fresh tasks", activate=True)
        fresh_panel = open_panel(app, "Tasks & Processes")
        assert state(app, fresh_panel)["task"] is None and state(app, fresh_panel)["process"] is None
        assert not any(j["owner"] == fresh_panel for j in jobs(app))
        app.request("layout.delete", layout=fresh["layout"])
        app.request("layout.activate", layout=layout)

        # Hidden layouts do not suspend worker threads or panel polling.
        other = app.request("layout.create", name="Other", activate=True)
        before = state(app, panel)["task"]["progress"]
        wait_state(app, panel, lambda p: p["task"]["progress"] > before)
        app.request("panel.close", panel=panel)
        for _ in range(120):
            if not jobs(app):
                break
            app.request("wait_frames", frames=2)
        assert not jobs(app), "Closing the inactive panel left owned work alive"

        # Retiring the layout waits for both workers without stalling UI.
        app.request("layout.activate", layout=layout)
        panel = open_panel(app, "Tasks & Processes")
        app.click_text("Start task")
        app.click_text("Start process")
        wait_state(app, panel, lambda p: p["task"]["progress"] > 0
                   and "Ready." in p["process"]["stdout"])
        app.request("layout.delete", layout=layout)
        app.request("layout.activate", layout=other["layout"])
        snapshot = app.inspect()
        assert all(l["entity"] != layout for l in snapshot["workbench"]["layouts"])
        assert app.request("app.inspect_tasks")["panels"] == []

        open_panel(app, "Tasks & Processes")
        captures.append(app.capture("06-rebuilt.png"))
        assert all(s["diagnostics"]["error"] is None for s in app.inspect()["ui"]["surfaces"])

    log = re.sub(r"\x1b\[[0-9;]*m", "", (output / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"bed2 task/process smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
