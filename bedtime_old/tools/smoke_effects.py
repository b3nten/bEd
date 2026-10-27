#!/usr/bin/env python3
"""Exercise bed's GPU effects, curved hit-testing and persisted Appearance values."""
import argparse
import json
import math
import os
from pathlib import Path
import tempfile

from automation import Application
from smoke_bed import ready, panels, center, open_panel
from smoke_titlebar import rgba_rows
from smoke_workspace_policy import clean_quit


ZERO = {name: 0.0 for name in (
    "scanline_intensity", "vignette_intensity", "bloom_intensity", "noise_intensity",
    "curvature_intensity", "pixel_grid_intensity", "burn_in_intensity", "pulse_intensity")}


def display_position(app, source, curvature):
    """Invert the radial sampling map to click a known unwarped widget."""
    width, height = app.inspect()["windows"][0]["size"]
    c = [source[0] / width - 0.5, source[1] / height - 0.5]
    radius = math.hypot(*c)
    lo, hi = 0.0, radius
    for _ in range(40):
        r = (lo + hi) / 2
        if r * (1 + curvature * 0.25 * r * r) < radius:
            lo = r
        else:
            hi = r
    scale = (lo + hi) / (2 * radius) if radius else 1.0
    return [(0.5 + c[0] * scale) * width, (0.5 + c[1] * scale) * height]


def click_label(app, label, curvature):
    texts = app.inspect()["ui"]["texts"]
    target = next(text for text in texts if text["text"] == label)
    app.input({"type": "click", "position": display_position(app, center(target["rect"]), curvature)})


def pixels(capture):
    return b"".join(rgba_rows(capture["path"]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/bed-effects-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = {name: os.environ.get(name) for name in ["XDG_CONFIG_HOME", "HOME", "SHELL"]}
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="config-", dir=output) as directory:
            home = Path(directory) / "home"
            home.mkdir()
            os.environ.update(XDG_CONFIG_HOME=directory, HOME=str(home), SHELL="/bin/sh")
            with Application([binary], output=output / "first", timeout=60) as app:
                snapshot = ready(app)
                assert app.request("app.inspect_settings")["global"]["effects_enabled"] is False
                app.input({"type": "move", "position": [450, 500]})
                scale = snapshot["windows"][0]["scale_factor"]
                crop = [int(200 * scale), 0, int(740 * scale), int(50 * scale)]
                app.request("app.set_settings", values={**ZERO, "effects_enabled": False})
                app.request("wait_frames", frames=20)
                off = app.capture("off.png", crop=crop)
                app.request("app.set_settings", values={"effects_enabled": True})
                app.request("wait_frames", frames=20)
                neutral = app.capture("neutral.png", crop=crop)
                assert max(abs(a-b) for a,b in zip(pixels(off), pixels(neutral))) <= 1, "Neutral effects changed color or alpha"
                captures += [off, neutral]

                # Near the top-right edge, curvature moves the target by more
                # than its height. A physical click at its displayed position
                # must still open Settings through egui.
                app.request("app.set_settings", values={"curvature_intensity": 0.8})
                app.request("wait_frames", frames=10)
                tool = next(t for t in app.request("app.inspect_titlebar")["tools"] if t["key"] == "bed.settings")
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
                logical = center(tool["rect"], zoom)
                displayed = display_position(app, logical, 0.8)
                assert abs(displayed[1] - logical[1]) > 10
                app.input({"type": "click", "position": displayed})
                app.wait_until(lambda s: any(p["key"] == "bed.settings" and p["visible"] for p in panels(s)))
                click_label(app, "Appearance", 0.8)
                app.request("window.resize", size=[1280, 1000])
                app.request("wait_frames", frames=20)
                assert any(t["text"] == "Shader effects" for t in app.inspect()["ui"]["texts"])
                captures.append(app.capture("appearance-curved.png"))
                label = next(t for t in app.inspect()["ui"]["texts"] if t["text"] == "Bloom")
                zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
                x, y = center(label["rect"])
                x = label["rect"]["min"][0]
                app.input({"type": "drag",
                           "from": display_position(app, [x - 150 * zoom, y], 0.8),
                           "to": display_position(app, [x - 90 * zoom, y], 0.8), "frames": 12})
                assert app.request("app.inspect_settings")["global"]["bloom_intensity"] > 0.5, "Curved slider drag missed"
                click_label(app, "Shader effects", 0.8)
                assert app.request("app.inspect_settings")["global"]["effects_enabled"] is False
                assert abs(app.request("app.inspect_settings")["global"]["curvature_intensity"] - 0.8) < 1e-6
                click_label(app, "Shader effects", 0.0)
                assert app.request("app.inspect_settings")["global"]["effects_enabled"] is True

                app.request("app.set_settings", values={
                    "scanline_intensity": 0.4, "vignette_intensity": 0.25,
                    "bloom_intensity": 0.3, "noise_intensity": 0.1,
                    "pixel_grid_intensity": 0.2, "burn_in_intensity": 0.95, "pulse_intensity": 0.1})
                app.request("wait_frames", frames=30)
                captures.append(app.capture("all-effects.png"))
                app.request("app.set_settings", values={"bloom_intensity": 0.0})
                app.request("wait_frames", frames=10)
                captures.append(app.capture("bloom-disabled.png"))
                app.request("app.set_settings", values={"bloom_intensity": 0.3})
                # History must survive ordinary frames, and reset on size and
                # layout changes without sampling retired textures.
                app.request("window.resize", size=[1001, 741])
                app.request("app.select_layout", slot=2)
                app.request("wait_frames", frames=20)
                captures.append(app.capture("resized-layout.png"))
                open_panel(app, "Bed")
                app.request("wait_frames", frames=30)
                captures.append(app.capture("viewport.png"))
                app.request("app.set_settings", values={"noise_intensity": 0.0, "pulse_intensity": 0.0})
                for opacity in [1.0, 0.6, 0.0]:
                    app.request("app.set_settings", values={"window_opacity": opacity})
                    app.request("wait_frames", frames=20)
                    capture = app.capture(f"alpha-{opacity}.png", crop=crop)
                    rgba = pixels(capture)
                    assert max(max(rgba[i:i+3]) - rgba[i+3] for i in range(0, len(rgba), 4)) <= 1
                    if opacity == 0.0:
                        assert 0 in rgba[3::4], "Effects made the transparent background opaque"
                    captures.append(capture)
                clean_quit(app)
            log = (output / "first/stderr.log").read_text()
            assert "ERROR" not in log and "panicked" not in log, log
            with Application([binary], output=output / "restored", timeout=60) as app:
                ready(app)
                settings = app.request("app.inspect_settings")["global"]
                assert settings["effects_enabled"] is True
                for key, value in {"curvature_intensity": 0.8, "burn_in_intensity": 0.95, "bloom_intensity": 0.3}.items():
                    assert abs(settings[key] - value) < 1e-6, settings
                app.request("app.set_settings", values={"effects_enabled": False})
                app.request("wait_frames", frames=10)
                captures.append(app.capture("restored-disabled.png"))
                clean_quit(app)
            assert "ERROR" not in (output / "restored/stderr.log").read_text()
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
    (output / "summary.json").write_text(json.dumps({"captures": captures}, indent=2))
    print(f"Effects, curved clicks, alpha and persistence passed; {len(captures)} GPU captures")


if __name__ == "__main__":
    main()
