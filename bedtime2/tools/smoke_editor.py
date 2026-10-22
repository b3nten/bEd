#!/usr/bin/env python3
"""Native editor smoke: shared views/history, real saves, navigation and a million lines.

Uses only temporary files and the automation memory clipboard. Build bed2 with
--features automation first. Captures and a machine-readable summary are retained.
"""
import argparse
import json
import sys
import tempfile
import time
from pathlib import Path
from automation import Application, AutomationError
from smoke_bed2 import open_panel, prepare_demo_layout

PRIMARY = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"


def primary(app, key, shift=False):
    steps = [{"type": "key", "code": PRIMARY, "pressed": True}]
    if shift:
        steps.append({"type": "key", "code": "ShiftLeft", "pressed": True})
    steps += [{"type": "key", "code": key, "pressed": True},
              {"type": "key", "code": key, "pressed": False}]
    if shift:
        steps.append({"type": "key", "code": "ShiftLeft", "pressed": False})
    steps.append({"type": "key", "code": PRIMARY, "pressed": False})
    app.input(*steps)


def inspect(app):
    return app.request("app.inspect_filetree")


def wait(app, predicate, seconds=90):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = inspect(app)
        if predicate(last):
            return last
        app.request("wait_frames", frames=2)
    raise AutomationError(f"Editor condition timed out: {str(last)[:4000]}")


def viewer(state, panel):
    return next(p for p in state["previews"] if p["entity"] == panel)


def navigate(app, panel, position, *, select=True, focus=True, reveal="minimal"):
    current = viewer(inspect(app), panel)
    arguments = dict(panel=panel, revision=current["document_info"]["revision"],
                     start=position, end=position, focus=focus, reveal=reveal)
    if select:
        arguments.update(anchor=position, head=position)
    app.request("app.navigate_editor", **arguments)
    app.request("wait_frames", frames=2)


def editor_context_menu(app, panel):
    state = app.request("app.inspect_filetree")
    preview = next(p for p in state["previews"] if str(p["entity"]) == str(panel))
    rect = preview["editor_rect"]
    app.input({"type": "click", "position": [rect["min"][0] + 30, rect["min"][1] + 10], "button": "Right"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/editor-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []
    with tempfile.TemporaryDirectory(prefix="project-", dir=output) as directory:
        root = Path(directory)
        small = root / "small.txt"
        small.write_text("hello café\nsecond line\n", encoding="utf-8")
        large = root / "large.txt"
        line = b"x" * 111 + b"\n"
        with large.open("wb") as file:
            chunk = line * 10000
            for _ in range(100):
                file.write(chunk)
        large_size = large.stat().st_size
        with Application([str(Path(args.binary).resolve()), str(root)], output=output, timeout=90) as app:
            prepare_demo_layout(app, viewport=True)
            app.request("configure", fixed_dt=1 / 60, clipboard="")
            initial = app.wait_until(lambda state: bool(state["workbench"]["workspaces"]))
            workspace = initial["workbench"]["workspaces"][0]["entity"]
            open_panel(app, "Files")
            wait(app, lambda state: any(any(row["path"] == str(small) for row in tree["rows"]) for tree in state["panels"]))
            app.click_text("small.txt")
            state = wait(app, lambda state: any(p["path"] == str(small) and p["document"] for p in state["previews"]))
            first = next(p["entity"] for p in state["previews"] if p["path"] == str(small))
            navigate(app, first, 0)
            primary(app, "KeyA")
            app.input({"type": "text", "text": "edited café"})
            expected = "edited café"
            state = wait(app, lambda state: viewer(state, first)["content"] == expected)
            assert viewer(state, first)["pinned"] and viewer(state, first)["document_info"]["dirty"]
            assert small.read_text() == "hello café\nsecond line\n"
            editor_context_menu(app, first)
            app.click_text("Split view")
            state = wait(app, lambda state: len([p for p in state["previews"] if p["path"] == str(small)]) == 2)
            second = next(p["entity"] for p in state["previews"] if p["path"] == str(small) and p["entity"] != first)
            assert viewer(state, first)["document"] == viewer(state, second)["document"]
            navigate(app, second, 0)
            app.input({"type": "text", "text": "🙂"})
            expected = "🙂" + expected
            state = wait(app, lambda state: all(p["content"] == expected for p in state["previews"] if p["path"] == str(small)))
            primary(app, "KeyZ")
            wait(app, lambda state: viewer(state, second)["content"] == "edited café")
            primary(app, "KeyZ", shift=True)
            wait(app, lambda state: viewer(state, second)["content"] == expected)
            primary(app, "KeyS")
            wait(app, lambda state: not viewer(state, second)["document_info"]["dirty"] and not viewer(state, second)["document_info"]["saving"])
            assert small.read_text() == expected
            editor_context_menu(app, second)
            app.click_text("Line numbers")
            app.input({"type": "key", "code": "Escape", "pressed": True}, {"type": "key", "code": "Escape", "pressed": False})
            state = inspect(app)
            assert sum(not p["editor"]["line_numbers"] for p in state["previews"] if p["path"] == str(small)) == 1
            # Find owns its input, replacements share history, and wrapping is
            # view-local. Exercise the real controls rather than mutating state.
            navigate(app, second, 0)
            primary(app, "KeyF")
            app.input({"type": "text", "text": "café"})
            wait(app, lambda state: viewer(state, second)["editor"]["search_matches"] == 1,seconds=10)
            app.click_text("Replace", index=0)
            app.click_text("Replacement")
            app.input({"type": "text", "text": "mundo"})
            app.click_text("Replace next")
            expected = "🙂edited mundo"
            wait(app, lambda state: viewer(state, second)["content"] == expected)
            app.click_text("Close find")
            primary(app, "KeyZ")
            wait(app, lambda state: viewer(state, second)["content"] == "🙂edited café")
            primary(app, "KeyZ", shift=True)
            wait(app, lambda state: viewer(state, second)["content"] == expected)

            # Replace all is one canonical undo step, including both viewers.
            primary(app, "KeyF")
            primary(app, "KeyA")
            app.input({"type": "text", "text": "o"})
            wait(app, lambda state: viewer(state, second)["editor"]["search_matches"] == 1,seconds=10)
            app.click_text("Replace")
            app.click_text("mundo")
            primary(app, "KeyA")
            app.input({"type": "text", "text": "OO"})
            app.click_text("Replace all")
            expected = "🙂edited mundOO"
            wait(app, lambda state: all(p["content"] == expected for p in state["previews"] if p["path"] == str(small)))
            captures.append(app.capture("02-find-replace.png"))
            app.click_text("Close find")
            primary(app, "KeyZ")
            wait(app, lambda state: viewer(state, second)["content"] == "🙂edited mundo")
            primary(app, "KeyZ", shift=True)
            wait(app, lambda state: viewer(state, second)["content"] == expected)
            primary(app, "KeyS")
            wait(app, lambda state: not viewer(state, second)["document_info"]["dirty"])
            assert small.read_text() == expected

            # Smart newline, tab-stop backspace and block indentation use the
            # same canonical editing path as ordinary typing.
            navigate(app, second, 0)
            primary(app, "KeyA")
            app.input({"type": "text", "text": "    {}"},
                      {"type": "key", "code": "ArrowLeft", "pressed": True},
                      {"type": "key", "code": "ArrowLeft", "pressed": False},
                      {"type": "key", "code": "Enter", "pressed": True},
                      {"type": "key", "code": "Enter", "pressed": False})
            block = "    {\n        \n    }"
            wait(app, lambda state: viewer(state, second)["content"] == block)
            app.input({"type": "key", "code": "Backspace", "pressed": True},
                      {"type": "key", "code": "Backspace", "pressed": False})
            block = "    {\n    \n    }"
            wait(app, lambda state: viewer(state, second)["content"] == block)
            primary(app, "KeyA")
            app.input({"type": "key", "code": "Tab", "pressed": True},
                      {"type": "key", "code": "Tab", "pressed": False})
            wait(app, lambda state: viewer(state, second)["content"] == "        {\n        \n        }")
            primary(app, "KeyZ")
            wait(app, lambda state: viewer(state, second)["content"] == block)
            primary(app, "KeyA")
            app.input({"type": "text", "text": expected})
            primary(app, "KeyS")
            wait(app, lambda state: not viewer(state, second)["document_info"]["dirty"])
            assert small.read_text() == expected

            # The first editor's wrap setting does not change its sibling.
            editor_context_menu(app, first)
            app.click_text("Editing")
            app.click_text("Wrap lines")
            app.input({"type": "key", "code": "Escape", "pressed": True},
                      {"type": "key", "code": "Escape", "pressed": False})
            wait(app, lambda state: viewer(state, first)["editor"]["wrapping_ready"])
            assert not viewer(inspect(app), second)["editor"]["wrap"]
            captures.append(app.capture("01-shared-editors.png"))

            app.click_text("large.txt")
            state = wait(app, lambda state: any(p["path"] == str(large) and p["document"] for p in state["previews"]))
            big = next(p["entity"] for p in state["previews"] if p["path"] == str(large))
            current = viewer(state, big)
            assert current["byte_len"] == large_size and current["line_count"] == 1000001
            assert current["content_truncated"]
            before = (current["editor"]["anchor"], current["editor"]["head"])
            target = len(line) * 999999
            navigate(app, big, target, select=False, focus=False, reveal="center")
            app.request("wait_frames", frames=45)
            current = viewer(inspect(app), big)
            assert (current["editor"]["anchor"], current["editor"]["head"]) == before
            assert current["editor"]["scroll"][1] > 1000000
            navigate(app, big, target, reveal="center")
            app.request("wait_frames", frames=10)
            app.input({"type": "text", "text": "!"})
            state = wait(app, lambda state: viewer(state, big)["byte_len"] == large_size + 1)
            current = viewer(state, big)
            assert current["rendered_lines"] < 100 and current["laid_out_bytes"] < 32768
            primary(app, "KeyS")
            wait(app, lambda state: not viewer(state, big)["document_info"]["dirty"] and not viewer(state, big)["document_info"]["saving"])
            assert large.stat().st_size == large_size + 1
            with large.open("rb") as file:
                file.seek(target)
                assert file.read(1) == b"!"
            primary(app, "KeyZ")
            wait(app, lambda state: viewer(state, big)["byte_len"] == large_size and viewer(state, big)["document_info"]["dirty"])
            primary(app, "KeyZ", shift=True)
            state = wait(app, lambda state: viewer(state, big)["byte_len"] == large_size + 1 and not viewer(state, big)["document_info"]["dirty"])
            current = viewer(state, big)
            captures.append(app.capture("03-million-line-editor.png"))
            app.request("workspace.close", workspace=workspace)
            summary = {"passed": True, "large_file_bytes": large_size, "line_count": 1000001,
                       "rendered_lines": current["rendered_lines"], "laid_out_bytes": current["laid_out_bytes"],
                       "captures": captures}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"Editor smoke passed: {large_size} bytes, {current['rendered_lines']} rendered lines, {current['laid_out_bytes']} laid-out bytes")


if __name__ == "__main__":
    main()
