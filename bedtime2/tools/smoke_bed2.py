#!/usr/bin/env python3
"""Exercise the real bed2 executable exclusively through automation."""
import argparse
import json
import re
import sys
from pathlib import Path

from automation import Application


def workspace(snapshot):
    return next(w for w in snapshot["workbench"]["workspaces"] if w["active"])


def panels(snapshot):
    return [p for w in snapshot["workbench"]["workspaces"]
            for layout in w["layouts"] for area in layout["areas"] for p in area["panels"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/automation2-smoke")
    args = parser.parse_args()
    (Path(args.output) / "summary.json").unlink(missing_ok=True)
    captures = []
    with Application([str(Path(args.binary).resolve())], output=args.output) as app:
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        # The application now starts with its 3D viewport. This scenario keeps
        # exercising the smaller Hello/Theme layout; smoke_cube covers 3D.
        snapshot = app.wait_until(lambda s: bool(s["viewports"]))
        assert [p["key"] for p in panels(snapshot)] == ["bed.cube"]
        app.request("panel.close", panel=panels(snapshot)[0]["entity"])
        app.request("panel.open", key="bed.hello")
        snapshot = app.wait_until(lambda s: any(t["text"] == "Hello, world!" for t in s["ui"]["texts"]))
        initial = workspace(snapshot)
        assert len(snapshot["workbench"]["workspaces"]) == 1
        assert len(initial["layouts"]) == 1
        assert [p["key"] for p in panels(snapshot)] == ["bed.hello"]
        surface = snapshot["workbench"]["surfaces"][0]
        status = surface["presentation"]["status_bar"]
        assert status["max"][1] == status["min"][1], status
        captures.append(app.capture("01-hello.png"))
        assert captures[-1]["nontransparent_pixels"] > 100

        # Open Theme using the actual titlebar button and normal input routing.
        bounds = surface["presentation"]["titlebar_content"]
        zoom = snapshot["ui"]["surfaces"][0]["zoom"]
        app.input({"type": "click", "position": [
            (bounds["max"][0] - 12) * zoom,
            (bounds["min"][1] + bounds["max"][1]) / 2 * zoom,
        ]})
        snapshot = app.wait_until(lambda s: any(p["key"] == "bed.theme" and p["visible"] for p in panels(s)))
        theme_panel = next(p["entity"] for p in panels(snapshot) if p["key"] == "bed.theme")
        captures.append(app.capture("02-theme-dark.png"))
        dark = snapshot["workbench"]["surfaces"][0]["theme"]
        app.click_text("Light preset")
        snapshot = app.inspect()
        light = snapshot["workbench"]["surfaces"][0]["theme"]
        assert dark["palette"]["background"] != light["palette"]["background"]
        captures.append(app.capture("03-theme-light.png"))
        assert captures[-1]["hash"] != captures[-2]["hash"]

        # Type into a real egui numeric editor, including a platform shortcut.
        app.click_text("7.0")
        modifier = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
        app.input(
            {"type": "key", "code": modifier, "pressed": True},
            {"type": "key", "code": "KeyA", "pressed": True},
            {"type": "key", "code": "KeyA", "pressed": False},
            {"type": "key", "code": modifier, "pressed": False},
            {"type": "text", "text": "11.5"},
            {"type": "key", "code": "Enter", "pressed": True},
            {"type": "key", "code": "Enter", "pressed": False},
        )
        assert app.inspect()["workbench"]["surfaces"][0]["settings"]["rounding"] == 11.5
        app.request("surface.configure", settings={"rounding": 7})

        # Configuration patches validate before mutation and apply through the
        # same public components used by the theme panel.
        app.request("theme.set", preset="dark", theme={"palette": {"accent": [0.2, 0.9, 0.5, 1]}})
        app.request("surface.configure", settings={"drag_corners": False, "tab_bar": "hidden", "panel_gap": 12, "single_area_edge_to_edge": False})
        snapshot = app.inspect()
        assert not snapshot["workbench"]["surfaces"][0]["settings"]["drag_corners"]
        assert snapshot["workbench"]["surfaces"][0]["settings"]["tab_bar"] == "hidden"
        captures.append(app.capture("04-configured.png", panel=theme_panel))
        app.request("surface.configure", settings={"drag_corners": True, "tab_bar": "always", "single_area_edge_to_edge": True, "panel_gap": 5})

        # Split, move an existing tab, and verify actual geometry/rendering.
        area = initial["layouts"][0]["areas"][0]["entity"]
        split = app.request("area.split", area=area, axis="horizontal", fraction=0.5)
        app.request("panel.move", panel=theme_panel, area=split["area"])
        snapshot = app.inspect()
        assert len(workspace(snapshot)["layouts"][0]["areas"]) == 2
        assert all(p["visible"] for p in panels(snapshot))
        areas = workspace(snapshot)["layouts"][0]["areas"]
        left, right = sorted(areas, key=lambda a: a["rect"]["min"][0])
        divider = (left["surface"]["max"][0] + right["surface"]["min"][0]) / 2
        y = (left["surface"]["min"][1] + left["surface"]["max"][1]) / 2
        app.input({"type": "drag", "from": [divider * zoom, y * zoom],
                   "to": [(divider + 75) * zoom, y * zoom], "frames": 12})
        snapshot = app.inspect()
        changed = workspace(snapshot)["layouts"][0]["areas"]
        assert next(a for a in changed if a["entity"] == left["entity"])["rect"] != left["rect"]
        text_before = {t["text"] for t in snapshot["ui"]["texts"]}
        p = next(p for p in panels(snapshot) if p["entity"] == theme_panel)
        app.input({"type": "move", "position": [(p["rect"]["max"][0] - 20) * zoom, (p["rect"]["min"][1] + 100) * zoom]},
                  {"type": "scroll", "delta": [0, -12]})
        text_after = {t["text"] for t in app.inspect()["ui"]["texts"]}
        assert text_before != text_after
        app.input({"type": "scroll", "delta": [0, 100]})
        captures.append(app.capture("05-split.png"))

        # Serialize a populated workspace, instantiate it with fresh identities,
        # then compare the entity-free representation exactly.
        saved = app.request("workspace.capture")
        restored = app.request("workspace.restore", snapshot=saved, activate=True)
        assert app.request("workspace.capture") == saved
        snapshot = app.inspect()
        assert workspace(snapshot)["entity"] == restored["workspace"]
        for w in snapshot["workbench"]["workspaces"]:
            for layout in w["layouts"]:
                for a in layout["areas"]:
                    for p in a["panels"]:
                        assert p["active_workspace"] == w["active"]

        # Exercise the lifecycle fixes through a running renderer.
        camera = snapshot["ui"]["surfaces"][0]["camera"]
        app.request("surface.detach")
        snapshot = app.inspect()
        assert snapshot["workbench"]["surfaces"] == []
        assert snapshot["ui"]["surfaces"] == []
        app.request("entity.inspect", entity=camera)
        app.request("surface.attach")
        snapshot = app.wait_until(lambda s: bool(s["ui"]["texts"]))
        assert snapshot["ui"]["surfaces"][0]["camera"] == camera
        captures.append(app.capture("06-reattached.png"))

        app.request("window.resize", size=[1100, 720])
        snapshot = app.wait_until(lambda s: s["windows"][0]["size"] == [1100, 720])
        captures.append(app.capture("07-resized.png"))
        assert captures[-1]["width"] == snapshot["windows"][0]["physical_size"][0]

        app.request("workspace.close", workspace=initial["entity"])
        app.request("workspace.close", workspace=restored["workspace"])
        snapshot = app.inspect()
        assert snapshot["workbench"]["workspaces"] == []
        captures.append(app.capture("08-empty.png"))
        created = app.request("workspace.create", name="Rebuilt", activate=True)
        app.request("panel.open", key="bed.hello")
        snapshot = app.inspect()
        assert workspace(snapshot)["entity"] == created["workspace"]
        assert any(t["text"] == "Hello, world!" for t in snapshot["ui"]["texts"])
        captures.append(app.capture("09-rebuilt.png"))
        assert all(s["diagnostics"]["error"] is None for s in snapshot["ui"]["surfaces"])

    log = re.sub(r"\x1b\[[0-9;]*m", "", (Path(args.output) / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    summary = {"passed": True, "captures": captures}
    (Path(args.output) / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"bed2 automation smoke passed: {len(captures)} captures in {args.output}")


if __name__ == "__main__":
    main()
