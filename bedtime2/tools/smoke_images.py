#!/usr/bin/env python3
"""Native image canvas, Open With, live SVG, source lifecycle and restore smoke."""
import argparse
import json
import math
import os
import tempfile
from pathlib import Path

from automation import Application
from smoke_bed2 import center, prepare_demo_layout, ready
from smoke_editor import primary
from smoke_filetree import capture, click_row, inspection, open_native, wait_tree, wait_value
from smoke_titlebar import rgba_rows


def preview(app, path, viewer="image"):
    state = wait_value(app, lambda state: any(p["path"] == str(path) and p["viewer"] == viewer
        and not p["loading"] and p["error"] is None for p in state["previews"]))
    return next(p for p in state["previews"] if p["path"] == str(path) and p["viewer"] == viewer)


def current(app, panel):
    return next(p for p in inspection(app)["previews"] if p["entity"] == panel)


def choose(app, label):
    app.wait_until(lambda state: any(t["text"] == label for t in state["ui"]["texts"]))
    app.click_text(label)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/images-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = os.environ.get("XDG_CONFIG_HOME")
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as fixture:
            root = Path(fixture) / "project"
            root.mkdir()
            os.environ["XDG_CONFIG_HOME"] = str(Path(fixture) / "config")
            svg = root / "drawing.svg"
            original = '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120"><rect width="160" height="120" fill="red" fill-opacity="0.5"/><circle cx="120" cy="60" r="30" fill="blue"/></svg>'
            svg.write_text(original)
            raster = root / "sample.PNG"
            raster.write_bytes(Path("bedtime/tests/fixtures/images/sample.png").read_bytes())
            with Application([binary, str(root)], output=output / "first", timeout=90) as app:
                prepare_demo_layout(app, viewport=False)
                _, tree = open_native(app)
                wait_tree(app, tree, lambda state: str(svg) in {r["path"] for r in state["rows"]})
                click_row(app, tree, raster)
                raster_view = preview(app, raster)
                assert raster_view["image"]["dimensions"] and raster_view["document"] is not None
                captures.append(capture(app, "01-raster.png"))
                click_row(app, tree, svg)
                image = preview(app, svg)
                assert image["entity"] == raster_view["entity"], "Viewer preview must be reused"
                app.request("panel.focus", panel=image["entity"])
                app.request("wait_frames", frames=10)
                image = current(app, image["entity"])
                assert image["image"]["dimensions"] == [240, 120]
                rect = image["image"]["rect"]
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
                point = [(rect["min"][0] * 0.7 + rect["max"][0] * 0.3) * zoom,
                         (rect["min"][1] * 0.6 + rect["max"][1] * 0.4) * zoom]
                captures.append(capture(app, "02-svg-contained.png"))
                app.input({"type": "move", "position": point}, {"type": "scroll", "delta": [0, 180], "pixels": True})
                wait_value(app, lambda state: next(p for p in state["previews"] if p["entity"] == image["entity"])["image"]["zoom"] > 1.1)
                app.request("wait_frames", frames=40)
                after_zoom = current(app, image["entity"])["image"]
                # Image-space point under the cursor must be fixed.
                viewport = [rect["max"][i] - rect["min"][i] for i in range(2)]
                fit = min(viewport[0] / 240, viewport[1] / 120)
                for axis, dimension in enumerate([240, 120]):
                    offset = point[axis] / zoom - (rect["min"][axis] + rect["max"][axis]) / 2
                    before = 0.5 + offset / (dimension * fit)
                    after = after_zoom["center"][axis] + offset / (dimension * fit * after_zoom["zoom"])
                    assert math.isclose(before, after, abs_tol=0.003), (before, after)
                captures.append(capture(app, "03-cursor-zoom.png", different_from=captures[-1]))
                start = center(rect, zoom)
                app.input({"type": "drag", "from": start, "to": [start[0]+60, start[1]+35], "frames": 16})
                panned = current(app, image["entity"])["image"]
                assert panned["center"] != after_zoom["center"]
                captures.append(capture(app, "04-pan.png", different_from=captures[-1]))
                # Canvas menu opens another pinned viewer, preserving this preview.
                app.input({"type": "click", "position": start, "button": "Right"})
                choose(app, "Open With")
                choose(app, "Code")
                code = preview(app, svg, "code")
                assert code["document"] == image["document"] and code["pinned"]
                assert not current(app, image["entity"])["pinned"]
                # The Files menu must reuse the existing Image tab and pin it.
                click_row(app, tree, svg, button="Right")
                choose(app, "Open With")
                choose(app, "Image (default)")
                assert current(app, image["entity"])["pinned"]
                # Show both canonical views while editing through the real text canvas.
                area = app.request("area.split", area=image["area"], axis="vertical")["area"]
                app.request("panel.move", panel=code["entity"], area=area)
                app.request("panel.focus", panel=code["entity"])
                app.request("wait_frames", frames=10)
                code = current(app, code["entity"])
                app.input({"type": "click", "position": center(code["editor_rect"], zoom)})
                primary(app, "KeyA")
                changed = '<svg xmlns="http://www.w3.org/2000/svg" width="300" height="150"><rect width="300" height="150" fill="#00ff00"/></svg>'
                app.input({"type": "text", "text": changed})
                wait_value(app, lambda state: next(p for p in state["previews"] if p["entity"] == image["entity"])["image"]["dimensions"] == [300,150])
                app.request("wait_frames", frames=20)
                assert svg.read_text() == original, "Preview must update before saving"
                captures.append(capture(app, "05-unsaved-svg.png"))
                pixels = b"".join(rgba_rows(captures[-1]["path"]))
                assert sum(1 for i in range(0, len(pixels), 4) if pixels[i+1] > 180 and pixels[i] < 80 and pixels[i+2] < 80) > 1000, "Green SVG pixels were not rendered"
                primary(app, "KeyS")
                wait_value(app, lambda state: not next(p for p in state["previews"] if p["entity"] == code["entity"])["document_info"]["dirty"])
                assert svg.read_text() == changed
            with Application([binary, str(root)], output=output / "restored", timeout=90) as app:
                ready(app)
                restored_image = preview(app, svg)
                restored_code = preview(app, svg, "code")
                assert restored_image["document"] == restored_code["document"]
                assert restored_image["image"]["zoom"] == 1
                captures.append(capture(app, "06-restored.png"))
                # A confirmed tree rename rebinds both viewers; deletion retires pixels.
                _, tree = open_native(app)
                click_row(app, tree, svg, button="Right")
                choose(app, "Rename…")
                from smoke_filetree import edit_name
                edit_name(app, "renamed.svg")
                renamed = root / "renamed.svg"
                wait_value(app, lambda state: len([p for p in state["previews"] if p["path"] == str(renamed)]) == 2)
                renamed.unlink()
                wait_value(app, lambda state: all(p["path"] != str(renamed) for p in state["previews"]))
                captures.append(capture(app, "07-deleted.png"))
            (output / "summary.json").write_text(json.dumps({"captures": captures, "checks": ["raster", "SVG alpha", "cursor zoom", "pan", "canvas and tree Open With", "canonical live unsaved SVG", "save", "viewer restoration", "rename and deletion"]}, indent=2) + "\n")
    finally:
        if previous is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous
    print(f"Image smoke passed: {output}")


if __name__ == "__main__":
    main()
