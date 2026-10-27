#!/usr/bin/env python3
"""Native read-only Hex fallback, paging, search, clipboard and restore smoke."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from automation import Application
from smoke_bed import ready, session, center, prepare_demo_layout
from smoke_filetree import key, primary, capture, wait_value
from smoke_workspace_policy import clean_quit, wait_for


def state(app, panel):
    return next(p for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == panel)


def open_file(app, path, viewer=None):
    layout = next(l["entity"] for l in session(app.inspect())["layouts"] if l["active"])
    args = {"layout": layout, "path": str(path)}
    if viewer:
        args["viewer"] = viewer
    panel = app.request("app.open_file", **args)["panel"]
    wait_for(app, lambda: not state(app, panel)["loading"], "File load did not finish")
    assert state(app, panel)["error"] is None, state(app, panel)
    return panel


def goto(app, panel, offset):
    app.request("panel.focus", panel=panel)
    app.click_text("Go to…")
    app.request("wait_frames", frames=2)
    primary(app, "KeyA")
    app.input({"type": "text", "text": hex(offset)})
    app.click_text("Go")
    wait_for(app, lambda: state(app, panel)["hex"]["cursor"] == offset, "Go to offset was not exact")
    app.click_text("Close")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/hex-smoke")
    parser.add_argument("--functional-only", action="store_true",
                        help="Validate interactions without claiming screenshot rendering coverage")
    args = parser.parse_args()
    def take_capture(app, filename):
        return capture(app, filename, validate=not args.functional_only)
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").unlink(missing_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = {key: os.environ.get(key) for key in ["XDG_CONFIG_HOME", "HOME", "SHELL"]}
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as fixture:
            root = Path(fixture)
            project, home, config = root / "project", root / "home", root / "config"
            project.mkdir(); home.mkdir()
            data = project / "bytes.bin"
            content = bytearray(bytes(range(256)) * 4096)
            crossing = 65534
            content[crossing:crossing+4] = bytes.fromhex("DE AD BE EF")
            data.write_bytes(content)
            huge = project / "huge.bin"
            offset = 2**32 + 37
            with huge.open("wb") as file:
                file.truncate(offset+64)
                file.seek(offset)
                file.write(bytes.fromhex("12 34 56 78"))
            empty = project / "empty.bin"; empty.touch()
            wrong = project / "wrong.png"; wrong.write_text("plain text with an image extension")
            os.environ.update(XDG_CONFIG_HOME=str(config), HOME=str(home), SHELL="/bin/sh")
            with Application([binary, str(project)], output=output / "first", timeout=60) as app:
                prepare_demo_layout(app, viewport=False)
                panel = open_file(app, data)
                wait_for(app, lambda: state(app, panel)["hex"] and state(app, panel)["hex"]["rendered_rows"] > 0, "Hex did not draw")
                value = state(app, panel)
                assert value["viewer"] == "hex" and value["document"] is not None
                assert value["hex"]["cached_bytes"] <= 2*1024*1024
                assert value["hex"]["rendered_rows"] < 100
                captures.append(take_capture(app, "01-fallback.png"))
                app.click_text("Find…")
                app.request("wait_frames", frames=2)
                app.input({"type": "text", "text": "DE AD BE EF"})
                app.click_text("Next")
                wait_for(app, lambda: state(app, panel)["hex"]["selection"] == [crossing, crossing+4], "Search did not cross the page boundary")
                captures.append(take_capture(app, "02-search.png"))
                app.click_text("Close")
                value = state(app, panel)
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
                # Focus without changing the selected range, then use native Copy.
                rect = value["hex"]["rect"]
                app.input({"type": "click", "position": [rect["min"][0]*zoom+2, rect["min"][1]*zoom+2]})
                primary(app, "KeyC")
                app.request("wait_frames", frames=8)
                app.click_text("Find…")
                app.request("wait_frames", frames=2)
                primary(app, "KeyA")
                primary(app, "KeyV")
                wait_for(app, lambda: state(app, panel)["hex"]["query"] == "DE AD BE EF", "Copy did not use the selected bytes")
                app.click_text("Close")
                large = open_file(app, huge)
                wait_for(app, lambda: state(app, large)["hex"] is not None, "Large Hex source unavailable")
                goto(app, large, offset)
                wait_for(app, lambda: state(app, large)["hex"]["cached_pages"] >= 2, "Distant page did not load")
                assert state(app, large)["hex"]["len"] == offset+64
                assert state(app, large)["hex"]["cached_bytes"] <= 2*1024*1024
                captures.append(take_capture(app, "03-large-offset.png"))
                app.request("app.set_settings", values={"font_size": 22.0})
                app.request("wait_frames", frames=6)
                captures.append(take_capture(app, "04-app-font-size.png"))
                app.request("window.resize", size=[520, 720])
                app.request("wait_frames", frames=6)
                captures.append(take_capture(app, "05-narrow.png"))
                app.request("window.resize", size=[1280, 900])
                empty_panel = open_file(app, empty, "hex")
                wait_for(app, lambda: state(app, empty_panel)["hex"] is not None, "Empty Hex source unavailable")
                assert state(app, empty_panel)["hex"]["len"] == 0
                captures.append(take_capture(app, "06-empty.png"))
                text = open_file(app, wrong)
                assert state(app, text)["viewer"] == "code"
                app.request("app.set_settings", values={"autosave": "Never"})
                app.request("panel.focus", panel=text)
                app.request("wait_frames", frames=3)
                app.input({"type": "click", "position": center(state(app, text)["editor_rect"])})
                primary(app, "KeyA")
                unsaved = "unsaved byte projection"
                app.input({"type": "text", "text": unsaved})
                wait_for(app, lambda: state(app, text)["content"] == unsaved, "Text edit did not reach the canonical document")
                explicit = open_file(app, wrong, "hex")
                assert state(app, explicit)["viewer_explicit"]
                assert state(app, explicit)["document"] == state(app, text)["document"]
                assert state(app, explicit)["hex"]["len"] == len(unsaved.encode())
                assert wrong.read_text() != unsaved
                app.click_text("Find…")
                app.request("wait_frames", frames=2)
                app.input({"type": "text", "text": unsaved.encode().hex(" ")})
                app.click_text("Next")
                wait_for(app, lambda: state(app, explicit)["hex"]["selection"] == [0, len(unsaved.encode())], "Hex search did not use unsaved document bytes")
                app.click_text("Close")
                captures.append(take_capture(app, "07-explicit-hex.png"))
                app.request("panel.focus", panel=text)
                app.request("wait_frames", frames=2)
                app.input({"type": "click", "position": center(state(app, text)["editor_rect"])})
                primary(app, "KeyZ")
                wait_for(app, lambda: not state(app, text)["document_info"]["dirty"], "Undo did not return the shared document to its disk baseline")
                clean_quit(app)
            with Application([binary, str(project)], output=output / "restore", timeout=60) as app:
                ready(app)
                wait_for(app, lambda: any(p["path"] == str(wrong) and p["viewer"] == "hex" and p["viewer_explicit"] for p in app.request("app.inspect_filetree")["previews"]), "Explicit Hex did not restore")
                app.request("wait_frames", frames=8)
                captures.append(take_capture(app, "08-restored.png"))
                clean_quit(app)
        (output / "summary.json").write_text(json.dumps({"captures": captures, "capture_validation": not args.functional_only}, indent=2))
        scope = "functional smoke (pixels not validated)" if args.functional_only else "smoke"
        print(f"Hex {scope}: {len(captures)} captures in {output}")
    finally:
        for key, value in previous.items():
            if value is None: os.environ.pop(key, None)
            else: os.environ[key] = value


if __name__ == "__main__":
    main()
