#!/usr/bin/env python3
"""Native command-click, symbol hover, completion/resolve/snippets and undo."""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from automation import Application
from smoke_bed import open_panel, prepare_demo_layout
from smoke_editor import navigate, primary, PRIMARY
from smoke_lsp import wait, wait_servers
from smoke_workspace_policy import clean_quit


def capture(app, name):
    for _ in range(3):
        value=app.capture(name)
        if value["nontransparent_pixels"]:
            return value
        app.request("wait_frames",frames=4)
    raise AssertionError(f"Capture remained empty: {name}")


def key(app, code):
    app.input({"type":"key","code":code,"pressed":True}, {"type":"key","code":code,"pressed":False})


def interaction(app, panel):
    return next((p for p in app.request("app.inspect_lsp_interaction")["panels"] if p["panel"] == panel), {})


def until(app, predicate, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        app.request("wait_frames", frames=2)
    debug = {op: app.request(op) for op in ["app.inspect_lsp", "app.inspect_lsp_servers", "app.inspect_filetree", "app.inspect_lsp_interaction"]}
    (app.output/"failed-state.json").write_text(json.dumps(debug,indent=2))
    app.capture("failed.png")
    raise AssertionError(f"Interaction condition did not settle; see {app.output}/failed-state.json")


def preview(app, panel):
    return next(p for p in app.request("app.inspect_filetree")["previews"] if p["entity"] == panel)


def point_at_caret(app, panel):
    caret = preview(app,panel)["caret_rect"]
    return [caret["min"][0]+2, (caret["min"][1]+caret["max"][1])/2]


def real_rust(binary, output):
    captures = []
    with tempfile.TemporaryDirectory(prefix="bed-real-rust-") as temporary:
        root=Path(temporary).resolve(); project=root/"project"; project.mkdir()
        os.environ["XDG_CONFIG_HOME"]=str(root/"config")
        (project/"Cargo.toml").write_text('[package]\nname="bed_hover_fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n[[bin]]\nname="main"\npath="main.rs"\n')
        source=project/"main.rs"
        text="fn main() { let _values: Vec<u8> = Vec::new(); }\n"
        source.write_text(text)
        config={"program":"rust-analyzer","arguments":[],"environment":{"CARGO_NET_OFFLINE":"true"},"settings":{"rust-analyzer":{"checkOnSave":False}},"initialization_options":None}
        with Application([str(binary),str(project)],output=output/"real-rust",timeout=60) as app:
            prepare_demo_layout(app,viewport=True)
            app.request("app.set_settings",values={"autosave":"Never","language_servers":{"rust":config}})
            app.request("configure",fixed_dt=1/60,clipboard="")
            open_panel(app,"Files")
            until(app,lambda:any(any(r["path"]==str(source) for r in p["rows"]) for p in app.request("app.inspect_filetree")["panels"]))
            app.click_text("main.rs")
            wait(app,lambda value:any(d["status"]=="Some(Ready)" for d in value["documents"]))
            panel=next(p["entity"] for p in app.request("app.inspect_filetree")["previews"] if p["path"]==str(source))
            for symbol,position in [("Vec",text.rfind("Vec::")),("new",text.rfind("new()"))]:
                navigate(app,panel,position)
                pointer=point_at_caret(app,panel)
                app.input({"type":"move","position":pointer})
                until(app,lambda: symbol in (interaction(app,panel).get("hover") or ""),seconds=45)
                until(app,lambda:interaction(app,panel).get("syntax_highlights",0)>0)
                captures.append(capture(app,f"05-real-{symbol}-hover.png"))
                app.input({"type":"move","position":[10,10]})
            navigate(app,panel,text.rfind("new()"))
            pointer=point_at_caret(app,panel)
            navigate(app,panel,0)
            app.input({"type":"key","code":PRIMARY,"pressed":True},{"type":"click","position":pointer},{"type":"key","code":PRIMARY,"pressed":False})
            opened=until(app,lambda: next((p for p in app.request("app.inspect_filetree")["previews"] if "rustlib/src/rust/library/alloc/src/vec/mod.rs" in (p["path"] or "") and p["document"]),None),seconds=45)
            assert not opened["pinned"],opened
            selection=until(app,lambda:next(p["selection"] for p in app.request("app.inspect_code")["panels"] if p["panel"]==opened["entity"]) != [0,0] and next(p["selection"] for p in app.request("app.inspect_code")["panels"] if p["panel"]==opened["entity"]))
            assert Path(opened["path"]).read_bytes()[selection[0]:selection[1]]==b"new", selection
            def target_visible():
                view=preview(app,opened["entity"])
                caret, canvas=view["caret_rect"],view["editor_rect"]
                return canvas["min"][1] <= caret["min"][1] and caret["max"][1] <= canvas["max"][1]
            until(app,target_visible)
            captures.append(capture(app,"06-real-new-definition.png"))
            clean_quit(app)
    return captures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/lsp-interaction-smoke")
    parser.add_argument("--real-rust",action="store_true",help="Also check standard-library Vec hover and definition with rust-analyzer")
    args = parser.parse_args()
    output = Path(args.output).resolve(); output.mkdir(parents=True,exist_ok=True)
    captures = []
    previous = os.environ.get("XDG_CONFIG_HOME")
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-",dir=output) as temporary:
            root = Path(temporary); project = root/"project"; project.mkdir()
            os.environ["XDG_CONFIG_HOME"] = str(root/"config")
            source = project/"main.rs"; target = project/"target.rs"
            initial = "// 🙂 Unicode\nfn first() {}\nfn caller() { first(); }\n"
            source.write_text(initial); target.write_text("fn destination() {}\n")
            transcript = root/"server.jsonl"
            server = Path(__file__).resolve().parents[1]/"crates/bedtime-lsp/tests/server.py"
            config = {"program":sys.executable,"arguments":[str(server),"cross-file",str(transcript)],"environment":{},"settings":{},"initialization_options":None}
            with Application([str(Path(args.binary).resolve()),str(project)],output=output,timeout=60) as app:
                prepare_demo_layout(app,viewport=True)
                app.request("app.set_settings",values={"autosave":"Never","language_servers":{"rust":config}})
                app.request("configure",fixed_dt=1/60,clipboard="")
                open_panel(app,"Files")
                until(app,lambda:any(any(r["path"]==str(source) for r in p["rows"]) for p in app.request("app.inspect_filetree")["panels"]))
                app.click_text("main.rs")
                wait(app,lambda value:any(d["status"]=="Some(Ready)" for d in value["documents"]))
                wait_servers(app,lambda value:any(s.get("definition") for s in value["servers"]))
                panel = next(p["entity"] for p in app.request("app.inspect_filetree")["previews"] if p["path"]==str(source))
                at = initial.encode().find(b"first();")
                navigate(app,panel,at)
                position = point_at_caret(app,panel)
                app.input({"type":"move","position":position})
                app.request("wait_frames",frames=3)
                assert not interaction(app,panel).get("hover"), "LSP hover should wait 200 ms"
                until(app,lambda:interaction(app,panel).get("hover"))
                until(app,lambda:interaction(app,panel).get("syntax_highlights",0)>0)
                captures.append(capture(app,"01-symbol-hover.png"))
                hover_rect = interaction(app,panel)["hover_rect"]
                app.input({"type":"move","position":[hover_rect["min"][0]+20,hover_rect["min"][1]+20]})
                assert interaction(app,panel)["hover"], "Hover must remain readable under the pointer"
                key(app,"Escape")
                assert not interaction(app,panel)["hover"], "Escape must dismiss hover"
                # The caret stays elsewhere: navigation must use the pointer position.
                navigate(app,panel,0)
                app.input({"type":"move","position":position},
                          {"type":"key","code":PRIMARY,"pressed":True},
                          {"type":"click","position":position},
                          {"type":"key","code":PRIMARY,"pressed":False})
                opened = until(app,lambda:next((p for p in app.request("app.inspect_filetree")["previews"] if p["path"]==str(target) and p["document"]),None))
                assert not opened["pinned"]
                records = [json.loads(line) for line in transcript.read_text().splitlines()]
                definition = next(r for r in records if r.get("method")=="textDocument/definition")
                assert definition["params"]["position"]["line"]==2, definition
                captures.append(capture(app,"02-command-definition.png"))
                app.click_text("main.rs")
                panel = until(app,lambda:next((p["entity"] for p in app.request("app.inspect_filetree")["previews"] if p["path"]==str(source) and p["document"]),None))
                wait(app,lambda value:any(d["status"]=="Some(Ready)" and d["uri"].endswith("main.rs") for d in value["documents"]))
                navigate(app,panel,at)
                # Move away from symbols so the hover cannot obscure completion.
                app.input({"type":"move","position":[10,10]}, {"type":"text","text":"fi"})
                until(app,lambda:(interaction(app,panel).get("completion") or {}).get("items"))
                until(app,lambda:any(i["documentation"] for i in (interaction(app,panel).get("completion") or {}).get("items") or []))
                until(app,lambda:interaction(app,panel).get("documentation_rect"))
                geometry = interaction(app,panel)
                menu, docs = geometry["completion_rect"], geometry["documentation_rect"]
                assert menu["max"][0] <= docs["min"][0] or docs["max"][0] <= menu["min"][0] or menu["max"][1] <= docs["min"][1] or docs["max"][1] <= menu["min"][1], geometry
                captures.append(capture(app,"03-completion.png"))
                key(app,"Enter")
                until(app,lambda:preview(app,panel)["document_info"]["revision"]>=2)
                selection = next(p["selection"] for p in app.request("app.inspect_code")["panels"] if p["panel"]==panel)
                assert selection[1]-selection[0]==5, selection # value placeholder
                app.input({"type":"text","text":"hello"})
                key(app,"Tab")
                primary(app,"KeyS")
                until(app,lambda:source.read_text().startswith("// auto import\n"))
                assert "first(hello)first();" in source.read_text(),source.read_text()
                captures.append(capture(app,"04-snippet.png"))
                # Undo placeholder typing and completion independently; the import
                # and function insertion disappear together on the second undo.
                primary(app,"KeyZ"); primary(app,"KeyZ"); primary(app,"KeyS")
                expected = initial[:initial.index("first();")]+"fi"+initial[initial.index("first();"):]
                until(app,lambda:source.read_text()==expected)
                app.input({"type":"key","code":"ControlLeft","pressed":True},
                          {"type":"key","code":"Space","pressed":True},
                          {"type":"key","code":"Space","pressed":False},
                          {"type":"key","code":"ControlLeft","pressed":False})
                until(app,lambda:(interaction(app,panel).get("completion") or {}).get("items"))
                key(app,"Escape")
                assert not interaction(app,panel).get("completion")
                clean_quit(app)
            (output/"server.jsonl").write_text(transcript.read_text())
        if args.real_rust:
            captures.extend(real_rust(Path(args.binary).resolve(),output))
    finally:
        if previous is None: os.environ.pop("XDG_CONFIG_HOME",None)
        else: os.environ["XDG_CONFIG_HOME"]=previous
    summary={"passed":True,"captures":captures}
    (output/"summary.json").write_text(json.dumps(summary,indent=2)); print(json.dumps(summary))

if __name__=="__main__":
    main()
