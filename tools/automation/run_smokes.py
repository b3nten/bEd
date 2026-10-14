#!/usr/bin/env python3
"""Observable application tests through the same public client agents import."""
import argparse
from pathlib import Path
import sqlite3
import tempfile
import time
from bed import Bed
from bedtime import Client, Panel, AutomationError


def wait_state(app, predicate, timeout=30):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        state=app.call("bedside.state")
        result=predicate(state)
        if result:
            return result
        time.sleep(0.02)
    raise TimeoutError(f"Bedside state did not settle: {state}")


def bed_scenario(binary, artifacts):
    with tempfile.TemporaryDirectory(prefix="bed-script-") as temporary:
        path=Path(temporary)/"example.txt"
        path.write_text("original\n")
        with Bed.launch(binary, log_path=artifacts/"bed.log") as app:
            assert any(e["name"]=="bed.open_file" for e in app.describe()["extensions"])
            panel=app.open_file(path)
            app.wait_ready(panel)
            app.focus(panel)
            app.type_text("typed ")
            typed=app.read_document(panel)
            assert b"typed " in typed["bytes"], typed
            app.undo(panel)
            assert app.read_document(panel)["bytes"]==b"original\n"
            app.redo(panel)
            assert app.read_document(panel)["bytes"]==typed["bytes"]
            app.save(panel)
            assert not app.read_document(panel)["info"]["dirty"]
            assert path.read_bytes()==typed["bytes"]
            revision=app.read_document(panel)["info"]["revision"]
            app.apply_edits(panel,revision,[(0,0,"script ")])
            try:
                app.apply_edits(panel,revision,[(0,0,"stale ")])
            except AutomationError:
                pass
            else:
                raise AssertionError("Stale edits were accepted")
            assert app.read_document(panel)["bytes"].startswith(b"script ")
            app.save(panel)
            with app.submit("bedtime.settings.set",owner="bedtime.ui",key="fontSize",value=19) as change:
                change.wait()
                capture=app.capture(panel,after=change)
                assert capture.frame["frame"]>0 and capture.frame["generation"]>0
                capture.save(artifacts/"bed-editor.png")
            second=app.split(panel)
            app.wait_ready(second)
            assert app.read_document(second)["bytes"]==app.read_document(panel)["bytes"]
            app.close_panel(second)
            app.set_setting("bedtime.ui","fontSize",20)
            original=app.settings()["settings"]
            changes=[("bedtime.ui","font","Paper Mono"),("bedtime.ui","fontSize",24),
                     ("bed.application","theme","solarized-light"),("bedtime.editor","treesitter",True)]
            for owner,key,value in changes:app.set_setting(owner,key,value)
            app.capture().save(artifacts/"bed-appearance.png")
            for owner,key,_ in changes:app.set_setting(owner,key,original[owner][key])
            assert app.settings()["settings"]==original
            app.select_layout(1)
            assert app.state()["layout"]=="1"
            app.capture().save(artifacts/"bed-window.png")
            app.quit()
    print("bEd external automation PASS")


def seed(root):
    paths=[]
    for name,count in [("first",625),("second",3)]:
        path=root/f"{name}.sqlite"
        with sqlite3.connect(path) as db:
            db.executescript("CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT); CREATE TABLE items(id INTEGER PRIMARY KEY,category INTEGER REFERENCES categories(id),name TEXT,note TEXT,payload BLOB); CREATE VIEW names AS SELECT id,name FROM items; INSERT INTO categories VALUES(1,'Things');")
            db.executemany("INSERT INTO items VALUES(?,1,?,NULL,?)",[(n,f"Item {n} · café",b"\0\1\xff") for n in range(1,count+1)])
        paths.append(path)
    return paths


def bedside_scenario(binary,artifacts):
    sql="SELECT count(*) FROM items"
    with tempfile.TemporaryDirectory(prefix="bedside-script-") as temporary:
        root=Path(temporary);config=root/"config";paths=seed(root)
        with Client.launch(binary,args=paths,config_dir=config,log_path=artifacts/"bedside-initial.log") as app:
            connections=wait_state(app,lambda s:s["connections"] if len(s["connections"])==2 and all(c["ready"] for c in s["connections"]) else False)
            for connection,command in [(connections[0],"bedside.open-data"),(connections[0],"bedside.open-schema"),(connections[1],"bedside.open-data")]:
                app.invoke(command,arguments={"connection":connection["id"],"table":"items"})
            app.invoke("bedside.new-query",arguments={"connection":connections[0]["id"]})
            query=wait_state(app,lambda s:next((q for q in s["queries"] if q["input_center"]),False))
            panel=Panel(query["panel"]);app.focus(panel);app.click(*query["input_center"])
            wait_state(app,lambda s:s["queries"][0]["input_active"])
            app.type_text(sql)
            wait_state(app,lambda s:s["queries"][0]["sql"]==sql and s["queries"][0]["dirty"])
            app.shortcut("KeyZ",character="z",primary=True)
            wait_state(app,lambda s:s["queries"][0]["sql"]=="")
            app.shortcut("KeyZ",character="z",primary=True,shift=True)
            wait_state(app,lambda s:s["queries"][0]["sql"]==sql)
            app.shortcut("Enter",named="Enter",primary=True)
            wait_state(app,lambda s:s["queries"][0]["rows"]==[["625"]])
            app.shortcut("KeyS",character="s",primary=True)
            wait_state(app,lambda s:not s["queries"][0]["dirty"] and s["saved_queries"][0]["sql"]==sql and all(d["ready"] for d in s["data_panels"]))
            app.capture().save(artifacts/"bedside-initial.png")
            app.quit()
        with Client.launch(binary,config_dir=config,log_path=artifacts/"bedside-restart.log") as app:
            state=wait_state(app,lambda s:s if len(s["queries"])==1 and len(s["data_panels"])==2 and all(d["ready"] for d in s["data_panels"]) else False)
            assert state["queries"][0]["sql"]==sql
            assert not state["queries"][0]["running"] and state["queries"][0]["rows"] is None
            app.capture().save(artifacts/"bedside-restart.png")
            app.quit()
    print("Bedside external initial + restart PASS")


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bed-binary",type=Path,default=Path("target/debug/bed"))
    parser.add_argument("--bedside-binary",type=Path,default=Path("target/debug/bedside-table"))
    parser.add_argument("--artifacts",type=Path,default=Path("target/automation-captures"))
    parser.add_argument("--scenario",choices=["all","bed","bedside"],default="all")
    args=parser.parse_args();args.artifacts.mkdir(parents=True,exist_ok=True)
    if args.scenario in ("all","bed"):bed_scenario(args.bed_binary,args.artifacts)
    if args.scenario in ("all","bedside"):bedside_scenario(args.bedside_binary,args.artifacts)


if __name__=="__main__":main()
