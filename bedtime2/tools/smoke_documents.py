#!/usr/bin/env python3
"""Exercise shared typed/text document sessions through the real bEd2 UI."""
import argparse
import json
import re
import sys
from pathlib import Path

from automation import Application, AutomationError


def inspect(app):
    return app.request("app.inspect_documents")


def document(app, identifier):
    return next(d for d in inspect(app)["documents"] if d["id"] == identifier)


def wait_document(app, identifier, predicate, frames=600):
    for _ in range(frames):
        value = document(app, identifier)
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AutomationError(f"Document did not reach expected state: {value}")


def shortcut(app, key, *, shift=False):
    modifier = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"
    steps = [{"type": "key", "code": modifier, "pressed": True}]
    if shift:
        steps.append({"type": "key", "code": "ShiftLeft", "pressed": True})
    steps += [{"type": "key", "code": key, "pressed": True},
              {"type": "key", "code": key, "pressed": False}]
    if shift:
        steps.append({"type": "key", "code": "ShiftLeft", "pressed": False})
    steps.append({"type": "key", "code": modifier, "pressed": False})
    app.input(*steps)


def click(app, text):
    # The conflict controls live below the editor in a normal scroll area.
    for direction, count in [(1, 5), (-1, 12)]:
        for _ in range(count):
            if any(t["text"] == text for t in app.inspect()["ui"]["texts"]):
                app.click_text(text)
                return
            app.input({"type": "move", "position": [900, 600]},
                      {"type": "scroll", "delta": [0, direction * 8]})
    raise AutomationError(f"Could not reveal document control: {text!r}")


def replace_text(app, current, value):
    click(app, current)
    shortcut(app, "KeyA")
    # Bevy's Character input rejects control characters in a single text event.
    # Exercise normal multiline typing with separate Enter events instead.
    steps = []
    for index, line in enumerate(value.split("\n")):
        if index:
            steps += [{"type": "key", "code": "Enter", "pressed": True},
                      {"type": "key", "code": "Enter", "pressed": False}]
        if line:
            steps.append({"type": "text", "text": line})
    if not steps:
        steps = [{"type": "key", "code": "Backspace", "pressed": True},
                 {"type": "key", "code": "Backspace", "pressed": False}]
    app.input(*steps)


def save(app, identifier):
    click(app, "Save")
    value = wait_document(app, identifier, lambda d: not d["saving"] and not d["dirty"])
    assert value["last_save_error"] is None, value
    assert value["memory_source"]["content"] == value["content"], value
    return value


def expect_error(app, op, **arguments):
    try:
        app.request(op, **arguments)
    except AutomationError:
        return
    raise AssertionError(f"Expected {op} to reject {arguments}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/documents-smoke")
    args = parser.parse_args()
    output = Path(args.output)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []

    with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        snapshot = app.wait_until(lambda s: bool(s["viewports"]))
        workspace = snapshot["workbench"]["workspaces"][0]["entity"]
        picker = app.request("panel.open", key="bed.documents")["panel"]
        click(app, "Create / reopen text")
        state = inspect(app)
        text = next(d for d in state["documents"] if d["kind"] == "text")
        identifier, initial = text["id"], text["content"]
        first = state["viewers"][0]["panel"]
        click(app, "Duplicate viewer")
        state = inspect(app)
        assert len(state["viewers"]) == 2
        assert all(v["document"] == identifier for v in state["viewers"])
        captures.append(app.capture("01-shared-text.png"))

        edited = "One shared UTF-8 document: café and 界.\n"
        replace_text(app, initial, edited)
        assert document(app, identifier)["content"] == edited
        assert document(app, identifier)["dirty"]
        # Undo from the OTHER focused text widget must use document history, not
        # a stale widget-private undo buffer. Redo also leaves both views current.
        app.request("panel.focus", panel=first)
        click(app, edited)
        shortcut(app, "KeyZ")
        assert document(app, identifier)["content"] == initial
        shortcut(app, "KeyZ", shift=True)
        assert document(app, identifier)["content"] == edited
        shortcut(app, "KeyS")
        wait_document(app, identifier, lambda d: not d["saving"] and not d["dirty"])
        assert document(app, identifier)["memory_source"]["content"] == edited
        captures.append(app.capture("02-saved-text.png"))

        click(app, "External update + observe")
        external = document(app, identifier)["content"]
        assert external != edited and not document(app, identifier)["dirty"]
        click(app, "Undo")
        assert document(app, identifier)["content"] == edited
        assert document(app, identifier)["dirty"]
        assert document(app, identifier)["memory_source"]["content"] == external
        click(app, "Redo")
        assert not document(app, identifier)["dirty"]

        divergent = "Keep these local changes.\n"
        replace_text(app, external, divergent)
        click(app, "External update + observe")
        assert document(app, identifier)["content"] == divergent
        assert document(app, identifier)["conflict"]
        captures.append(app.capture("03-explicit-conflict.png"))
        click(app, "Keep local")
        assert not document(app, identifier)["conflict"]
        assert document(app, identifier)["dirty"]
        save(app, identifier)

        # An unobserved source change must fail compare-and-swap, never overwrite.
        click(app, "Change source only")
        source = document(app, identifier)["memory_source"]["content"]
        click(app, "Save")
        failed = wait_document(app, identifier, lambda d: not d["saving"] and d["last_save_error"] is not None)
        assert failed["requires_source_refresh"] and failed["memory_source"]["content"] == source
        click(app, "Observe source")
        assert not document(app, identifier)["conflict"]
        assert document(app, identifier)["content"] == source

        latest = "A successful retry preserves the shared history.\n"
        replace_text(app, source, latest)
        click(app, "Reject saves (simulate a backend failure)")
        click(app, "Save")
        failed = wait_document(app, identifier, lambda d: not d["saving"] and d["last_save_error"] is not None)
        assert failed["dirty"] and failed["content"] == latest
        click(app, "Reject saves (simulate a backend failure)")
        save(app, identifier)
        captures.append(app.capture("04-recovered-save.png"))

        # Reopen in another layout keeps the instance AND its history. Closing
        # every viewer still leaves this workspace's document alive.
        layout = app.request("layout.create", name="Documents", activate=True)["layout"]
        picker2 = app.request("panel.open", key="bed.documents")["panel"]
        click(app, "Create / reopen text")
        assert len(inspect(app)["documents"]) == 1
        revision = document(app, identifier)["revision"]
        for viewer in list(inspect(app)["viewers"]):
            app.request("panel.close", panel=viewer["panel"])
        assert inspect(app)["viewers"] == []
        assert document(app, identifier)["revision"] == revision
        app.request("panel.focus", panel=picker2)
        click(app, "Create / reopen text")
        assert document(app, identifier)["content"] == latest
        assert document(app, identifier)["can_undo"]

        app.request("panel.focus", panel=picker2)
        click(app, "Create / reopen model")
        typed = next(d for d in inspect(app)["documents"] if d["kind"] == "model")
        typed_id = typed["id"]
        click(app, "Duplicate viewer")
        click(app, "+1")
        assert document(app, typed_id)["content"]["count"] == 2
        click(app, "Undo")
        assert document(app, typed_id)["content"]["count"] == 1
        click(app, "Redo")
        save(app, typed_id)
        click(app, "External update + observe")
        assert document(app, typed_id)["content"]["count"] == 12
        assert not document(app, typed_id)["dirty"]
        captures.append(app.capture("05-typed-model.png"))

        # Source identity is workspace-local, and the external layout-move ingress
        # must reject cross-workspace document bindings before modifying ownership.
        other = app.request("workspace.create", name="Other", activate=True)["workspace"]
        app.request("panel.open", key="bed.documents")
        click(app, "Create / reopen text")
        other_text = next(d for d in inspect(app)["documents"] if d["workspace"] == other)
        assert other_text["id"] != identifier and other_text["content"] == initial
        app.request("layout.activate", layout=None, workspace=workspace)
        expect_error(app, "layout.move", layout=layout, workspace=other)
        assert document(app, identifier)["workspace"] == workspace
        assert all(v["error"] is None for v in inspect(app)["viewers"]), inspect(app)
        replace_text(app, initial, "Unsaved in another workspace\n")
        expect_error(app, "workspace.close", workspace=other, replacement=workspace)
        assert document(app, other_text["id"])["dirty"]
        assert inspect(app)["quit_phase"] == "idle"
        captures.append(app.capture("06-workspace-ownership.png"))
        assert all(s["diagnostics"]["error"] is None for s in app.inspect()["ui"]["surfaces"])

    log = re.sub(r"\x1b\[[0-9;]*m", "", (output / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"bed2 documents smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
