#!/usr/bin/env python3
"""Verify rendered diagnostics, F12 cross-file navigation, saves and restart."""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import time

from automation import Application
from smoke_bed2 import open_panel, prepare_demo_layout
from smoke_editor import navigate, primary, editor_context_menu
from smoke_workspace_policy import clean_quit


def wait(app, predicate, seconds=30):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        value = app.request("app.inspect_lsp")
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AssertionError(f"LSP did not settle: {value}")


def wait_servers(app, predicate, seconds=30):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        value = app.request("app.inspect_lsp_servers")
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AssertionError(f"LSP observations did not settle: {value}")


def escape(app):
    app.input({"type": "key", "code": "Escape", "pressed": True},
              {"type": "key", "code": "Escape", "pressed": False})


def status_menu(app):
    bar = app.request("app.inspect_status_bar")["bars"][0]
    menu = bar["menu_rect"]
    app.input({"type": "click", "position": [(menu["min"][0] + menu["max"][0]) / 2,
                                               (menu["min"][1] + menu["max"][1]) / 2]})


def ui_texts(app):
    return [target["text"] for target in app.inspect()["ui"]["texts"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/lsp-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    captures = []
    previous = os.environ.get("XDG_CONFIG_HOME")
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as temporary:
            root = Path(temporary)
            os.environ["XDG_CONFIG_HOME"] = str(root / "config")
            project = root / "project"
            project.mkdir()
            source, target = project / "main.rs", project / "target.rs"
            source.write_text("fn first() {}\nfn caller() { first(); }\n")
            target.write_text("fn destination() {}\n")
            transcript = root / "server.jsonl"
            server = Path(__file__).resolve().parents[1] / "crates/bedtime-lsp2/tests/server.py"
            config = {"program": sys.executable, "arguments": [str(server), "cross-file", str(transcript)],
                      "environment": {}, "settings": {}, "initialization_options": None}
            with Application([str(Path(args.binary).resolve()), str(project)], output=output, timeout=60) as app:
                prepare_demo_layout(app, viewport=True)
                app.request("app.set_settings", values={"autosave": "Never", "language_servers": {}})
                app.request("configure", fixed_dt=1 / 60, clipboard="")
                open_panel(app, "Files")
                until = time.monotonic() + 20
                while True:
                    trees = app.request("app.inspect_filetree")["panels"]
                    if any(any(row["path"] == str(source) for row in tree["rows"]) for tree in trees):
                        break
                    assert time.monotonic() < until
                    app.request("wait_frames", frames=2)
                app.click_text("main.rs")
                app.request("wait_frames", frames=4)
                assert app.request("app.inspect_status_bar")["bars"] == [], "No LSP should reserve no status row"
                view = next(p for p in app.request("app.inspect_filetree")["previews"] if p["path"] == str(source))
                panel = view["entity"]
                navigate(app, panel, 0)
                primary(app, "KeyA")
                for action in ["Copy", "Cut", "Paste"]:
                    editor_context_menu(app, panel)
                    labels = ui_texts(app)
                    assert all(label in labels for label in ["Cut", "Copy", "Paste"]), labels
                    assert not any(label in labels for label in ["Save", "Reload", "Split view", "Keep open", "Editing", "Line numbers", "Folding", "Language server", "Go to Definition     F12"]), labels
                    app.click_text(action)
                    app.request("wait_frames", frames=3)
                    content = next(p["content"] for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == panel)
                    assert content == ("" if action == "Cut" else source.read_text()), (action, content)
                app.request("app.set_settings", values={"language_servers": {"rust": config}})
                wait(app, lambda value: any(d["status"] == "Some(Ready)" and d["diagnostics"]
                                          and d["diagnostics"]["items"] for d in value["documents"]))
                app.request("wait_frames", frames=2)
                assert any(bar["errors"] == 1 for bar in app.request("app.inspect_status_bar")["bars"])
                previews = app.request("app.inspect_filetree")["previews"]
                panel = next(p["entity"] for p in previews if p["path"] == str(source))
                navigate(app, panel, 28)
                captures.append(app.capture("01-diagnostics.png"))
                editor_context_menu(app, panel)
                assert "Go to Definition     F12" in ui_texts(app)
                escape(app)
                app.input({"type": "key", "code": "F12", "pressed": True},
                          {"type": "key", "code": "F12", "pressed": False})
                until = time.monotonic() + 20
                while True:
                    previews = app.request("app.inspect_filetree")["previews"]
                    opened = next((p for p in previews if p["path"] == str(target) and p["document"]), None)
                    if opened:
                        selection = next(p["selection"] for p in app.request("app.inspect_code")["panels"]
                                         if p["panel"] == opened["entity"])
                        if selection == [0, 1]:
                            break
                    assert time.monotonic() < until, previews
                    app.request("wait_frames", frames=2)
                assert not opened["pinned"], "Definition destinations should remain temporary preview tabs"
                captures.append(app.capture("02-definition.png"))
                app.click_text("main.rs")
                until = time.monotonic() + 20
                while True:
                    previews = app.request("app.inspect_filetree")["previews"]
                    source_view = next((p for p in previews if p["path"] == str(source) and p["document"] and not p["loading"]), None)
                    if source_view:
                        panel = source_view["entity"]
                        break
                    assert time.monotonic() < until
                    app.request("wait_frames", frames=2)
                rect = source_view["editor_rect"]
                app.input({"type": "click", "position": [rect["min"][0] + 60, rect["min"][1] + 10]})
                navigate(app, panel, 0)
                primary(app, "KeyA")
                replacement = "fn edited() {}\n"
                app.input({"type": "text", "text": replacement.rstrip("\n")},
                          {"type": "key", "code": "Enter", "pressed": True},
                          {"type": "key", "code": "Enter", "pressed": False})
                primary(app, "KeyS")
                until = time.monotonic() + 20
                while True:
                    records = [json.loads(line) for line in transcript.read_text().splitlines()]
                    if any(r.get("method") == "textDocument/didSave" and r["params"]["text"] == replacement for r in records):
                        break
                    assert time.monotonic() < until, records
                    app.request("wait_frames", frames=2)
                assert source.read_text() == replacement
                status_menu(app)
                labels = ui_texts(app)
                assert "Open Language Servers" in labels and "Bedtime fixture" in labels, labels
                assert "The server has not published a diagnostic report for this document." not in labels
                app.click_text("Bedtime fixture")
                assert "Restart" in ui_texts(app) and "Stop" in ui_texts(app)
                app.click_text("Stop")
                wait_servers(app, lambda value: any(s["status"] == "Stopped" and not s["retiring"] for s in value["servers"]))
                assert app.request("app.inspect_status_bar")["bars"], "The stopped server must remain restartable"
                # F12 produces an actionable, temporary message on the actual row.
                navigate(app, panel, 0)
                app.input({"type": "key", "code": "F12", "pressed": True},
                          {"type": "key", "code": "F12", "pressed": False})
                error_bar = next(bar for bar in app.request("app.inspect_status_bar")["bars"] if bar["document_error"])
                error = error_bar["document_error"]
                assert error in ui_texts(app) and "Document error" not in ui_texts(app), ui_texts(app)
                error_target = next(t for t in app.inspect()["ui"]["texts"] if t["text"] == error)
                rect = error_target["rect"]
                app.input({"type": "move", "position": [(rect["min"][0]+rect["max"][0])/2, (rect["min"][1]+rect["max"][1])/2]})
                app.request("wait_frames", frames=3)
                assert ui_texts(app).count(error) >= 2, ui_texts(app)
                time.sleep(8.2)
                app.request("wait_frames", frames=3)
                assert all(bar["document_error"] is None for bar in app.request("app.inspect_status_bar")["bars"])
                status_menu(app)
                app.click_text("Bedtime fixture")
                app.click_text("Restart")
                wait(app, lambda value: any(d["status"] == "Some(Ready)" for d in value["documents"]))
                app.request("app.lsp_restart", panel=panel)
                wait(app, lambda value: any(d["status"] == "Some(Ready)" for d in value["documents"]))
                bad = dict(config, program="/nonexistent/bedtime-lsp-fixture")
                app.request("app.set_settings", values={"language_servers": {"rust": bad}})
                wait(app, lambda value: any("Failed" in d["status"] for d in value["documents"]))
                until = time.monotonic() + 10
                while True:
                    bars = app.request("app.inspect_status_bar")["bars"]
                    if any(bar["server_errors"] == 1 and bar["lsp"] == "LSP error" for bar in bars):
                        break
                    assert time.monotonic() < until, bars
                    app.request("wait_frames", frames=2)
                menu = next(bar["menu_rect"] for bar in bars if bar["lsp"] == "LSP error")
                app.input({"type": "click", "position": [(menu["min"][0]+menu["max"][0])/2,
                                                          (menu["min"][1]+menu["max"][1])/2]})
                captures.append(app.capture("03-startup-error.png"))
                app.input({"type": "key", "code": "Escape", "pressed": True},
                          {"type": "key", "code": "Escape", "pressed": False})
                app.request("app.set_settings", values={"language_servers": {"rust": config}})
                wait(app, lambda value: any(d["status"] == "Some(Ready)" for d in value["documents"]))
                progress_config = dict(config, arguments=[str(server), "progress", str(transcript)])
                app.request("app.set_settings", values={"language_servers": {"rust": progress_config}})
                observed = wait_servers(app, lambda value: any(len(s["progress"]) == 2 and len(s["messages"]) == 64 for s in value["servers"]))
                active = next(s for s in observed["servers"] if not s["retiring"] and len(s["progress"]) == 2)
                open_panel(app, "Language Servers")
                app.request("wait_frames", frames=2)
                bars = app.request("app.inspect_status_bar")["bars"]
                assert any(bar["no_report"] and len(bar["work"]) == 2 for bar in bars), bars
                captures.append(app.capture("04-progress-panel.png"))
                # Use the panel's actual restart control. Observation panels do
                # not own workers, so the process generation must change here.
                app.click_text("Restart")
                observed = wait_servers(app, lambda value: any(s["pid"] != active["pid"] and len(s["progress"]) == 2 for s in value["servers"]))
                active = next(s for s in observed["servers"] if not s["retiring"] and len(s["progress"]) == 2)
                lsp_panel = observed["panels"][0]
                navigate(app, panel, 0)
                app.input({"type": "text", "text": "//"})
                wait_servers(app, lambda value: any(len(s["progress"]) == 1 and s["progress"][0]["percentage"] == 50 for s in value["servers"]))
                primary(app, "KeyS")
                wait_servers(app, lambda value: any(s["pid"] == active["pid"] and s["progress"] == [] for s in value["servers"]))
                app.request("panel.focus", panel=lsp_panel)
                app.request("wait_frames", frames=2)
                bars = app.request("app.inspect_status_bar")["bars"]
                assert any(bar["no_report"] and bar["work"] == [] and not bar["stale_diagnostics"] for bar in bars), bars
                for bar in bars:
                    rect, menu = bar["rect"], bar["menu_rect"]
                    assert rect["max"][1] - rect["min"][1] <= 26, bar
                    assert rect["min"][1] <= menu["min"][1] < menu["max"][1] <= rect["max"][1], bar
                texts = [text["text"] for text in app.inspect()["ui"]["texts"]]
                assert not any(text in texts for text in ["0 errors", "0 warnings", "No diagnostic report", "Diagnostics stale", "Unversioned", "LSP ready"]), texts
                captures.append(app.capture("05-idle-without-diagnostics.png"))
                app.request("panel.close", panel=lsp_panel)
                observed = app.request("app.inspect_lsp_servers")
                assert any(s["pid"] == active["pid"] and s["status"] == "Ready" for s in observed["servers"])
                clean_quit(app)
            records = [json.loads(line) for line in transcript.read_text().splitlines()]
            (output / "server.jsonl").write_text(transcript.read_text())
            assert any(r.get("method") == "shutdown" for r in records)
            assert any(r.get("method") == "exit" for r in records)
            assert all(c["nontransparent_pixels"] > 0 for c in captures)
    finally:
        if previous is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"LSP smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
