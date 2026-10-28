#!/usr/bin/env python3
"""Drive bEd from a script: launch it, click and type, look at its widgets,
take screenshots. Standard library only.

    from bed import Bed

    with Bed(settings={"appearance.theme": "Light"}) as bed:
        bed.key("Primary+Comma")
        bed.click("Font")
        bed.screenshot("settings")

Each `Bed` gets its own temporary config directory and port, and its window
opens unfocused, so scripts can run while you keep working. Screenshots and
logs go to target/automation/.

Run saved scripts (tests/automation/*.py by default):

    python3 tools/bed.py run [script-or-directory ...]

Scripts can import this module because the runner puts tools/ on PYTHONPATH;
run one directly with `PYTHONPATH=tools python3 script.py`.

The app side is bedtime-automation (bedtime/crates/bedtime-automation), over
the Bevy Remote Protocol; `call` reaches any of its methods, including Bevy's
own (`world.query`, `world.get_resources`, ...).
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "target" / "automation"
BINARY = ROOT / "target" / "debug" / "bed"

# Chord names, as in keybinding settings, to Bevy key codes.
MODIFIERS = {
    "Primary": "SuperLeft" if sys.platform == "darwin" else "ControlLeft",
    "Super": "SuperLeft",
    "Ctrl": "ControlLeft",
    "Control": "ControlLeft",
    "Shift": "ShiftLeft",
    "Alt": "AltLeft",
}
KEYS = {
    ",": "Comma", ".": "Period", "/": "Slash", ";": "Semicolon", "'": "Quote",
    "[": "BracketLeft", "]": "BracketRight", "-": "Minus", "=": "Equal",
    "`": "Backquote", "\\": "Backslash", " ": "Space",
    "Esc": "Escape", "Return": "Enter", "Up": "ArrowUp", "Down": "ArrowDown",
    "Left": "ArrowLeft", "Right": "ArrowRight",
}


def key_code(name):
    """'a' -> 'KeyA', '1' -> 'Digit1', ',' -> 'Comma'; Bevy names pass through."""
    if name in KEYS:
        return KEYS[name]
    if len(name) == 1 and name.isalpha():
        return "Key" + name.upper()
    if len(name) == 1 and name.isdigit():
        return "Digit" + name
    return name


_built = False


def build():
    """Build bed with automation, once per process."""
    global _built
    if not _built:
        subprocess.run(
            ["cargo", "build", "--quiet", "-p", "bed", "--features", "automation"],
            cwd=ROOT,
            check=True,
        )
        _built = True


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class AutomationError(Exception):
    pass


class Bed:
    """A running bEd. `folder` opens a workspace; `settings` maps setting keys
    to RON values written to the global layer before launch; `recent` lists
    folders for the recent list."""

    def __init__(self, folder=None, *, settings=None, recent=None, name=None, timeout=30, build_first=True):
        if build_first:
            build()
        self.name = name or Path(sys.argv[0]).stem or "bed"
        OUTPUT.mkdir(parents=True, exist_ok=True)
        self.config = Path(tempfile.mkdtemp(prefix="bed-automation-"))
        values = settings or {}
        global_dir = self.config / "global"
        global_dir.mkdir()
        (global_dir / "settings.ron").write_text(
            "{\n" + "".join(f"    {json.dumps(k)}: {v},\n" for k, v in values.items()) + "}\n"
        )
        if recent:
            folders = [str(Path(folder).resolve()) for folder in recent]
            (global_dir / "recent.ron").write_text(
                "[\n" + "".join(f"    {json.dumps(f)},\n" for f in folders) + "]\n"
            )
        self.port = free_port()
        self._id = 0
        self.log_path = OUTPUT / f"{self.name}.log"
        log = open(self.log_path, "w")
        env = {
            **os.environ,
            "BED_CONFIG_DIR": str(self.config),
            "BEDTIME_AUTOMATION_PORT": str(self.port),
        }
        command = [str(BINARY)] + ([str(folder)] if folder else [])
        self.process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            self._wait_ready(timeout)
        except BaseException:
            self.close()
            raise

    # --- protocol ---

    def call(self, method, params=None):
        """Call any remote method; returns its result."""
        self._id += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self._id, "method": method, "params": params})
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/",
            data=body.encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            reply = json.load(response)
        if "error" in reply:
            raise AutomationError(f"{method}: {reply['error'].get('message')}")
        return reply.get("result")

    def status(self):
        return self.call("bedtime.status")

    def _wait_ready(self, timeout):
        deadline = time.monotonic() + timeout
        while True:
            if self.process.poll() is not None:
                raise AutomationError(f"bed exited ({self.process.returncode}); see {self.log_path}")
            try:
                if self.status()["frame"] >= 3 and self.widgets():
                    return
            except (urllib.error.URLError, ConnectionError, socket.timeout):
                pass
            if time.monotonic() > deadline:
                raise AutomationError(
                    f"bed didn't answer on its automation port; see {self.log_path}. "
                    "Was it built with --features automation?"
                )
            time.sleep(0.05)

    def settle(self, frames=2, timeout=10):
        """Wait until queued input has run, then `frames` more frames."""
        deadline = time.monotonic() + timeout
        target = None
        while True:
            status = self.status()
            if status["errors"]:
                raise AutomationError("; ".join(status["errors"]))
            if target is None and status["pending"] == 0:
                target = status["frame"] + frames
            if target is not None and status["frame"] >= target:
                return
            if time.monotonic() > deadline:
                raise AutomationError("the app stopped advancing frames")
            time.sleep(0.01)

    # --- looking ---

    def widgets(self):
        """Every widget in the last frame, in tree order: dicts with id,
        parent, role, label, value, rect [x0, y0, x1, y1] (logical pixels),
        disabled, focused, toggled, selected."""
        return self.call("bedtime.ui")

    def find_all(self, label=None, *, role=None, contains=None, visible=True):
        """Widgets matching label (exact), contains (substring) and role. With
        `visible`, only those whose centre is inside the window: scrolled-out
        widgets are still in the tree."""
        window = self.status()["window"] if visible else None
        found = []
        for widget in self.widgets():
            text = widget.get("label") or widget.get("value") or ""
            if label is not None and text != label:
                continue
            if contains is not None and contains not in text:
                continue
            if role is not None and widget["role"] != role:
                continue
            rect = widget.get("rect")
            if not rect:
                continue
            if window:
                x, y = (rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2
                if not (0 <= x < window["width"] and 0 <= y < window["height"]):
                    continue
            found.append(widget)
        return found

    def find(self, label=None, *, role=None, contains=None, index=0, visible=True):
        found = self.find_all(label, role=role, contains=contains, visible=visible)
        if len(found) <= index:
            labels = sorted({w.get("label") or w.get("value") or "" for w in self.widgets()} - {""})
            raise AutomationError(
                f"No widget {label or contains!r} (role {role}); labels: {labels}"
            )
        return found[index]

    def exists(self, label=None, **kwargs):
        return bool(self.find_all(label, **kwargs))

    def screenshot(self, name, timeout=10):
        """Save the window as rendered to target/automation/<name>.png."""
        path = OUTPUT / f"{name}.png"
        path.unlink(missing_ok=True)
        self.call("bedtime.screenshot", {"path": str(path)})
        deadline = time.monotonic() + timeout
        while str(path) not in self.status()["screenshots"]:
            if time.monotonic() > deadline:
                raise AutomationError(f"screenshot {name} wasn't written")
            time.sleep(0.02)
        return path

    # --- doing ---

    def input(self, *steps):
        self.call("bedtime.input", {"steps": list(steps)})
        self.settle()

    def move(self, x, y):
        self.input({"move": {"x": x, "y": y}})

    def click(self, target=None, *, role=None, button="Left", **kwargs):
        """Click a widget (found by label, or a dict from `find`) or an (x, y)."""
        if isinstance(target, tuple):
            x, y = target
        else:
            widget = target if isinstance(target, dict) else self.find(target, role=role, **kwargs)
            x0, y0, x1, y1 = widget["rect"]
            x, y = (x0 + x1) / 2, (y0 + y1) / 2
        self.input(
            {"move": {"x": x, "y": y}},
            {"press": button},
            {"release": button},
        )

    def drag(self, start, end, steps=8):
        (x0, y0), (x1, y1) = start, end
        moves = [
            {"move": {"x": x0 + (x1 - x0) * i / steps, "y": y0 + (y1 - y0) * i / steps}}
            for i in range(1, steps + 1)
        ]
        self.input({"move": {"x": x0, "y": y0}}, {"press": "Left"}, *moves, {"release": "Left"})

    def key(self, chord):
        """Press a chord like 'Primary+Shift+Z', 'Escape' or 'Primary+,'."""
        if chord == "+" or chord.endswith("++"):
            modifiers, key = chord[:-2].split("+") if len(chord) > 1 else [], "+"
        else:
            *modifiers, key = chord.split("+")
        held = [MODIFIERS[m] for m in modifiers]
        steps = [{"key_down": m} for m in held]
        steps += [{"key_down": key_code(key)}, {"key_up": key_code(key)}]
        steps += [{"key_up": m} for m in reversed(held)]
        self.input(*steps)

    def type(self, text):
        self.input({"text": text})

    def scroll(self, dy, dx=0, at=None):
        steps = []
        if at is not None:
            x0, y0, x1, y1 = (self.find(at) if isinstance(at, str) else at)["rect"]
            steps.append({"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}})
        steps.append({"scroll": {"x": dx, "y": dy}})
        self.input(*steps)

    def invoke(self, command):
        """Run a command by key ('app.open_settings'), as its shortcut would."""
        self.call("bedtime.invoke", {"command": command})
        self.settle()

    def wait(self, frames):
        self.input({"wait": frames})

    def relaunched(self, action, timeout=10):
        """Run `action`, which switches workspace, and wait until the next App
        (rebuilt in the same window) is drawing. Returns the seconds taken."""
        launch = self.status()["launch"]
        start = time.monotonic()
        action()
        while time.monotonic() - start < timeout:
            try:
                status = self.status()
                if status["launch"] > launch and status["frame"] >= 3 and self.widgets():
                    return time.monotonic() - start
            except (urllib.error.URLError, ConnectionError, AutomationError):
                pass
            time.sleep(0.01)
        raise AutomationError("the next App didn't start drawing")

    # --- ending ---

    def close(self, timeout=10):
        """Exit normally (stores are saved), then remove the config directory."""
        if self.process.poll() is None:
            try:
                self.call("bedtime.exit")
                self.process.wait(timeout)
            except (AutomationError, urllib.error.URLError, subprocess.TimeoutExpired, ConnectionError):
                self.process.kill()
                self.process.wait()
        shutil.rmtree(self.config, ignore_errors=True)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def run(paths):
    """Run each script in its own process; report and return the failures."""
    scripts = []
    for path in (Path(path).resolve() for path in paths or [ROOT / "tests" / "automation"]):
        scripts += sorted(path.glob("*.py")) if path.is_dir() else [path]
    build()
    env = {**os.environ, "PYTHONPATH": str(ROOT / "tools")}
    failed = []
    for script in scripts:
        start = time.monotonic()
        result = subprocess.run([sys.executable, str(script)], env=env, cwd=ROOT)
        took = time.monotonic() - start
        ok = result.returncode == 0
        print(f"{'ok  ' if ok else 'FAIL'} {script.relative_to(ROOT)} ({took:.1f}s)")
        if not ok:
            failed.append(script)
    print(f"{len(scripts) - len(failed)} passed, {len(failed)} failed")
    return failed


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "run":
        sys.exit(1 if run(sys.argv[2:]) else 0)
    print(__doc__)
