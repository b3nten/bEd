#!/usr/bin/env python3
"""Exercise real Git panel input, partial staging, commits, discard and restore."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import statistics
import tempfile
import time

from automation import Application
from smoke_bed2 import open_panel, prepare_demo_layout
from smoke_workspace_policy import clean_quit


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.STDOUT, text=True)


def settled(app, predicate=lambda value: True):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        value = app.request("app.inspect_git")
        if value["sessions"]:
            session = value["sessions"][0]
            if session["status"] is not None and not session["busy"] and not session["refreshing"] and not session.get("projecting", False) and predicate(session):
                return value
        app.request("wait_frames", frames=2)
    raise AssertionError(f"Git did not settle: {[(s['busy'], s['error'], s['draft'], s['status']['entries'][:3]) for s in value['sessions']]}")


def presentation_settled(app):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        value = app.request("app.inspect_git")
        if all(not p["tree"]["animating"] for p in value["panels"]):
            return value
        app.request("wait_frames", frames=2)
    raise AssertionError("Git tree animation did not settle")


def control(app, name, *, button="Left"):
    value = presentation_settled(app)
    rect = next(panel["controls"][name] for panel in value["panels"] if name in panel["controls"])
    zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
    app.input({"type": "click", "button": button,
               "position": [(a + b) * zoom / 2 for a, b in zip(rect["min"], rect["max"])]})



def option(app, label, view, grouping):
    control(app, "git-settings")
    app.request("wait_frames", frames=3)
    app.click_text(label)
    app.input({"type":"key","code":"Escape","pressed":True},
              {"type":"key","code":"Escape","pressed":False})
    settled(app, lambda session: session["presented_options"] == {"view":view,"group_by":grouping})
    app.request("wait_frames", frames=2)
    return presentation_settled(app)


def commit_mode(app, label):
    control(app, "commit-mode")
    app.request("wait_frames", frames=3)
    app.click_text(label)
    app.request("wait_frames", frames=3)
    state = app.request("app.inspect_git")
    assert state["sessions"][0]["commit_mode"] == label
    return state


def centered_commit_label(app, label):
    state = presentation_settled(app)
    button = state["panels"][0]["controls"]["commit-submit"]
    text = next(t["rect"] for t in app.inspect()["ui"]["texts"] if t["text"] == label)
    assert abs(sum(text[edge][0] for edge in ["min", "max"]) / 2
               - sum(button[edge][0] for edge in ["min", "max"]) / 2) < 1.0, (label, button, text)


def entry(session, path):
    return next((e for e in session["status"]["entries"] if e["path"] == path), None)


def capture_checked(app, filename):
    # Move away from controls and observe actual render progress before capture.
    # Native OS focus can change independently while automation is running.
    app.input({"type": "move", "position": [4, 4]})
    render_frame = app.inspect()["render"]["frames"]
    app.wait_until(lambda state: state["render"]["frames"] >= render_frame + 2
                   and state["render"]["pending_pipelines"] == 0
                   and bool(state["ui"]["surfaces"])
                   and all(surface["diagnostics"]["indices"] > 0
                           and surface["diagnostics"]["error"] is None
                           for surface in state["ui"]["surfaces"]))
    app.request("wait_frames", frames=12)
    result = app.capture(filename)
    assert result["nontransparent_pixels"] > 1000, f"Rendered capture was blank: {filename}"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/git-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").unlink(missing_ok=True)
    previous = os.environ.get("XDG_CONFIG_HOME")
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="fixture-", dir=output) as temporary:
            fixture = Path(temporary)
            project = fixture / "project"
            project.mkdir()
            os.environ["XDG_CONFIG_HOME"] = str(fixture / "config")
            git(project, "init", "-b", "main")
            for key, value in [("user.name", "Bed Test"), ("user.email", "bed@example.invalid"), ("commit.gpgsign", "false"), ("core.hooksPath", ".git/hooks")]:
                git(project, "config", key, value)
            for name in ["a.txt", "b.txt"]:
                (project / name).write_text("original\n")
            git(project, "add", "a.txt", "b.txt")
            git(project, "commit", "-m", "Initial")
            for name in ["a.txt", "b.txt"]:
                (project / name).write_text("working changes\n")
            (project / "new").mkdir()
            for index in range(500):
                (project / "new" / f"file-{index:04}.txt").write_text("new\n")
            command = [str(Path(args.binary).resolve()), str(project)]
            with Application(command, output=output, timeout=60) as app:
                prepare_demo_layout(app, viewport=False)
                files = open_panel(app, "Files")
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    file_tree = next(p for p in app.request("app.inspect_filetree")["panels"] if p["entity"] == files)
                    if file_tree["row_rects"] and not file_tree["loading"] and not file_tree["animating"]:
                        break
                    app.request("wait_frames", frames=2)
                assert file_tree["row_rects"], file_tree
                file_rect = file_tree["row_rects"][0]["rect"]
                files_row_height = file_rect["max"][1] - file_rect["min"][1]
                panel = open_panel(app, "Git")
                state = settled(app)
                assert state["sessions"][0]["presented_options"] == {"view":"tree","group_by":"tracked_untracked"}
                assert len(state["sessions"][0]["status"]["entries"]) == 502
                app.request("wait_frames", frames=4)
                state = presentation_settled(app)
                assert 0 < state["panels"][0]["rendered_rows"] < 60, state
                controls = state["panels"][0]["controls"]
                assert abs(controls["stage-group:Tracked"]["min"][0] - controls["stage-file:a.txt"]["min"][0]) < 1.0, controls
                samples = [app.request("app.inspect_git")["panels"][0]["draw_micros"] for _ in range(30)]
                idle_draw_median_us = statistics.median(samples)
                centered_commit_label(app, "Commit Staged")
                captures.append(capture_checked(app, "01-changes.png"))
                control(app, "stage-file:a.txt")
                settled(app, lambda s: entry(s, "a.txt")["index_status"] == "M")
                assert git(project, "show", ":a.txt") == "working changes\n"
                (project / "a.txt").write_text("partial second edit\n")
                control(app, "refresh-cw")
                settled(app, lambda s: entry(s, "a.txt")["worktree_status"] == "M")
                captures.append(capture_checked(app, "02-partial-staging.png"))
                control(app, "git-settings")
                captures.append(capture_checked(app, "07-settings-menu.png"))
                app.input({"type":"key","code":"Escape","pressed":True},
                          {"type":"key","code":"Escape","pressed":False})
                option(app,"List","list","tracked_untracked")
                state = option(app,"None","list","none")
                assert state["sessions"][0]["row_count"] == 502
                state = option(app,"Staged and unstaged","list","staged_unstaged")
                assert state["sessions"][0]["row_count"] == 505
                controls = state["panels"][0]["controls"]
                assert "stage-file:Staged:a.txt" in controls and "stage-file:Unstaged:a.txt" in controls
                # Side-specific toggles preserve the working version.
                control(app,"stage-file:Staged:a.txt")
                settled(app, lambda s: entry(s,"a.txt")["index_status"] == " ")
                assert git(project,"show",":a.txt") == "original\n"
                control(app,"stage-file:Unstaged:a.txt")
                settled(app,lambda s: entry(s,"a.txt")["index_status"] == "M" and entry(s,"a.txt")["worktree_status"] == " ")
                (project/"a.txt").write_text("partial third edit\n")
                control(app,"refresh-cw")
                settled(app,lambda s: entry(s,"a.txt")["worktree_status"] == "M")
                state = option(app,"Tree","tree","staged_unstaged")
                assert state["sessions"][0]["row_count"] == 506
                rect = state["panels"][0]["controls"]["file:Staged:a.txt"]
                assert abs(rect["max"][1] - rect["min"][1] - files_row_height) < 0.1
                controls = state["panels"][0]["controls"]
                assert abs(controls["stage-group:Staged"]["min"][0] - controls["stage-file:Staged:a.txt"]["min"][0]) < 1.0, controls
                assert state["panels"][0]["tree"]["motion_rows"] < 60
                control(app, "file:Staged:a.txt")
                captures.append(capture_checked(app,"08-staged-unstaged-tree.png"))
                control(app,"directory-toggle:Unstaged:new")
                state = settled(app,lambda s:s["row_count"]==6)
                app.request("wait_frames",frames=2)
                state = app.request("app.inspect_git")
                state = presentation_settled(app)
                assert state["panels"][0]["rendered_rows"] <= 6
                assert state["panels"][0]["tree"]["retiring_rows"] == 0
                control(app,"stage-directory:Unstaged:new")
                settled(app,lambda s:all(e["index_status"]=="A" for e in s["status"]["entries"] if e["path"].startswith("new/")))
                assert len(git(project,"ls-files","--","new").splitlines()) == 500
                control(app,"stage-directory:Staged:new")
                settled(app,lambda s:all(e["index_status"]=="?" for e in s["status"]["entries"] if e["path"].startswith("new/")))
                state = option(app,"None","tree","none")
                assert state["sessions"][0]["row_count"] == 503
                state = option(app,"Tracked and untracked","tree","tracked_untracked")
                assert state["sessions"][0]["row_count"] == 505
                captures.append(capture_checked(app,"09-tracked-untracked-tree.png"))
                option(app,"List","list","tracked_untracked")

                control(app, "stage-file:a.txt")
                settled(app, lambda s: entry(s, "a.txt")["worktree_status"] == " ")
                assert git(project, "show", ":a.txt") == "partial third edit\n"
                control(app, "stage-file:a.txt")
                settled(app, lambda s: entry(s, "a.txt")["index_status"] == " ")
                control(app, "stage-group:Tracked")
                settled(app, lambda s: all(entry(s, name)["index_status"] == "M" for name in ["a.txt", "b.txt"]))
                control(app, "commit-message")
                app.input({"type": "text", "text": "Panel smoke commit"})
                app.click_text("Commit Staged")
                settled(app, lambda s: not s["draft"] and entry(s, "a.txt") is None)
                assert git(project, "log", "-1", "--format=%s").strip() == "Panel smoke commit"
                captures.append(capture_checked(app, "03-committed.png"))
                # Amend works without staged files and replaces, rather than adds, history.
                commit_mode(app, "Amend")
                control(app, "commit-message")
                app.input({"type":"text", "text":"Panel amended"})
                centered_commit_label(app, "Amend Commit")
                captures.append(capture_checked(app, "10-amend.png"))
                before = git(project, "rev-parse", "HEAD").strip()
                control(app, "commit-submit")
                settled(app, lambda s: entry(s, "a.txt") is None and entry(s, "b.txt") is None and s["draft"] == "")
                assert git(project, "log", "-1", "--format=%s").strip() == "Panel amended"
                assert git(project, "rev-list", "--count", "HEAD").strip() == "2"
                assert git(project, "rev-parse", "HEAD").strip() != before
                commit_mode(app, "Signoff")
                (project / "a.txt").write_text("signoff edit\n")
                control(app, "refresh-cw")
                settled(app, lambda s: entry(s,"a.txt") is not None)
                control(app, "stage-file:a.txt")
                settled(app, lambda s: entry(s,"a.txt")["index_status"] == "M")
                control(app, "commit-message")
                app.input({"type":"text", "text":"Panel signoff"})
                centered_commit_label(app, "Commit with Signoff")
                captures.append(capture_checked(app, "11-signoff.png"))
                control(app, "commit-submit")
                settled(app, lambda s: entry(s, "a.txt") is None and entry(s, "b.txt") is None and s["draft"] == "")
                assert "Signed-off-by: Bed Test <bed@example.invalid>" in git(project,"log","-1","--format=%B")
                assert git(project, "rev-list", "--count", "HEAD").strip() == "3"
                commit_mode(app, "Commit")

                (project / "b.txt").write_text("discard these changes\n")
                control(app, "refresh-cw")
                settled(app, lambda s: entry(s, "b.txt") is not None)
                control(app, "file:b.txt", button="Right")
                app.click_text("Discard file changes…")
                settled(app, lambda s: s["confirmation"] is not None)
                captures.append(capture_checked(app, "04-discard-confirmation.png"))
                # A visible comparison also draws the floating confirmation.
                # Click its topmost button rather than the obscured inline copy.
                confirmations = [t for t in app.inspect()["ui"]["texts"] if t["text"] == "Discard changes"]
                app.click_text("Discard changes", index=len(confirmations)-1)
                settled(app, lambda s: entry(s, "b.txt") is None)
                assert (project / "b.txt").read_text() == "working changes\n"
                app.click_text("main")
                app.request("wait_frames", frames=3)
                control(app, "branch-input")
                app.request("wait_frames", frames=2)
                app.input({"type": "text", "text": "panel-branch"})
                app.capture("branch-menu.png")
                app.click_text("Create and switch")
                settled(app, lambda s: s["status"]["branch"] == "panel-branch")
                app.request("window.resize", size=[520, 720])
                app.request("wait_frames", frames=20)
                state = app.request("app.inspect_git")
                assert state["panels"][0]["rendered_rows"] < 60
                assert any(t["text"] == "Commit Staged" for t in app.inspect()["ui"]["texts"])
                option(app,"Tree","tree","tracked_untracked")
                option(app,"Staged and unstaged","tree","staged_unstaged")
                control(app, "commit-message")
                app.input({"type": "text", "text": "Preserve draft after restart"})
                captures.append(capture_checked(app, "05-narrow.png"))
                clean_quit(app)
            with Application(command, output=output / "restored", timeout=60) as app:
                app.request("wait_frames", frames=30)
                restored = settled(app)
                assert restored["sessions"][0]["draft"] == "Preserve draft after restart"
                assert restored["sessions"][0]["presented_options"] == {"view":"tree","group_by":"staged_unstaged"}
                assert restored["panels"], "Git placement was not restored"
                assert restored["sessions"][0]["status"]["branch"] == "panel-branch"
                captures.append(capture_checked(app, "06-restored.png"))
                clean_quit(app)
        (output / "summary.json").write_text(json.dumps({"captures": captures, "idle_draw_median_us": idle_draw_median_us, "idle_draw_view": "tree", "files_and_git_row_height": files_row_height, "checks": "Tree/tracked-untracked defaults, centered commit labels and Commit/Amend/Signoff actions, matching Files/Git row height, bounded viewport animation state, all six list/tree and grouping combinations, folder collapse/stage/unstage, side-specific partial staging, options restore, 502 changes, virtual rows, partial/group staging, commit, confirmed discard, branch, resize and restore"}, indent=2))
        print(json.dumps({"captures": len(captures), "output": str(output)}))
    finally:
        if previous is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous


if __name__ == "__main__":
    main()
