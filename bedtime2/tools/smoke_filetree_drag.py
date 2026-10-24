#!/usr/bin/env python3
"""Exercise file-to-area drags, edge splits, cancellation and folder transfers."""
import argparse
import json
import os
import tempfile
from pathlib import Path

from automation import Application
from smoke_bed2 import center, prepare_demo_layout, session
from smoke_filetree import capture, inspection, open_native, row_position, wait_tree, wait_value


def areas(app):
    return next(layout for layout in session(app.inspect())["layouts"] if layout["active"])["areas"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/bed2-filetree-drag-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    previous = os.environ.get("XDG_CONFIG_HOME")
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as fixture:
            root = Path(fixture) / "project"
            root.mkdir()
            source = root / "drag.txt"
            source.write_text("FILE_DRAG_SHARED_DOCUMENT\n")
            folder = root / "destination"
            folder.mkdir()
            os.environ["XDG_CONFIG_HOME"] = str(Path(fixture) / "config")
            with Application([str(Path(args.binary).resolve()), str(root)], output=output, timeout=60) as app:
                prepare_demo_layout(app, viewport=False)
                _, layout, tree = open_native(app)
                wait_tree(app, tree, lambda value: str(source) in {row["path"] for row in value["rows"]})
                source_area = next(area for area in areas(app) if any(p["entity"] == tree for p in area["panels"]))["entity"]
                target = app.request("area.split", area=source_area, axis="horizontal")["area"]
                app.request("wait_frames", frames=20)
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]

                def drag_to(position):
                    start = row_position(app, tree, source)
                    before = {p["entity"] for p in inspection(app)["previews"]}
                    app.input({"type": "drag", "from": start, "to": position, "frames": 24})
                    value = wait_value(app, lambda value: any(p["entity"] not in before and p["pinned"]
                        and not p["loading"] and p["content"] == "FILE_DRAG_SHARED_DOCUMENT\n" for p in value["previews"]))
                    return next(p for p in value["previews"] if p["entity"] not in before)

                target_area = next(area for area in areas(app) if area["entity"] == target)
                first = drag_to(center(target_area["content"], zoom))
                assert first["area"] == target and first["layout"] == layout, first
                captures = [capture(app, "01-area-drop.png")]
                # Same file creates another viewer in the requested split, with shared content.
                target_area = next(area for area in areas(app) if area["entity"] == target)
                content = target_area["content"]
                second = drag_to([(content["max"][0] - 8) * zoom, center(content, zoom)[1]])
                assert second["area"] != target, second
                assert second["document"] == first["document"] and len(areas(app)) == 3
                captures.append(capture(app, "02-split-drop.png", different_from=captures[-1]))
                # Escape cancels the pending drag before mouse release.
                before = len(inspection(app)["previews"])
                start = row_position(app, tree, source)
                destination = center(next(a for a in areas(app) if a["entity"] == target)["content"], zoom)
                app.input({"type": "move", "position": start},
                          {"type": "button", "button": "Left", "pressed": True},
                          {"type": "move", "position": [start[0] + 20, start[1]]},
                          {"type": "wait", "frames": 3},
                          {"type": "move", "position": destination},
                          {"type": "wait", "frames": 3},
                          {"type": "key", "code": "Escape", "pressed": True},
                          {"type": "key", "code": "Escape", "pressed": False},
                          {"type": "button", "button": "Left", "pressed": False})
                assert len(inspection(app)["previews"]) == before and source.is_file()
                # A file payload still moves into a tree folder instead of creating a viewer.
                before = len(inspection(app)["previews"])
                start = row_position(app, tree, source)
                end = row_position(app, tree, folder)
                app.input({"type": "drag", "from": start, "to": end, "frames": 24})
                wait_tree(app, tree, lambda value: not value["busy"] and not source.exists()
                          and (folder / source.name).is_file())
                assert len(inspection(app)["previews"]) == before
                captures.append(capture(app, "03-folder-transfer.png", different_from=captures[-1]))
                summary = {"captures": captures, "checks": ["area placement", "edge split", "pinned viewers", "shared document", "Escape cancellation", "folder transfer priority"]}
                (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    finally:
        if previous is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous
    print(f"File drag smoke passed: {output}")


if __name__ == "__main__":
    main()
