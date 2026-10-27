#!/usr/bin/env python3
"""Render and interact with Bed/Ducky, then restore their application session."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from automation import Application
from smoke_bed import open_panel, panels, ready, session
from smoke_open import run_helper
from smoke_terminal import bootstrap
from smoke_workspace_policy import clean_quit


def point(snapshot, entity, x=0.5, y=0.5):
    viewport = next(v for v in snapshot["viewports"] if v["entity"] == entity)
    rect = viewport["rect"]
    zoom = snapshot["ui"]["surfaces"][0]["zoom"]
    return [(a + (b - a) * t) * zoom
            for a, b, t in zip(rect["min"], rect["max"], [x, y])]


def scenes(app):
    result = app.request("app.inspect_mascots")["scenes"]
    assert all(s["error"] is None for s in result), result
    return result


def settle(app, entity):
    app.wait_until(lambda s: any(v["entity"] == entity and v["visible"] for v in s["viewports"])
                   and s["render"]["pending_pipelines"] == 0)
    app.request("wait_frames", frames=12)


def capture(app, name, entity=None):
    if entity:
        settle(app, entity)
    else:
        app.request("wait_frames", frames=12)
    result = app.capture(name, **({"viewport": entity} if entity else {}))
    assert result["nontransparent_pixels"] > 1000, (name, result)
    if entity:
        assert result["nontransparent_pixels"] < result["width"] * result["height"], (name, "Lost transparent background")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/mascots-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").unlink(missing_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = {name: os.environ.get(name) for name in ["XDG_CONFIG_HOME", "SHELL"]}
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as temporary:
            fixture = Path(temporary)
            project = fixture / "project"
            project.mkdir()
            os.environ.update(XDG_CONFIG_HOME=str(fixture / "config"), SHELL="/bin/sh")
            with Application([binary, str(project)], output=output / "first", timeout=60) as app:
                app.request("configure", fixed_dt=1 / 60)
                initial = session(ready(app))
                terminal = app.request("app.inspect_terminal")["panels"][0]["entity"]
                bootstrap(app, terminal, "MASCOT_SHELL_READY")
                run_helper(app, terminal, "+ducky extra", fixture, "bad-arguments", expected=1)
                assert scenes(app) == []
                run_helper(app, terminal, "+ducky", fixture, "open-duck")
                duck = next(s["entity"] for s in scenes(app) if s["kind"] == "Duck")
                run_helper(app, terminal, "+ducky", fixture, "reuse-duck")
                assert len(scenes(app)) == 1
                snapshot = app.inspect()
                layout = next(l for l in session(snapshot)["layouts"] if l["active"])
                first_area = layout["areas"][0]["entity"]
                second_area = app.request("area.split", area=first_area, axis="horizontal", fraction=0.5)["area"]
                bed = open_panel(app, "Bed", area=second_area)
                app.request("panel.focus", panel=duck)
                app.request("app.set_settings", values={"theme": "Dark"})
                captures.append(capture(app, "01-bed.png", bed))
                captures.append(capture(app, "02-duck.png", duck))
                assert captures[0]["hash"] != captures[1]["hash"]
                assert all(s["active"] for s in scenes(app))
                # Real pointer tracking changes both the model and bed camera fit.
                app.input({"type": "move", "position": point(app.inspect(), bed, 0.95, 0.15)})
                tilted = capture(app, "03-bed-turned.png", bed)
                captures.append(tilted)
                assert tilted["hash"] != captures[0]["hash"]
                app.input({"type": "click", "position": point(app.inspect(), duck)})
                captures.append(capture(app, "04-duck-click.png", duck))
                # Theme changes update the per-instance bed material and alpha composition.
                app.request("app.set_settings", values={"theme": "Light"})
                captures.append(capture(app, "05-light.png"))
                app.request("window.resize", size=[740, 850])
                captures.append(capture(app, "06-portrait.png"))
                other_bed = open_panel(app, "Bed", area=second_area)
                settle(app, other_bed)
                assert not next(s for s in scenes(app) if s["entity"] == bed)["active"]
                app.request("panel.close", panel=other_bed)
                settle(app, bed)
                # Other Bevy geometry and lights must not leak into either mascot.
                third_area = app.request("area.split", area=first_area, axis="vertical", fraction=0.5)["area"]
                cube = app.request("app.open_file", layout=layout["entity"],
                                   path=str(Path("bedtime/tests/fixtures/gltf/cube.glb").resolve()))["panel"]
                app.request("panel.move", panel=cube, area=third_area)
                captures.append(capture(app, "07-cube-coexistence.png"))
                assert all(s["active"] for s in scenes(app))
                app.request("app.select_layout", slot=2)
                app.request("wait_frames", frames=4)
                assert all(not s["active"] for s in scenes(app))
                app.request("layout.activate", layout=layout["entity"])
                settle(app, bed)
                fresh = app.request("layout.create", name="Other", activate=True)
                assert all(not s["active"] for s in scenes(app))
                app.request("layout.activate", layout=layout["entity"])
                settle(app, bed)
                app.request("layout.delete", layout=fresh["layout"])
                app.request("panel.close", panel=cube)
                captures.append(capture(app, "08-restorable.png"))
                expected = sorted(p["key"] for p in panels(app.inspect()))
                clean_quit(app)
            with Application([binary, str(project)], output=output / "restored", timeout=60) as app:
                ready(app)
                assert sorted(p["key"] for p in panels(app.inspect())) == expected
                restored = scenes(app)
                assert sorted(s["kind"] for s in restored) == ["Bed", "Duck"]
                bed = next(s["entity"] for s in restored if s["kind"] == "Bed")
                captures.append(capture(app, "09-restored.png", bed))
                for scene in restored:
                    app.request("panel.close", panel=scene["entity"])
                assert scenes(app) == []
                assert not app.inspect()["viewports"]
                clean_quit(app)
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
    for log in output.rglob("stderr.log"):
        text = log.read_text()
        assert "Could not despawn entity" not in text, (log, "Duplicate scene cleanup")
        assert "panicked at" not in text and "ERROR" not in text, log
    result = {"captures": captures, "audible_output_verified": False,
              "checked": ["Bed and Ducky", "pointer and click input", "themes", "resize", "independent instances",
                          "hidden tabs and layouts", "cube coexistence", "+ducky", "cleanup", "restoration"]}
    (output / "summary.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
