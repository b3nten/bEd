#!/usr/bin/env python3
"""Exercise real +open helpers, cwd/workspace routing and shared file viewers."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time

from automation import Application, AutomationError
from smoke_bed2 import ready, workspace
from smoke_terminal import bootstrap, command, inspection
from smoke_workspace_policy import clean_quit


def wait_for(app, predicate, label):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if predicate():
            return
        app.request("wait_frames", frames=2)
    (app.output / "failure.json").write_text(json.dumps({
        "snapshot": app.inspect(),
        "files": app.request("app.inspect_filetree"),
        "terminal": inspection(app),
    }, indent=2))
    raise AutomationError(label)


def run_helper(app, panel, source, output, label, expected=0):
    status, stderr = output / (label + ".status"), output / (label + ".stderr")
    line = ("{ " + source + "; } >" + shlex.quote(str(stderr))
            + " 2>&1; code=$?; printf '%s' \"$code\" >" + shlex.quote(str(status)))
    command(app, panel, line)
    wait_for(app, lambda: status.exists() and status.read_text() != "", label)
    assert status.read_text() == str(expected), (label, status.read_text(), stderr.read_text())
    return stderr.read_text()


def terminal_for(app, owner):
    wait_for(app, lambda: any(p["workspace"] == owner and p["status"] == "Running"
                             for p in inspection(app)["panels"]), "Workspace terminal startup")
    return next(p["entity"] for p in inspection(app)["panels"] if p["workspace"] == owner)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/open-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = str(Path(args.binary).resolve())
    previous = {key: os.environ.get(key) for key in ["HOME", "XDG_CONFIG_HOME", "SHELL"]}
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as fixture:
            root = Path(fixture).resolve()
            home, project, config = root / "home", root / "project", root / "config"
            home.mkdir()
            project.mkdir()
            (project / "sub").mkdir()
            source = project / "file with spaces.rs"
            source.write_text("fn opened_from_shell() {}\n")
            image = project / "foo.png"
            image.write_bytes(Path("bedtime/tests/fixtures/images/sample.png").read_bytes())
            os.environ.update(HOME=str(home), XDG_CONFIG_HOME=str(config), SHELL="/bin/sh")
            helper = root / "+open"
            helper.symlink_to(binary)
            outside = os.environ.copy()
            outside.pop("BEDTERM_SOCKET", None)
            outside.pop("BEDTERM_PANE", None)
            result = subprocess.run([str(helper)], env=outside, capture_output=True, text=True, timeout=10)
            assert result.returncode == 1 and "BEDTERM_SOCKET is not set" in result.stderr
            assert result.stdout == "" and "bevy_render" not in result.stderr
            with Application([binary], output=output / "first", timeout=60) as app:
                initial = workspace(ready(app))["entity"]
                initial_terminal = terminal_for(app, initial)
                bootstrap(app, initial_terminal, "OPEN_HELPER_READY")
                run_helper(app, initial_terminal,
                           "cd " + shlex.quote(str(project)) + " && +open", root, "cwd")
                wait_for(app, lambda: workspace(app.inspect())["entity"] != initial,
                         "No-argument +open activates a folder workspace")
                folder = workspace(app.inspect())["entity"]
                wait_for(app, lambda: any(w["workspace"] == folder and w["persistent"]
                                         and w["root"] == str(project) and w["session_ready"]
                                         and w["settings_ready"]
                                         for w in app.request("app.inspect_filetree")["workspace_roots"]),
                         "Folder workspace is restored")
                app.request("workspace.activate", workspace=initial)
                run_helper(app, initial_terminal, "+open", root, "reuse")
                assert workspace(app.inspect())["entity"] == folder
                assert len(app.inspect()["workbench"]["workspaces"]) == 2
                panel = terminal_for(app, folder)
                bootstrap(app, panel, "FOLDER_SHELL_READY")
                relative = "cd " + shlex.quote(str(project / "sub")) + " && +open '../file with spaces.rs' ../foo.png"
                run_helper(app, panel, relative, root, "files")
                previews = app.request("app.inspect_filetree")["previews"]
                text = next(p for p in previews if p["path"] == str(source))
                metadata = next(p for p in previews if p["path"] == str(image))
                assert text["workspace"] == folder and text["pinned"]
                assert text["content"] == source.read_text() and text["error"] is None
                assert metadata["pinned"] and metadata["kind"] == "image" and metadata["error"] is None
                assert not app.request("app.inspect_filetree")["panels"], "Opening files needs no tree"
                run_helper(app, panel, "+o '../file with spaces.rs'", root, "alias")
                previews = app.request("app.inspect_filetree")["previews"]
                assert next(p for p in previews if p["path"] == str(source))["entity"] == text["entity"]
                assert len(previews) == 2
                message = run_helper(app, panel, "+open missing-file", root, "missing", expected=1)
                assert "missing-file" in message
                app.request("panel.focus", panel=text["entity"])
                app.request("wait_frames", frames=4)
                captures.append(app.capture("opened-file.png"))
                # Session persistence is exercised through the normal quit gate.
                clean_quit(app)
            with Application([binary], output=output / "restored", timeout=60) as app:
                origin = workspace(ready(app))["entity"]
                panel = terminal_for(app, origin)
                bootstrap(app, panel, "RESTORE_SHELL_READY")
                run_helper(app, panel, "cd " + shlex.quote(str(project)) + " && +open",
                           root, "restore-cwd")
                wait_for(app, lambda: len(app.request("app.inspect_filetree")["previews"]) == 2,
                         "Folder reopening restores helper-created tabs")
                wait_for(app, lambda: all(not p["loading"] for p in app.request("app.inspect_filetree")["previews"]),
                         "Restored files finish loading")
                assert {p["path"] for p in app.request("app.inspect_filetree")["previews"]} == {str(source), str(image)}
                captures.append(app.capture("restored-files.png"))
                clean_quit(app)
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
    assert all(c["nontransparent_pixels"] > 0 for c in captures)
    (output / "summary.json").write_text(json.dumps({
        "passed": True, "captures": captures, "cwd_workspace": True,
        "reuse_workspace": True, "relative_files": True, "alias": True,
        "missing_file_exit_status": True, "session_restore": True,
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
