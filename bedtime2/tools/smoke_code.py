#!/usr/bin/env python3
"""Exercise syntax, gutter folding and Structure against the real bed2 UI."""
import argparse
import json
import os
import re
import tempfile
from pathlib import Path
from automation import Application
from smoke_bed2 import open_panel, prepare_demo_layout


def wait_code(app, predicate, frames=300):
    for _ in range(frames):
        state = app.request("app.inspect_code")
        if predicate(state):
            return state
        app.request("wait_frames", frames=1)
    raise AssertionError("Code condition did not settle")


def code(state, panel):
    return next(p for p in state["panels"] if str(p["panel"]) == str(panel))


def tracked(state):
    return next((str(s["source"]) for s in state["sources"] if s["source"] is not None), None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/bed2-code-smoke")
    args = parser.parse_args()
    output = Path(args.output)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []
    with tempfile.TemporaryDirectory(prefix="bed2-code-project-") as directory:
        root=Path(directory).resolve()
        os.environ["XDG_CONFIG_HOME"] = str(root / "config")
        source=root/"cats.rs"
        source.write_text("struct Cat {\n    name: String,\n}\nimpl Cat {\n    fn greet(&self) -> String {\n        format!(\"Hello, {}!\", self.name)\n    }\n}\nfn main() {\n    println!(\"hi\");\n}\n")
        (root/"dogs.py").write_text("def dog():\n    return 42\n")
        with Application([str(Path(args.binary).resolve()),str(root)], output=output) as app:
            prepare_demo_layout(app, viewport=True)
            app.request("configure", fixed_dt=1 / 60, clipboard="")
            snapshot = app.inspect()
            workspace = next(w for w in snapshot["workbench"]["workspaces"] if w["active"])
            layout = workspace["layouts"][0]
            area = layout["areas"][0]["entity"]
            cube = layout["areas"][0]["panels"][0]["entity"]
            tree = open_panel(app, "Files", area=area)
            for _ in range(300):
                trees=app.request("app.inspect_filetree")["panels"]
                if any(any(row["path"]==str(source) for row in t["rows"]) for t in trees): break
                app.request("wait_frames",frames=1)
            else: raise AssertionError("File tree did not load")
            app.click_text("cats.rs")
            state=wait_code(app,lambda s: any(p["highlights"] is not None for p in s["panels"]))
            a=state["panels"][0]["panel"]
            assert code(state,a)["language"]=="rust"
            app.request("panel.focus",panel=a)
            app.click_text("cats.rs")
            app.click_text("cats.rs")
            snapshot=app.inspect()
            area=next(ar["entity"] for w in snapshot["workbench"]["workspaces"] for l in w["layouts"] for ar in l["areas"] if any(p["entity"]==a for p in ar["panels"]))
            right=app.request("area.split",area=area,axis="horizontal",fraction=0.7)["area"]
            structure = open_panel(app, "Structure", area=right)
            app.request("panel.focus", panel=a)
            wait_code(app, lambda s: tracked(s) == str(a))
            captures.append(app.capture("01-syntax-structure.png"))
            # The first Structure branch collapses with the shared disclosure triangle.
            snapshot = app.inspect()
            structure_panel = next(p for w in snapshot["workbench"]["workspaces"] for l in w["layouts"] for ar in l["areas"] for p in ar["panels"] if str(p["entity"]) == str(structure))
            x, y = structure_panel["rect"]["min"]
            app.input({"type": "click", "position": [x + 16, y + 9]})
            app.request("wait_frames", frames=12)
            captures.append(app.capture("09-structure-collapsed.png"))
            app.input({"type": "click", "position": [x + 16, y + 9]})
            app.request("wait_frames", frames=12)
            app.click_text("name")
            wait_code(app, lambda s: code(s, a)["selection"][0] != code(s, a)["selection"][1])
            tool = app.request("app.inspect_titlebar")["tools"][0]["rect"]
            app.input({"type": "move", "position": [(tool["min"][0] + tool["max"][0]) / 2, (tool["min"][1] + tool["max"][1]) / 2]})
            app.request("wait_frames", frames=60)
            captures.append(app.capture("10-titlebar-hover-tooltip.png"))


            # Folding remains available through the gutter.
            view = next(p for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == a)
            rect, caret = view["editor_rect"], view["caret_rect"]
            app.input({"type": "click", "position": [rect["min"][0] - 7, rect["min"][1] + (caret["max"][1] - caret["min"][1]) / 2]})
            state = wait_code(app, lambda s: code(s, a)["collapsed"] > 0)
            assert code(state, a)["revision"] == 0
            captures.append(app.capture("02-folded.png"))
            app.click_text("greet")
            state = wait_code(app, lambda s: code(s, a)["selection"][0] != code(s, a)["selection"][1])
            assert code(state, a)["collapsed"] < code(state, a)["folds"]
            captures.append(app.capture("03-structure-navigation.png"))

            # The retained source can be hidden behind an unrelated tab.
            app.request("panel.move",panel=cube,area=area,index=0)
            app.request("panel.focus", panel=cube)
            app.click_text("main")
            app.wait_until(lambda s: any(p["entity"] == a and p["focused"]
                                        for w in s["workbench"]["workspaces"]
                                        for l in w["layouts"] for ar in l["areas"] for p in ar["panels"]))
            captures.append(app.capture("06-hidden-source-revealed.png"))

            # Automatic tab focus after closure is eligible to establish a source.
            app.request("panel.focus",panel=tree)
            app.click_text("dogs.py")
            state=wait_code(app,lambda s: any(p["language"]=="python" for p in s["panels"]))
            b=next(p["panel"] for p in state["panels"] if p["language"]=="python")
            app.request("panel.focus",panel=b)
            wait_code(app,lambda s:tracked(s)==str(b))
            app.request("panel.close", panel=b)
            wait_code(app, lambda s: tracked(s) == str(a))
            captures.append(app.capture("07-automatic-source.png"))
            # Gutter controls remain interactive without line-number labels.
            app.request("app.set_settings", values={"line_numbers": False})
            state = app.request("app.inspect_code")
            before = code(state, a)["collapsed"]
            assert before > 0
            captures.append(app.capture("08-fold-gutter-without-numbers.png"))
            preview = next(p for p in app.request("app.inspect_filetree")["previews"] if str(p["entity"]) == str(a))
            rect, caret = preview["editor_rect"], preview["caret_rect"]
            app.input({"type": "click", "position": [rect["min"][0] - 7, rect["min"][1] + 6 + (caret["max"][1] - caret["min"][1]) / 2]})
            wait_code(app, lambda s: code(s, a)["collapsed"] == before - 1)
            assert all(surface["diagnostics"]["error"] is None for surface in app.inspect()["ui"]["surfaces"])
    log = re.sub(r"\x1b\[[0-9;]*m", "", (output / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"bed2 code/Structure smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
