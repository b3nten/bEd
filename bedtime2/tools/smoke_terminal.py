#!/usr/bin/env python3
"""Exercise real local PTYs through bed2's Terminal picker and native input (Unix)."""
import argparse
import json
import os
import re
import shlex
import sys
import time
from pathlib import Path

from automation import Application, AutomationError


PANEL_KEY = "bedtime.terminal"


def inspection(app):
    return app.request("app.inspect_terminal")


def state(app, panel):
    value = next((p for p in inspection(app)["panels"] if p["entity"] == panel), None)
    if value is None:
        raise AutomationError(f"Terminal panel disappeared: {panel}")
    return value


def wait_state(app, panel, predicate, *, seconds=20):
    # PTYs and shell sleep use real time, not configure.fixed_dt.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = state(app, panel)
        assert value["error"] is None and value["status"] != "Failed", value
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AutomationError(f"Terminal did not reach expected state: {value}")


def focus(app, panel):
    app.request("panel.focus", panel=panel)
    value = wait_state(app, panel, lambda p: p["presentation"]["visible"])
    snapshot = app.inspect()
    zoom = snapshot["ui"]["surfaces"][0]["zoom"]
    rect = value["presentation"]["rect"]
    # Inside the padded canvas, away from its scrollbar and any status notice.
    position = [(a + (b - a) * t) * zoom
                for a, b, t in zip(rect["min"], rect["max"], [0.65, 0.75])]
    app.input({"type": "click", "position": position})
    assert state(app, panel)["presentation"]["focused"]


def enter_steps():
    return [{"type": "key", "code": "Enter", "pressed": True},
            {"type": "key", "code": "Enter", "pressed": False}]


def command(app, panel, text):
    focus(app, panel)
    app.input({"type": "text", "text": text}, *enter_steps())


def control(app, panel, key):
    focus(app, panel)
    app.input(
        {"type": "key", "code": "ControlLeft", "pressed": True},
        {"type": "key", "code": key, "pressed": True},
        {"type": "key", "code": key, "pressed": False},
        {"type": "key", "code": "ControlLeft", "pressed": False},
    )


def print_marker(marker):
    # Never put the complete expected output in an echoed command line.
    # Merely observing PTY echo must not satisfy an output assertion.
    middle = len(marker) // 2
    return "printf '\\n%s%s\\n' " + " ".join(
        shlex.quote(part) for part in (marker[:middle], marker[middle:]))


def bootstrap(app, panel, marker):
    wait_state(app, panel, lambda p: p["status"] == "Running" and p["pid"] is not None)
    # Select a portable shell through real input, not launch flags or app hooks.
    command(app, panel, "exec /bin/sh")
    command(app, panel, "PS1='bed2$ '; export PS1; printf '\\033[2J\\033[H'; "
            + print_marker(marker))
    return wait_state(app, panel, lambda p: marker in p["text"])


def jobs(value, workspace):
    return next((w["jobs"] for w in value["workspaces"] if w["entity"] == workspace), [])


def assert_owned(app, workspace, panels):
    owned = jobs(inspection(app), workspace)
    assert {j["owner"] for j in owned} == set(panels), owned
    assert len(owned) == len(panels) and all(j["status"] == "Running" for j in owned), owned


def tick(value):
    return max((int(n) for n in re.findall(r"(?:^|\n)BED2_TICK:(\d+)", value["text"])),
               default=-1)


def observe_hidden(app, panel, pid, *, inactive=False):
    # Keep the normal app updating for actual wall time while a shell emits output.
    # Hidden snapshots may be stale; assert the transcript only after showing it.
    deadline = time.monotonic() + 2.2
    while time.monotonic() < deadline:
        value = state(app, panel)
        assert value["status"] == "Running" and value["pid"] == pid, value
        assert value["error"] is None and not value["presentation"]["visible"], value
        if inactive:
            assert not value["active_workspace"] and not value["running_workspace"], value
        owned = jobs(inspection(app), value["workspace"])
        assert any(j["owner"] == panel and j["status"] == "Running" for j in owned), owned
        app.request("wait_frames", frames=6)


def process_exists(pid):
    # Signal zero is an existence probe only. The scenario never kills children;
    # their normal Terminal/WorkspaceWork owner must do all process cleanup.
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def wait_retired(app, workspace, panels, pids):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = inspection(app)
        remaining = [p["entity"] for p in value["panels"] if p["entity"] in panels]
        owned = [j for j in jobs(value, workspace) if j["owner"] in panels]
        alive = [pid for pid in pids if process_exists(pid)]
        if not remaining and not owned and not alive:
            return
        app.request("wait_frames", frames=3)
    raise AutomationError(f"Terminal cleanup incomplete: panels={remaining}, jobs={owned}, pids={alive}")


def foreground_child(app, panel, label):
    marker = "BED2_" + label
    inner = "printf '\\n%s%s:%s\\n' 'BED2_' " + shlex.quote(label) + ' "$$"; exec sleep 60'
    shell_pid = state(app, panel)["pid"]
    command(app, panel, "/bin/sh -c " + shlex.quote(inner))
    pattern = r"(?:^|\n)" + re.escape(marker) + r":(\d+)"
    value = wait_state(app, panel, lambda p: re.search(pattern, p["text"]) is not None)
    child_pid = int(re.search(pattern, value["text"]).group(1))
    assert child_pid != shell_pid and process_exists(child_pid), value
    return [shell_pid, child_pid]


def render_observation(snapshot):
    """Keep failure messages focused; the complete inspect result is in the transcript."""
    return {
        "windows": snapshot["windows"],
        "render": snapshot["render"],
        "ui_frame": snapshot["ui"]["frame"],
        "ui_surfaces": snapshot["ui"]["surfaces"],
        "ui_text_count": len(snapshot["ui"]["texts"]),
        "ui_text_sample": [t["text"] for t in snapshot["ui"]["texts"][:12]],
        "viewports": snapshot["viewports"],
        "workbench_surfaces": [
            {key: surface[key] for key in ("window", "panels_visible", "presentation")}
            for surface in snapshot["workbench"]["surfaces"]
        ],
    }


def capture_checked(app, filename, *, different_from=None):
    before = app.inspect()
    result = app.capture(filename)
    reason = None
    if result["nontransparent_pixels"] <= 0:
        reason = "full-window capture has no nontransparent pixels"
    elif different_from is not None:
        # A resize changes the hash even for an all-zero image. Compare known
        # content transitions at identical dimensions, never just global uniqueness.
        if (result["width"], result["height"]) != (different_from["width"], different_from["height"]):
            reason = "content comparison unexpectedly changed screenshot dimensions"
        elif result["hash"] == different_from["hash"]:
            reason = "screenshot did not change across a known UI/content transition"
    if reason is None:
        return result

    diagnostics = {"reason": reason, "capture": result, "comparison": different_from,
                   "before": render_observation(before)}
    control_layout = None
    previous_layout = next((layout["entity"]
                            for workspace in before["workbench"]["workspaces"] if workspace["active"]
                            for layout in workspace["layouts"] if layout["active"]), None)
    try:
        diagnostics["after"] = render_observation(app.inspect())
        panels = inspection(app)["panels"]
        diagnostics["terminals"] = [
            {"entity": p["entity"], "status": p["status"], "pid": p["pid"],
             "error": p["error"], "revision": p["revision"], "presentation": p["presentation"],
             "grid": [p["grid"]["cols"], p["grid"]["rows"]], "text_length": len(p["text"])}
            for p in panels
        ]
        if not panels:
            diagnostics["scope"] = "failure before any terminal panel was opened"
            visible = next((v for v in before["viewports"] if v["visible"]), None)
            if visible is not None:
                # Separate scene texture from window/UI compositing on startup.
                diagnostics["scene_capture"] = app.capture(
                    "diagnostic-scene.png", viewport=visible["entity"])
        else:
            # A differential observation, NOT a retry that can turn failure into
            # success. Even a successful control cannot distinguish a temporal
            # render fault from a content-dependent one without more evidence.
            # A separate layout hides ALL terminals, including a second split
            # panel; covering only the focused tab would not isolate terminal UI.
            control_layout = app.request("layout.create", name="Capture diagnostic", activate=True)["layout"]
            app.request("panel.open", key="bed.hello", layout=control_layout)
            control_snapshot = app.wait_until(
                lambda s: any(t["text"] == "Hello, world!" for t in s["ui"]["texts"])
                and not any(p["presentation"]["visible"] for p in inspection(app)["panels"]))
            diagnostics["nonterminal_control"] = {
                "observation": render_observation(control_snapshot),
                "capture": app.capture("diagnostic-hello-control.png"),
            }
            diagnostics["scope"] = "full-window failure; nonterminal control recorded for comparison"
    except AutomationError as error:
        diagnostics["diagnostic_error"] = str(error)
    finally:
        if control_layout is not None:
            try:
                app.request("layout.delete", layout=control_layout)
                if previous_layout is not None:
                    app.request("layout.activate", layout=previous_layout)
            except AutomationError as error:
                diagnostics["control_cleanup_error"] = str(error)
    print("Terminal render/capture failure: " + json.dumps(diagnostics, indent=2), file=sys.stderr)
    raise AutomationError(f"{filename}: {reason}; see capture diagnostics above and transcript.jsonl")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed2")
    parser.add_argument("--output", default="target/terminal-smoke")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("this native PTY scenario requires Unix and /bin/sh")
    output = Path(args.output)
    (output / "summary.json").unlink(missing_ok=True)
    captures = []

    with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        snapshot = app.wait_until(lambda s: bool(s["viewports"])
                                  and s["viewports"][0]["visible"]
                                  and s["render"]["pending_pipelines"] == 0)
        render_frame = snapshot["render"]["frames"]
        # Use the existing input path to focus the native window, then observe
        # actual render progress after UI input. This is a readiness boundary,
        # not repeated capture attempts until a blank image happens to disappear.
        app.input({"type": "click", "position": [300, 180]})
        snapshot = app.wait_until(lambda s: s["windows"][0]["focused"]
                                  and s["render"]["frames"] >= render_frame + 2
                                  and s["render"]["pending_pipelines"] == 0
                                  and bool(s["ui"]["surfaces"])
                                  and all(surface["diagnostics"]["indices"] > 0
                                          and surface["diagnostics"]["error"] is None
                                          for surface in s["ui"]["surfaces"]))
        # If this is transparent, no Terminal panel has been opened yet: isolate
        # a window/render/capture problem instead of blaming terminal output.
        captures.append(capture_checked(app, "00-workbench-control.png"))
        workspace = snapshot["workbench"]["workspaces"][0]
        workspace_id = workspace["entity"]
        layout = workspace["layouts"][0]["entity"]
        area = workspace["layouts"][0]["areas"][0]["entity"]
        app.request("panel.close", panel=snapshot["viewports"][0]["entity"])

        # The first terminal comes from the real empty-area context menu.
        app.input({"type": "click", "button": "Right", "position": [300, 180]})
        app.wait_until(lambda s: any(t["text"] == "Terminal" for t in s["ui"]["texts"]))
        app.click_text("Terminal")
        app.wait_until(lambda _: bool(inspection(app)["panels"]))
        first = inspection(app)["panels"][0]["entity"]
        initial = bootstrap(app, first, "BED2_READY")
        first_pid = initial["pid"]
        assert initial["grid"]["cols"] > 0 and initial["grid"]["rows"] > 0
        assert_owned(app, workspace_id, [first])
        wait_state(app, first, lambda p: p["tab_title"] not in ("", "Terminal"))
        texts = [t["text"] for t in app.inspect()["ui"]["texts"]]
        assert "Terminal" not in texts and "Stop" not in texts and "Shell" not in texts, texts
        captures.append(capture_checked(app, "01-ready.png", different_from=captures[-1]))

        # A quiet foreground job changes the actual tab, then Ctrl+C returns its
        # label to the shell. No shell hooks, OSC title, or output are required.
        command(app, first, "/bin/sleep 30")
        sleeping = wait_state(app, first, lambda p: p["tab_title"] == "sleep")
        assert sleeping["pid"] == first_pid, sleeping
        app.wait_until(lambda s: any(t["text"] == "sleep" for t in s["ui"]["texts"]))
        captures.append(capture_checked(app, "01a-foreground-process.png", different_from=captures[-1]))
        control(app, first, "KeyC")
        wait_state(app, first, lambda p: p["tab_title"] not in ("", "Terminal", "sleep"))

        # Printable input plus physical Backspace/Enter must reach the actual PTY.
        focus(app, first)
        app.input(
            {"type": "text", "text": print_marker("BED2_BACKSPACE_OK") + "x"},
            {"type": "key", "code": "Backspace", "pressed": True},
            {"type": "key", "code": "Backspace", "pressed": False},
            *enter_steps(),
        )
        wait_state(app, first, lambda p: "BED2_BACKSPACE_OK" in p["text"])

        # ANSI styling and carriage-return/erase affect the grid, not literal text.
        command(app, first,
                "printf '\\033[1;31m%s%s\\033[0m\\n' 'BED2_' 'ANSI_RED'; "
                "printf 'replace_me\\r\\033[2K%s%s\\n' 'BED2_' 'REWRITTEN'")
        ansi = wait_state(app, first, lambda p: "BED2_REWRITTEN" in p["grid"]["lines"]
                          and "BED2_ANSI_RED" in p["grid"]["lines"])
        row = ansi["grid"]["lines"].index("BED2_ANSI_RED")
        colored = [c for c in ansi["grid"]["cells"] if c["row"] == row]
        assert colored and all(c["attributes"] & 1 for c in colored), colored
        assert all(c["foreground"] == "Indexed(1)" for c in colored), colored
        assert not any(line.startswith("replace_me") for line in ansi["grid"]["lines"])
        assert "\x1b" not in ansi["text"]

        # The inspector emits one cluster per leading cell, including wide text.
        command(app, first, print_marker("BED2_WIDE_界e\u0301_END"))
        wide = wait_state(app, first, lambda p: "BED2_WIDE_界e\u0301_END" in p["grid"]["lines"])
        row = wide["grid"]["lines"].index("BED2_WIDE_界e\u0301_END")
        assert any(c["row"] == row and c["text"] == "界" and c["width"] == 2
                   for c in wide["grid"]["cells"]), wide

        # Alternate screen + a blocking shell read exercise modes and direct input.
        command(app, first,
                "printf '\\033[?1049h\\033[2J\\033[H%s%s\\n' 'BED2_' 'ALT_SCREEN'; "
                "IFS= read -r answer; printf '\\033[?1049l\\n%s%s:%s\\n' 'BED2_' 'READ' \"$answer\"")
        wait_state(app, first, lambda p: p["modes"]["alt_screen"] and "BED2_ALT_SCREEN" in p["text"])
        command(app, first, "typed-through-pty")
        wait_state(app, first, lambda p: not p["modes"]["alt_screen"]
                   and "BED2_READ:typed-through-pty" in p["text"])
        command(app, first,
                "cd /; printf '\\033]2;%s%s\\007\\033]7;file://localhost/\\007' 'BED2_' 'TITLE'")
        wait_state(app, first, lambda p: p["title"] == "BED2_TITLE" and p["cwd"] == "/"
                   and p["tab_title"] not in ("", "Terminal", "BED2_TITLE"))
        assert "BED2_TITLE" not in [t["text"] for t in app.inspect()["ui"]["texts"]], \
            "OSC titles are still observed, but do not add an in-content header"
        captures.append(capture_checked(app, "02-ansi-input.png", different_from=captures[-1]))

        # Theme colors and fonts update the existing canvas/session, not a new
        # terminal. Include both ANSI ranges plus styles in actual rendered captures.
        original_theme = app.inspect()["workbench"]["surfaces"][0]["theme"]
        command(app, first,
                "printf '\\033[2J\\033[H'; "
                "for c in 30 31 32 33 34 35 36 37; do printf '\\033[%sm normal \\033[0m' \"$c\"; done; "
                "printf '\\n'; "
                "for c in 90 91 92 93 94 95 96 97; do printf '\\033[%sm bright \\033[0m' \"$c\"; done; "
                "printf '\\n\\033[1mBold\\033[0m  \\033[3mItalic\\033[0m  \\033[2mFaint\\033[0m  "
                "\\033[4mUnderline\\033[0m  \\033[9mStruck\\033[0m\\n'; "
                "printf '\\033[38;2;120;180;240mTruecolor\\033[0m  界é\\n'; "
                + print_marker("BED2_APPEARANCE"))
        appearance = wait_state(app, first, lambda p: "BED2_APPEARANCE" in p["grid"]["lines"])
        original_palette = appearance["palette"]
        original_lines = appearance["grid"]["lines"]
        original_grid = (appearance["grid"]["cols"], appearance["grid"]["rows"])
        captures.append(capture_checked(app, "02a-theme-dark.png", different_from=captures[-1]))

        app.request("theme.set", preset="light")
        light = wait_state(app, first, lambda p: p["palette"]["background"] == [253, 246, 227])
        assert light["pid"] == first_pid and light["grid"]["lines"] == original_lines, light
        assert light["palette"]["ansi"] != original_palette["ansi"], light
        truecolor = [c for c in light["grid"]["cells"] if c["text"] == "T"
                     and c["foreground"] == "Rgb([120, 180, 240])"]
        assert truecolor, light
        captures.append(capture_checked(app, "02b-theme-light.png", different_from=captures[-1]))

        app.request("theme.set", preset="dark")
        custom_ansi = [entry[:] for entry in original_theme["palette"]["terminal"]]
        custom_ansi[4] = [1.0, 0.6, 1.0, 1.0]
        custom_ansi[12] = [0.6, 1.0, 1.0, 1.0]
        app.request("theme.set", theme={"palette": {"terminal": custom_ansi}})
        custom = wait_state(app, first, lambda p: p["palette"]["ansi"][4] == [255, 153, 255]
                            and p["palette"]["ansi"][12] == [153, 255, 255])
        assert custom["pid"] == first_pid and custom["grid"]["lines"] == original_lines, custom
        captures.append(capture_checked(app, "02c-theme-custom.png", different_from=captures[-1]))

        # OSC overrides remain application-owned across host theme updates; an
        # OSC reset restores the CURRENT host theme rather than the startup one.
        command(app, first, "printf '\\033]4;4;#123456\\007'")
        wait_state(app, first, lambda p: p["palette"]["ansi"][4] == [18, 52, 86])
        app.request("theme.set", preset="light")
        wait_state(app, first, lambda p: p["palette"]["background"] == [253, 246, 227]
                   and p["palette"]["ansi"][4] == [18, 52, 86])
        command(app, first, "printf '\\033]104;4\\007'")
        wait_state(app, first, lambda p: p["palette"]["ansi"][4] == light["palette"]["ansi"][4])

        app.request("theme.set", theme={"metrics": {"font_size": original_theme["metrics"]["font_size"] + 3}})
        enlarged = wait_state(app, first, lambda p: (p["grid"]["cols"], p["grid"]["rows"]) != original_grid)
        assert enlarged["pid"] == first_pid, enlarged
        captures.append(capture_checked(app, "02d-theme-font-size.png", different_from=captures[-1]))
        app.request("theme.set", theme=original_theme)
        wait_state(app, first, lambda p: p["palette"] == original_palette
                   and (p["grid"]["cols"], p["grid"]["rows"]) == original_grid)

        # Resize the real canvas, then ask the child for the kernel's PTY size.
        old_grid = (state(app, first)["grid"]["rows"], state(app, first)["grid"]["cols"])
        app.request("window.resize", size=[1100, 720])
        app.wait_until(lambda s: s["windows"][0]["size"] == [1100, 720])
        app.request("wait_frames", frames=45)  # Complete the normal layout animation.
        resized = wait_state(app, first, lambda p: (p["grid"]["rows"], p["grid"]["cols"]) != old_grid)
        assert resized["pid"] == first_pid
        command(app, first, "printf '\\n%s%s ' 'BED2_' 'SIZE'; stty size")
        sized = wait_state(app, first, lambda p: re.search(r"(?:^|\n)BED2_SIZE (\d+) (\d+)", p["text"]) is not None)
        size = re.search(r"(?:^|\n)BED2_SIZE (\d+) (\d+)", sized["text"])
        assert [int(size.group(1)), int(size.group(2))] == [sized["grid"]["rows"], sized["grid"]["cols"]], sized
        captures.append(capture_checked(app, "03-resized.png"))

        # A second ordinary panel gets its own shell, grid and input target.
        split = app.request("area.split", area=area, axis="horizontal", fraction=0.5)
        second = app.request("panel.open", key=PANEL_KEY, area=split["area"])["panel"]
        second_state = bootstrap(app, second, "BED2_SECOND_ONLY")
        second_pid = second_state["pid"]
        assert second_pid != first_pid and "BED2_SECOND_ONLY" not in state(app, first)["text"]
        command(app, first, "BED2_ID=one; " + print_marker("BED2_FIRST_ONLY"))
        wait_state(app, first, lambda p: "BED2_FIRST_ONLY" in p["text"])
        assert "BED2_FIRST_ONLY" not in state(app, second)["text"]
        command(app, second, "BED2_ID=two; printf '\\n%s%s:%s\\n' 'BED2_' 'IDENTITY' \"$BED2_ID\"")
        wait_state(app, second, lambda p: "BED2_IDENTITY:two" in p["text"])
        command(app, first, "printf '\\n%s%s:%s\\n' 'BED2_' 'IDENTITY' \"$BED2_ID\"")
        wait_state(app, first, lambda p: "BED2_IDENTITY:one" in p["text"])
        assert_owned(app, workspace_id, [first, second])
        captures.append(capture_checked(app, "04-independent.png", different_from=captures[-1]))

        # Output continues while a tab is hidden and while its workspace is
        # inactive with background_updates disabled. Repaint before reading text.
        command(app, first,
                "i=0; while [ \"$i\" -lt 60 ]; do printf '\\n%s%s:%s\\n' 'BED2_' 'TICK' \"$i\"; "
                "i=$((i + 1)); sleep 1; done")
        before = tick(wait_state(app, first, lambda p: tick(p) >= 0))
        cover = app.request("panel.open", key="bed.hello", area=area)["panel"]
        wait_state(app, first, lambda p: not p["presentation"]["visible"])
        observe_hidden(app, first, first_pid)
        app.request("panel.focus", panel=first)
        shown = wait_state(app, first, lambda p: p["presentation"]["visible"] and tick(p) >= before + 2)
        before = tick(shown)
        app.request("workspace.configure", workspace=workspace_id, background_updates=False)
        other = app.request("workspace.create", name="Other", activate=True)
        observe_hidden(app, first, first_pid, inactive=True)
        assert_owned(app, workspace_id, [first, second])
        app.request("workspace.activate", workspace=workspace_id)
        wait_state(app, first, lambda p: p["presentation"]["visible"] and tick(p) >= before + 2)
        control(app, first, "KeyC")
        command(app, first, print_marker("BED2_INTERRUPTED"))
        wait_state(app, first, lambda p: "BED2_INTERRUPTED" in p["text"])
        app.request("panel.close", panel=cover)
        captures.append(capture_checked(app, "05-background.png"))

        # A nonzero shell exit retains the transcript and offers a real Restart.
        command(app, first, print_marker("BED2_FINAL_TRANSCRIPT") + "; exit 7")
        ended = wait_state(app, first, lambda p: p["status"] == "Exited"
                           and "BED2_FINAL_TRANSCRIPT" in p["text"])
        assert ended["exit_code"] == 7 and ended["success"] is False, ended
        assert not process_exists(first_pid), ended
        app.wait_until(lambda s: any(t["text"] == "Restart" for t in s["ui"]["texts"]))
        captures.append(capture_checked(app, "06-exited.png"))
        app.click_text("Restart")
        fresh = wait_state(app, first, lambda p: p["status"] == "Running" and p["pid"] != first_pid)
        first_pid = fresh["pid"]
        assert first_pid is not None and "BED2_FINAL_TRANSCRIPT" not in fresh["text"], fresh
        bootstrap(app, first, "BED2_RESTARTED")
        assert state(app, second)["pid"] == second_pid
        captures.append(capture_checked(app, "07-restarted.png", different_from=captures[-1]))

        # Inactive layout restoration contains panel descriptions, not sessions.
        saved = app.request("layout.capture", layout=layout)
        restored = app.request("layout.restore", workspace=workspace_id, snapshot=saved)
        restored_panels = [p for p in inspection(app)["panels"] if p["entity"] not in (first, second)]
        assert len(restored_panels) == 2, restored_panels
        assert all(p["status"] == "Pending" and p["pid"] is None and not p["text"].strip()
                   for p in restored_panels), restored_panels
        assert_owned(app, workspace_id, [first, second])
        app.request("layout.activate", layout=restored["layout"])
        restored_ids = [p["entity"] for p in restored_panels]
        restored_pids = [wait_state(app, p, lambda v: v["status"] == "Running" and v["pid"] is not None)["pid"]
                         for p in restored_ids]
        assert len(set(restored_pids + [first_pid, second_pid])) == 4, restored_pids
        assert all("BED2_RESTARTED" not in state(app, p)["text"] and "BED2_SECOND_ONLY" not in state(app, p)["text"]
                   for p in restored_ids)
        bootstrap(app, restored_ids[0], "BED2_RESTORED_FRESH")
        captures.append(capture_checked(app, "08-restored.png"))
        app.request("layout.delete", layout=restored["layout"])
        wait_retired(app, workspace_id, restored_ids, restored_pids)
        app.request("layout.activate", layout=layout)
        assert state(app, first)["pid"] == first_pid

        # Close without pressing Stop: normal panel despawn and workspace close
        # must cancel/reap both the shell and a live foreground process.
        second_pids = foreground_child(app, second, "PANEL_CHILD")
        app.request("panel.close", panel=second)
        wait_retired(app, workspace_id, [second], second_pids)
        assert state(app, first)["status"] == "Running"
        first_pids = foreground_child(app, first, "WORKSPACE_CHILD")
        app.request("workspace.close", workspace=workspace_id, replacement=other["workspace"])
        wait_retired(app, workspace_id, [first], first_pids)
        snapshot = app.inspect()
        assert [w["entity"] for w in snapshot["workbench"]["workspaces"]] == [other["workspace"]]
        assert inspection(app)["panels"] == []

        # Rebuilding after cleanup still uses the ordinary panel lifecycle.
        rebuilt = app.request("panel.open", key=PANEL_KEY)["panel"]
        bootstrap(app, rebuilt, "BED2_REBUILT")
        captures.append(capture_checked(app, "09-rebuilt.png"))
        assert all(s["diagnostics"]["error"] is None for s in app.inspect()["ui"]["surfaces"])
        rebuilt_pid = state(app, rebuilt)["pid"]
        app.request("workspace.close", workspace=other["workspace"])
        wait_retired(app, other["workspace"], [rebuilt], [rebuilt_pid])
        assert inspection(app)["panels"] == []

    log = re.sub(r"\x1b\[[0-9;]*m", "", (output / "stderr.log").read_text())
    assert " ERROR " not in log and "panicked at" not in log, log
    (output / "summary.json").write_text(json.dumps({"passed": True, "captures": captures}, indent=2) + "\n")
    print(f"bed2 terminal smoke passed: {len(captures)} captures in {output}")


if __name__ == "__main__":
    main()
