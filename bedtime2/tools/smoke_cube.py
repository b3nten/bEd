#!/usr/bin/env python3
"""Validate rendered cube viewports and orbit input through the real app."""
import argparse
import json
import math
import re
from pathlib import Path

from automation import Application
from smoke_bed2 import open_panel, prepare_demo_layout


def orbit(app, entity):
    return next(v for v in app.request("app.inspect_cube")["viewports"]
                if v["entity"] == entity)


def view(snapshot, entity):
    return next(v for v in snapshot["viewports"] if v["entity"] == entity)


def point(snapshot, entity, x=0.5, y=0.5):
    rect = view(snapshot, entity)["rect"]
    zoom = snapshot["ui"]["surfaces"][0]["zoom"]
    return [(a + (b - a) * t) * zoom
            for a, b, t in zip(rect["min"], rect["max"], [x, y])]


def stable(a, b):
    return all(a[key] == b[key] for key in
               ("target", "yaw", "pitch", "distance", "translation", "rotation"))


def assert_camera(camera):
    assert all(math.isfinite(v) for field in ("target", "translation", "rotation")
               for v in camera[field]), camera
    distance = math.dist(camera["translation"], camera["target"])
    assert math.isclose(distance, camera["distance"], rel_tol=1e-5), camera
    assert -math.pi / 2 < camera["pitch"] < math.pi / 2, camera


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/cube-smoke")
    args = parser.parse_args()
    output = Path(args.output)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []
    with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
        prepare_demo_layout(app, viewport=True)
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        snapshot = app.wait_until(lambda s: len(s["viewports"]) == 1
                                  and s["viewports"][0]["visible"]
                                  and s["render"]["pending_pipelines"] == 0)
        entity = snapshot["viewports"][0]["entity"]
        workspace = snapshot["workbench"]
        area = workspace["layouts"][0]["areas"][0]["entity"]
        initial = orbit(app, entity)
        assert initial["active"]
        assert_camera(initial)
        app.request("wait_frames", frames=12)
        captures.append(app.capture("01-cube.png"))
        captures.append(app.capture("02-target.png", viewport=entity))
        physical = view(app.inspect(), entity)["physical_size"]
        assert [captures[-1]["width"], captures[-1]["height"]] == physical
        assert captures[-1]["nontransparent_pixels"] == physical[0] * physical[1]

        # Drag through egui; both the orbit state and rendered pixels must move.
        snapshot = app.inspect()
        start = point(snapshot, entity)
        app.input({"type": "drag", "from": start,
                   "to": [start[0] + 110, start[1] + 40], "frames": 12})
        rotated = orbit(app, entity)
        assert rotated["yaw"] != initial["yaw"] and rotated["pitch"] != initial["pitch"]
        assert rotated["target"] == initial["target"]
        assert_camera(rotated)
        captures.append(app.capture("03-orbit.png", viewport=entity))
        assert captures[-1]["hash"] != captures[-2]["hash"]

        # Shift + primary and middle button both pan, with orientation preserved.
        start = point(app.inspect(), entity)
        app.input(
            {"type": "key", "code": "ShiftLeft", "pressed": True},
            {"type": "drag", "from": start,
             "to": [start[0] + 50, start[1] - 20], "frames": 8},
            {"type": "key", "code": "ShiftLeft", "pressed": False},
        )
        shifted = orbit(app, entity)
        assert shifted["target"] != rotated["target"]
        assert shifted["yaw"] == rotated["yaw"] and shifted["pitch"] == rotated["pitch"]
        app.input(
            {"type": "move", "position": start},
            {"type": "button", "button": "Middle", "pressed": True},
            {"type": "move", "position": [start[0] - 40, start[1] + 30]},
            {"type": "button", "button": "Middle", "pressed": False},
        )
        panned = orbit(app, entity)
        assert panned["target"] != shifted["target"]
        assert panned["yaw"] == shifted["yaw"] and panned["pitch"] == shifted["pitch"]
        app.input({"type": "move", "position": start},
                  {"type": "scroll", "delta": [0, 3]},
                  {"type": "wait", "frames": 40})
        zoomed = orbit(app, entity)
        assert zoomed["distance"] < panned["distance"]
        assert_camera(zoomed)
        captures.append(app.capture("04-pan-zoom.png"))
        app.click_text("Reset view")
        assert stable(orbit(app, entity), initial)

        # Pitch/zoom limits remain finite even with extreme input.
        start = point(app.inspect(), entity)
        app.input({"type": "drag", "from": start,
                   "to": [start[0], start[1] + 2000], "frames": 4})
        assert_camera(orbit(app, entity))
        app.click_text("Reset view")
        app.input({"type": "move", "position": start},
                  {"type": "scroll", "delta": [0, 100000], "pixels": True},
                  {"type": "wait", "frames": 120})
        assert orbit(app, entity)["distance"] >= 0.25
        assert_camera(orbit(app, entity))
        app.click_text("Reset view")

        # Two independent cameras share one scene and renderer.
        split = app.request("area.split", area=area, axis="horizontal", fraction=0.5)
        other = open_panel(app, "3D Viewport", area=split["area"])
        snapshot = app.wait_until(lambda s: len(s["viewports"]) == 2
                                  and all(v["visible"] for v in s["viewports"]))
        before = orbit(app, other)
        assert orbit(app, entity)["image"] != before["image"]
        start = point(snapshot, entity)
        app.input({"type": "drag", "from": start,
                   "to": [start[0] + 45, start[1] + 15], "frames": 8})
        assert stable(orbit(app, other), before)
        assert not stable(orbit(app, entity), initial)
        captures.append(app.capture("05-two-viewports.png"))

        # A drag started in the left viewport continues across the divider,
        # without transferring to the right camera. It ends on release.
        before_left = orbit(app, entity)
        start = point(app.inspect(), entity, 0.85, 0.6)
        end = point(app.inspect(), other, 0.4, 0.6)
        app.input({"type": "drag", "from": start, "to": end, "frames": 12})
        assert not stable(orbit(app, entity), before_left)
        assert stable(orbit(app, other), before)
        stopped = orbit(app, entity)
        app.input({"type": "move", "position": [end[0] + 20, end[1] + 20]})
        assert stable(orbit(app, entity), stopped)

        # A wheel event outside both image rectangles cannot zoom either view.
        snapshot = app.inspect()
        toolbar = [point(snapshot, entity)[0],
                   (view(snapshot, entity)["rect"]["min"][1] - 5)
                   * snapshot["ui"]["surfaces"][0]["zoom"]]
        app.input({"type": "move", "position": toolbar},
                  {"type": "scroll", "delta": [0, -3]},
                  {"type": "wait", "frames": 40})
        assert stable(orbit(app, entity), stopped)
        assert stable(orbit(app, other), before)

        # Window resize updates each image and its projection together.
        images = {v["entity"]: v["image"]
                  for v in app.request("app.inspect_cube")["viewports"]}
        app.request("window.resize", size=[1100, 720])
        snapshot = app.wait_until(lambda s: s["windows"][0]["size"] == [1100, 720])
        # Layouts animate while resizing. Sample the projection and target size
        # together rather than compare observations from different frames.
        for camera in app.request("app.inspect_cube")["viewports"]:
            assert camera["image"] == images[camera["entity"]]
            assert camera["target_size"] == camera["physical_size"]
            assert math.isclose(camera["aspect_ratio"],
                                camera["physical_size"][0] / camera["physical_size"][1],
                                rel_tol=1e-5)
        captures.append(app.capture("06-resized.png"))

        # Hidden tabs and hidden layouts suspend rendering without losing
        # orbit state. A newly created layout gets fresh concrete view state.
        open_panel(app, "Settings", area=area)
        assert not orbit(app, entity)["active"]
        app.input({"type": "drag", "from": point(app.inspect(), entity),
                   "to": point(app.inspect(), entity, 0.7, 0.7), "frames": 8})
        assert stable(orbit(app, entity), stopped)
        app.request("panel.focus", panel=entity)
        assert orbit(app, entity)["active"]
        fresh = app.request("layout.create", name="Fresh viewports", activate=True)
        fresh_first = open_panel(app, "3D Viewport")
        fresh_split = app.request("area.split", area=next(l["areas"][0]["entity"] for l in app.inspect()["workbench"]["layouts"] if l["entity"] == fresh["layout"]), axis="horizontal", fraction=0.5)
        fresh_second = open_panel(app, "3D Viewport", area=fresh_split["area"])
        app.wait_until(lambda s: len(s["viewports"]) == 4
                       and all(v["visible"] for v in s["viewports"] if v["entity"] in (fresh_first, fresh_second)))
        states = app.request("app.inspect_cube")["viewports"]
        assert len(states) == 4
        assert all(not v["active"] for v in states if v["entity"] in images)
        assert all(stable(v, initial) for v in states if v["entity"] not in images)
        captures.append(app.capture("07-fresh.png"))
        app.request("layout.activate", layout=workspace["layouts"][0]["entity"])
        assert stable(orbit(app, entity), stopped)

        # Detach/reattach UI, close a viewport, then retire complete layouts.
        app.request("surface.detach")
        assert all(not v["active"] for v in app.request("app.inspect_cube")["viewports"])
        app.request("surface.attach")
        app.wait_until(lambda s: any(v["visible"] for v in s["viewports"]))
        assert orbit(app, entity)["active"]
        app.request("panel.close", panel=other)
        assert all(v["entity"] != other for v in app.request("app.inspect_cube")["viewports"])
        app.request("layout.delete", layout=workspace["layouts"][0]["entity"])
        app.request("layout.delete", layout=fresh["layout"])
        assert app.request("app.inspect_cube")["viewports"] == []
        app.request("layout.create", name="Fresh", activate=True)
        open_panel(app, "3D Viewport")
        app.wait_until(lambda s: len(s["viewports"]) == 1 and s["viewports"][0]["visible"])
        captures.append(app.capture("08-rebuilt.png"))
        snapshot = app.inspect(entities=True)
        assert sum(e["name"] == "Cube" for e in snapshot["entities"]) == 1
        assert all(s["diagnostics"]["error"] is None for s in snapshot["ui"]["surfaces"])

    log = re.sub(r"\x1b\[[0-9;]*m", "", (output / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"Cube smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
