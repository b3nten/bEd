#!/usr/bin/env python3
"""Check bEd's live font picker, terminal resize and inherited RON settings."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from automation import Application, AutomationError
from smoke_bed import ready, session, open_panel
from smoke_terminal import wait_state, capture_checked
from smoke_workspace_policy import clean_quit


def settled(app):
    app.request("wait_frames", frames=5)
    for _ in range(600):
        settings = app.request("app.inspect_settings")
        if not settings["fonts"]["loading"]:
            assert settings["fonts"]["error"] is None, settings
            return settings
        app.request("wait_frames", frames=1)
    raise AssertionError("Installed font discovery did not finish")


def assert_font(settings, requested, resolved):
    assert settings["fonts"]["applied"] == [{"requested": requested, "resolved": resolved}], settings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/bed-font-smoke")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    captures = []
    previous = {name: os.environ.get(name) for name in ["XDG_CONFIG_HOME", "HOME", "SHELL"]}
    try:
        with tempfile.TemporaryDirectory(prefix="config-", dir=output) as config:
            home = Path(config) / "home"
            project = Path(config) / "project"
            home.mkdir()
            project.mkdir()
            os.environ.update(XDG_CONFIG_HOME=config, HOME=str(home), SHELL="/bin/sh")
            with Application([binary, str(project)], output=output / "first", timeout=60) as app:
                snapshot = ready(app)
                settings = settled(app)
                assert settings["global"]["font_family"] == "Bedfont"
                assert settings["global"]["font_size"] == 15.5
                assert_font(settings, "Bedfont", "Bedfont")
                families = settings["fonts"]["available"]
                assert families[0] == "Bedfont" and len(families) > 1, families
                family = next((f for f in ["Courier New", "Menlo", "DejaVu Sans Mono", "Liberation Mono"]
                               if f in families[:10]), families[1])
                terminal = app.request("app.inspect_terminal")["panels"][0]["entity"]
                original = wait_state(app, terminal, lambda p: p["status"] == "Running")
                pid = original["pid"]
                grid = original["grid"]["cols"], original["grid"]["rows"]
                captures.append(capture_checked(app, "01-bedfont.png"))

                panel = open_panel(app, "Settings")
                app.click_text("Appearance")
                app.click_text("Bedfont")
                # This uses the real egui dropdown and installed font collection index.
                app.click_text(family)
                settings = settled(app)
                assert settings["global"]["font_family"] == family
                assert_font(settings, family, family)
                captures.append(capture_checked(app, "02-system-family.png"))

                app.request("app.set_settings", values={"font_size": 22.0})
                app.request("panel.focus", panel=terminal)
                resized = wait_state(app, terminal, lambda p:
                                     (p["grid"]["cols"], p["grid"]["rows"]) != grid)
                assert resized["pid"] == pid
                captures.append(capture_checked(app, "03-larger-terminal.png"))
                app.request("app.set_settings", scope="project",
                            values={"font_family": "Bedfont", "font_size": 18.0})
                settings = settled(app)
                assert_font(settings, "Bedfont", "Bedfont")
                local = settings["project"]
                assert local["effective"]["font_size"] == 18.0
                assert settings["global"]["font_family"] == family
                assert settings["global"]["font_size"] == 22.0

                app.request("app.set_settings", scope="project", values={"font_family": None})
                assert_font(settled(app), family, family)
                app.request("app.set_settings", scope="project",
                            values={"font_family": "Missing bEd smoke font"})
                assert_font(settled(app), "Missing bEd smoke font", "Bedfont")
                app.request("app.set_settings", scope="project", values={"font_family": "Bedfont"})
                for values in [{"font_size": 0}, {"font_family": ""}]:
                    try:
                        app.request("app.set_settings", values=values)
                    except AutomationError:
                        pass
                    else:
                        raise AssertionError("Invalid font setting was accepted")
                app.request("panel.focus", panel=panel)
                captures.append(capture_checked(app, "04-workspace-settings.png"))
                clean_quit(app)

            with Application([binary, str(project)], output=output / "restored", timeout=60) as app:
                snapshot = ready(app)
                settings = settled(app)
                assert settings["global"]["font_family"] == family
                assert settings["global"]["font_size"] == 22.0
                local = settings["project"]
                assert local["overrides"]["font_family"] == "Bedfont"
                assert local["effective"]["font_size"] == 18.0
                assert_font(settings, "Bedfont", "Bedfont")
                app.request("app.set_settings", scope="project",
                            values={"font_family": None, "font_size": None})
                assert_font(settled(app), family, family)
                clean_quit(app)
            result = {"system_family": family, "captures": captures,
                      "checked": ["font picker", "live terminal resize and PID", "workspace inheritance/reset",
                                  "missing family fallback", "validation", "RON persistence"]}
            (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


if __name__ == "__main__":
    main()
