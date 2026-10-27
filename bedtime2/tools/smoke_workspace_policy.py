#!/usr/bin/env python3
"""Exercise centralized RON storage, temporary startup, real autosave and layouts."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from automation import Application, AutomationError
from smoke_bed2 import ready, session, panels, open_panel, toolbar, center


def clean_quit(app):
    try:
        app.request("invoke", command="bed.application.quit", **{"global": True})
    except AutomationError:
        if app.process.poll() not in (None, 0):
            raise
    try:
        assert app.process.wait(timeout=12) == 0
    except subprocess.TimeoutExpired:
        # Preserve the actual close gate instead of reporting only a process timeout.
        (app.output / "quit-timeout.json").write_text(json.dumps({
            "snapshot": app.inspect(),
            "files": app.request("app.inspect_filetree"),
        }, indent=2))
        app.capture("quit-timeout.png")
        raise


def wait_for(app, predicate, label):
    deadline = time.monotonic()+12
    while time.monotonic() < deadline:
        if predicate():
            return
        app.request("wait_frames", frames=2)
    diagnostic = {"files":app.request("app.inspect_filetree"), "settings":app.request("app.inspect_settings"), "snapshot":app.inspect()}
    (app.output/"failure.json").write_text(json.dumps(diagnostic,indent=2))
    raise AssertionError(label + "; see " + str(app.output/"failure.json"))


def settled_storage(app):
    entries = app.request("app.inspect_filetree")["storage"]
    return next((entry for entry in entries if entry["loaded"]
                 and not entry["pending"] and entry["error"] is None), None)


def edit(app, source, content):
    preview = next(p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source))
    app.request("panel.focus", panel=preview["entity"])
    wait_for(app, lambda: any(p["path"] == str(source) and p["editor_rect"] for p in app.request("app.inspect_filetree")["previews"]), "Editor did not draw")
    preview = next(p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source))
    zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
    primary = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
    steps = [{"type":"click", "position":center(preview["editor_rect"], zoom)},
             {"type":"key", "code":primary, "pressed":True},
             {"type":"key", "code":"KeyA", "pressed":True},
             {"type":"key", "code":"KeyA", "pressed":False},
             {"type":"key", "code":primary, "pressed":False}]
    # Native text events contain printable text. Enter uses its physical key
    # event; upstream correctly rejects control characters in Character events.
    lines = content.split("\n")
    for index,line in enumerate(lines):
        if line: steps.append({"type":"text", "text":line})
        if index+1 < len(lines):
            steps += [{"type":"key", "code":"Enter", "pressed":True},
                      {"type":"key", "code":"Enter", "pressed":False}]
    app.input(*steps)
    state = next(p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source))
    assert state["content"] == content, state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/workspace-policy-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = {name:os.environ.get(name) for name in ["HOME","XDG_CONFIG_HOME","SHELL"]}
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as fixture:
            root = Path(fixture)
            home, config, project = root/"home", root/"config", root/"project"
            home.mkdir(); project.mkdir()
            source = project/"source.txt"
            source.write_text("ORIGINAL\n")
            os.environ.update(HOME=str(home), XDG_CONFIG_HOME=str(config), SHELL="/bin/sh")
            with Application([binary], output=output/"temporary", timeout=60) as app:
                initial = session(ready(app))
                assert not app.request("app.inspect_filetree")["project"]["persistent"]
                app.request("app.set_settings", values={"theme":"Light"})
                app.request("app.set_settings", scope="project", values={"line_numbers":False})
                app.request("app.select_layout", slot=2)
                open_panel(app, "Settings")
                clean_quit(app)
            assert (config/"bed2/preferences.bed").exists()
            assert not (config/"bed2/workspaces").exists()
            assert not (home/".bed").exists()
            with Application([binary], output=output/"temporary-restart", timeout=60) as app:
                initial = session(ready(app))
                settings = app.request("app.inspect_settings")
                assert settings["global"]["theme"] == "Light"
                assert settings["project"]["effective"]["line_numbers"] is True
                assert len(initial["layouts"]) == 1
                assert app.request("app.inspect_layouts")["layouts"][0]["slot"] == 1
                clean_quit(app)
            with Application([binary,str(project)], output=output/"folder", timeout=60) as app:
                initial = session(ready(app))
                app.request("wait_frames", frames=30)
                assert not list((config/"bed2/workspaces").glob("*.bed")), "Startup alone created workspace metadata"
                menu = app.request("app.inspect_menus")
                if sys.platform == "darwin":
                    assert menu["native_installed"], menu
                assert any(entry.get("label") == "Layout" for entry in menu["entries"][3]["entries"])
                app.request("app.set_settings", scope="project", values={"line_numbers":False})
                # Use the real Files activation and editor input paths.
                open_panel(app, "Files")
                wait_for(app, lambda:any(any(row["path"] == str(source) for row in tree["rows"]) for tree in app.request("app.inspect_filetree")["panels"]), "Files did not list source")
                app.click_text("source.txt")
                wait_for(app, lambda:any(p["path"] == str(source) and p["content"] == "ORIGINAL\n" for p in app.request("app.inspect_filetree")["previews"]), "Source did not load")
                edit(app, source, "TIMER\n")
                wait_for(app, lambda:source.read_text() == "TIMER\n", "Timer autosave did not write real bytes")
                app.request("app.set_settings", scope="project", values={"autosave":"ChangeFocus"})
                edit(app, source, "FOCUS\n")
                app.input({"type":"click", "position":toolbar(app,"bed.settings")})
                wait_for(app, lambda:source.read_text() == "FOCUS\n", "Focus autosave did not write real bytes")
                app.request("app.set_settings", scope="project", values={"autosave":"Never"})
                edit(app, source, "EXPLICIT\n")
                app.request("wait_frames", frames=100)
                assert source.read_text() == "FOCUS\n"
                app.request("app.activate_menu", path=["File","Save"])
                wait_for(app, lambda:source.read_text() == "EXPLICIT\n", "Explicit Save did not write real bytes")
                app.request("app.select_layout", slot=2)
                open_panel(app, "Structure")
                app.request("app.activate_menu", path=["View","Layout","Save Current Layout"])
                open_panel(app, "Settings")
                before_reset = session(app.inspect())["layouts"]
                target = next(layout for layout in before_reset if layout["active"])
                retired = {panel["entity"] for area in target["areas"] for panel in area["panels"]}
                retained = {panel["entity"] for layout in before_reset if not layout["active"] for area in layout["areas"] for panel in area["panels"]}
                app.request("app.activate_menu", path=["View","Layout","Reset Current Layout"])
                wait_for(app, lambda: all(layout["entity"] != target["entity"] for layout in session(app.inspect())["layouts"]), "Reset did not replace its layout")
                after_reset = app.inspect()
                remaining = {panel["entity"] for panel in panels(after_reset)}
                assert not retired & remaining, "Reset retained its old panels"
                assert retained <= remaining, "Reset retired panels in another layout"
                assert len([p for p in panels(after_reset) if p["key"] == "bed.structure"]) == 1
                current = next(layout for layout in session(after_reset)["layouts"] if layout["active"])
                assert [p["key"] for area in current["areas"] for p in area["panels"]] == ["bed.structure"]
                app.request("app.open_folder", path=str(project))
                assert "workspaces" not in app.inspect()["workbench"]
                wait_for(app, lambda:settled_storage(app), "Workspace metadata did not settle")
                path = Path(settled_storage(app)["path"])
                assert path.parent == config/"bed2/workspaces" and path.suffix == ".bed"
                assert "source.txt" in path.read_text() and "ProjectPreferences" in path.read_text()
                assert str(source) not in (config/"bed2/preferences.bed").read_text(), "Global layout default copied a document path"
                clean_quit(app)
            assert not (project/".bed").exists()
            assert len(list((config/"bed2/workspaces").glob("*.bed"))) == 1
            with Application([binary,str(project)], output=output/"restored", timeout=60) as app:
                initial = session(ready(app))
                slots = app.request("app.inspect_layouts")["layouts"]
                assert {entry["slot"] for entry in slots} == {1,2}
                assert next(entry for entry in slots if entry["active"])["slot"] == 2
                settings = app.request("app.inspect_settings")
                assert settings["project"]["effective"]["line_numbers"] is False
                before = path.read_bytes(); stamp = path.stat().st_mtime_ns
                app.request("wait_frames", frames=30)
                assert path.read_bytes() == before and path.stat().st_mtime_ns == stamp, "Restoration or idle rendering rewrote metadata"
                app.request("app.select_layout", slot=1)
                wait_for(app, lambda:any(p["path"] == str(source) and p["content"] == "EXPLICIT\n" for p in app.request("app.inspect_filetree")["previews"]), "File descriptors did not restore")
                clean_quit(app)
            (output/"summary.json").write_text(json.dumps({"passed":True,"ron_workspace_files":1,"temporary_default":True,"timer_autosave":True,"focus_autosave":True,"native_layout_menu":True,"restored_slots":[1,2]}, indent=2)+"\n")
    finally:
        for name,value in previous.items():
            if value is None: os.environ.pop(name,None)
            else: os.environ[name]=value
    print(f"Workspace policy smoke passed in {output}")


if __name__ == "__main__":
    main()
