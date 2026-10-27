#!/usr/bin/env python3
"""Exercise the real bed executable exclusively through automation."""
import argparse
import json
import re
import sys
from pathlib import Path

from automation import Application


def session(snapshot):
    return snapshot["workbench"]


def panels(snapshot):
    return [p for layout in snapshot["workbench"]["layouts"] for area in layout["areas"] for p in area["panels"]]


def open_panel(app, label, *, area=None, layout=None):
    """Activate the app's native View action, then explicitly place/focus it.

    The automation adapter revalidates the same abstract action as the OS menu;
    native menu text isn't part of Bevy's rendered-text inspection.
    """
    if layout is not None:
        app.request("layout.activate", layout=layout)
    previous = {panel["entity"] for panel in panels(app.inspect())}
    app.request("app.activate_menu", path=["View", label])
    snapshot = app.wait_until(lambda s: any(p["entity"] not in previous for p in panels(s)))
    created = [p for p in panels(snapshot) if p["entity"] not in previous]
    assert len(created) == 1, created
    panel = created[0]["entity"]
    if area is not None:
        app.request("panel.move", panel=panel, area=area)
    app.request("panel.focus", panel=panel)
    return panel


def ready(app):
    for _ in range(300):
        project = app.request("app.inspect_filetree")["project"]
        layouts = app.request("app.inspect_layouts")["layouts"]
        if project and layouts and project["settings_ready"] and project["session_ready"]:
            return app.inspect()
        app.request("wait_frames", frames=1)
    raise AssertionError("Session startup did not settle")


def prepare_demo_layout(app, *, viewport=True):
    """Give independent feature smokes their explicit single-area fixture."""
    ready(app)
    app.request("app.set_settings", values={"autosave": "Never", "smart_wrapping": False})
    app.request("app.select_layout", slot=2)
    snapshot = app.inspect()
    active = session(snapshot)
    current = next(layout["entity"] for layout in active["layouts"] if layout["active"])
    for layout in active["layouts"]:
        if layout["entity"] != current:
            app.request("layout.delete", layout=layout["entity"])
    app.request("wait_frames", frames=20)
    if viewport:
        open_panel(app, "Bed")
    return app.inspect()


def center(rect, zoom=1):
    return [(a+b)*zoom/2 for a,b in zip(rect["min"], rect["max"])]


def toolbar(app, key):
    tools = app.request("app.inspect_titlebar")["tools"]
    rect = next(tool["rect"] for tool in tools if tool["key"] == key)
    return center(rect, app.inspect()["ui"]["surfaces"][0]["zoom"])


def main():
    import os
    import tempfile
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/bed-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    previous = {name: os.environ.get(name) for name in ["XDG_CONFIG_HOME", "HOME", "SHELL"]}
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="config-", dir=output) as directory:
            home = Path(directory)/"home"
            home.mkdir()
            os.environ.update(XDG_CONFIG_HOME=directory, HOME=str(home), SHELL="/bin/sh")
            # The fresh canonical namespace must not read or modify the former app's data.
            previous_preferences = Path(directory) / "bed2" / "preferences.bed"
            previous_preferences.parent.mkdir()
            previous_preferences.write_bytes(b"previous application data; preserve exactly")
            with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
                app.request("configure", fixed_dt=1/60)
                snapshot = ready(app)
                initial = session(snapshot)
                assert len(initial["layouts"]) == 1 and len(initial["layouts"][0]["areas"]) == 1, initial
                assert [p["key"] for p in panels(snapshot)] == ["bedtime.terminal"], snapshot
                assert app.request("app.inspect_settings")["startup"]["open_last_workspace"] is False
                assert app.request("app.inspect_settings")["global"]["smart_wrapping"] is True
                tools = app.request("app.inspect_titlebar")["tools"]
                assert [tool["key"] for tool in sorted(tools, key=lambda t: t["rect"]["min"][0])] == ["bedtime.terminal", "bed.git", "bedtime.filetree", "bed.structure", "bed.settings"], tools
                for key in ["bedtime.terminal", "bedtime.filetree", "bed.structure", "bed.settings"]:
                    app.input({"type": "click", "position": toolbar(app, key)})
                    before = len([p for p in panels(app.inspect()) if p["key"] == key])
                    app.input({"type": "click", "position": toolbar(app, key)})
                    assert len([p for p in panels(app.inspect()) if p["key"] == key]) == before
                snapshot = app.inspect()
                settings = next(p["entity"] for p in panels(snapshot) if p["key"] == "bed.settings")
                largest = max(session(snapshot)["layouts"][0]["areas"], key=lambda a: (a["rect"]["max"][0]-a["rect"]["min"][0])*(a["rect"]["max"][1]-a["rect"]["min"][1]))
                app.request("panel.move", panel=settings, area=largest["entity"])
                app.request("panel.focus", panel=settings)
                app.click_text("Language servers")
                text = " ".join(t["text"] for t in app.inspect()["ui"]["texts"])
                for label in ["Enable language servers", "Go · gopls", "Python · Pyright", "Rust · rust-analyzer", "TypeScript · TypeScript Language Server"]:
                    assert label in text, (label, text)
                header = app.inspect()["ui"]["texts"]
                path = next(t for t in header if t["text"].startswith("/"))
                scope = next(t for t in header if t["text"] == "Workspace")
                assert abs(center(path["rect"])[1] - center(scope["rect"])[1]) <= 1, (path, scope)
                assert path["rect"]["max"][0] < scope["rect"]["min"][0], (path, scope)
                captures.append(app.capture("00-language-servers.png"))
                app.request("window.resize", size=[520, 720])
                app.request("wait_frames", frames=8)
                app.click_text("Editor")
                app.click_text("Language servers")
                captures.append(app.capture("00-settings-narrow.png"))
                app.request("window.resize", size=[1280, 900])
                app.request("wait_frames", frames=8)
                app.click_text("Appearance")
                text = " ".join(t["text"] for t in app.inspect()["ui"]["texts"])
                for label in ["Theme", "Window opacity", "Panel opacity", "Panel border radius", "Panel gap", "File tree indentation"]:
                    assert label in text, (label, text)
                labels = {t["text"]: t["rect"] for t in app.inspect()["ui"]["texts"]}
                for control, name in [("Dark", "Theme"), ("Bedfont", "Font family")]:
                    assert labels[control]["max"][0] < labels[name]["min"][0], labels
                    assert abs(center(labels[control])[1] - center(labels[name])[1]) <= 1, labels
                assert center(labels["Theme"])[1] < center(labels["Font family"])[1] < center(labels["Font size"])[1], labels
                captures.append(app.capture("01-settings-dark.png"))
                app.click_text("Dark")
                app.click_text("Light")
                assert app.request("app.inspect_settings")["global"]["theme"] == "Light"
                captures.append(app.capture("02-settings-light.png"))
                snapshot = app.inspect()
                label = next(text for text in snapshot["ui"]["texts"] if text["text"] == "Workspace")
                zoom = snapshot["ui"]["surfaces"][0]["zoom"]
                app.input({"type": "click", "position": [(label["rect"]["min"][0]-24)*zoom, center(label["rect"], zoom)[1]]})
                app.click_text("Editor")
                app.click_text("Line numbers")
                state = app.request("app.inspect_settings")
                scoped = state["project"]
                assert state["global"]["line_numbers"] and scoped["overrides"]["line_numbers"] is False, state
                captures.append(app.capture("03-workspace-editor.png"))
                app.click_text("↶")
                state = app.request("app.inspect_settings")
                assert state["project"]["overrides"]["line_numbers"] is None
                primary = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
                app.input({"type":"key", "code":primary, "pressed":True}, {"type":"key", "code":"Digit2", "pressed":True}, {"type":"key", "code":"Digit2", "pressed":False}, {"type":"key", "code":primary, "pressed":False})
                slots = app.request("app.inspect_layouts")["layouts"]
                assert next(slot for slot in slots if slot["active"])["slot"] == 2, slots
                snapshot = app.inspect()
                active = next(layout for layout in session(snapshot)["layouts"] if layout["active"])
                target = center(active["areas"][0]["surface"], snapshot["ui"]["surfaces"][0]["zoom"])
                before = len(panels(snapshot))
                app.input({"type":"drag", "from":toolbar(app,"bed.settings"), "to":target, "frames":15})
                app.wait_until(lambda s: len(panels(s)) == before+1)
                snapshot = app.inspect()
                area = next(layout for layout in session(snapshot)["layouts"] if layout["active"])["areas"][0]
                target = [area["surface"]["max"][0]-8, (area["surface"]["min"][1]+area["surface"]["max"][1])/2]
                zoom = snapshot["ui"]["surfaces"][0]["zoom"]
                target = [v*zoom for v in target]
                app.input({"type":"drag", "from":toolbar(app,"bedtime.terminal"), "to":target, "frames":15})
                app.wait_until(lambda s: len(panels(s)) == before+2)
                app.request("app.activate_menu", path=["View","Layout","Save Current Layout"])
                app.request("app.activate_menu", path=["View","Layout","Reset Current Layout"])
                assert len(panels(app.inspect())) == before+2
                captures.append(app.capture("04-dragged-layout.png"))
                assert all(capture["nontransparent_pixels"] > 0 for capture in captures), captures
                assert len({capture["hash"] for capture in captures}) == len(captures), captures
            assert (Path(directory)/"bed/preferences.bed").exists()
            assert not (Path(directory)/"bed/workspaces").exists()
            assert previous_preferences.read_bytes() == b"previous application data; preserve exactly"
    finally:
        for name, value in previous.items():
            if value is None: os.environ.pop(name, None)
            else: os.environ[name] = value
    (output/"summary.json").write_text(json.dumps({"passed":True,"captures":captures,"titlebar_drag":True,"settings_scope":True,"layout_shortcut":True}, indent=2)+"\n")
    print(f"bed foundation smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
