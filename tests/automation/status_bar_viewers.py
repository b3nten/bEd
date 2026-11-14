"""Status bar: Git, a terminal, a database and the HTTP client each show
what they're about while focused (`add_panel_status`)."""
import sqlite3, subprocess, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from bed import Bed

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'{"ok": true}'
        self.send_response(200); self.send_header("Content-Type","application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
server = HTTPServer(("127.0.0.1", 0), H); threading.Thread(target=server.serve_forever, daemon=True).start()

def until(bed, check, what, tries=300):
    for _ in range(tries):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}; the bar shows {bar(bed)!r}")

def bar(bed):
    """The status bar's texts, left to right."""
    h = bed.status()["window"]["height"]
    ws = [w for w in bed.widgets() if w.get("rect") and w["rect"][1] > h - 40 and (w.get("label") or w.get("value"))]
    return " | ".join((w.get("label") or w.get("value")) for w in sorted(ws, key=lambda w: w["rect"][0]))

def git(root, *a): subprocess.run(["git", *a], cwd=root, check=True, capture_output=True)

with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    git(root, "init", "-q", "-b", "main"); git(root, "config", "user.email", "b@e.com"); git(root, "config", "user.name", "b")
    (root / "a.txt").write_text("one\n"); git(root, "add", "."); git(root, "commit", "-q", "-m", "x")
    (root / "a.txt").write_text("two\n"); (root / "b.txt").write_text("new\n"); git(root, "add", "b.txt")
    db = sqlite3.connect(root / "demo.db"); db.execute("create table users(id integer primary key, name text)")
    db.executemany("insert into users(name) values (?)", [(f"u{i}",) for i in range(1500)]); db.commit(); db.close()

    with Bed(folder, settings={"effects.enabled": True}) as bed:
        until(bed, lambda: bed.exists("a.txt"), "listed")
        # Panels open in the focused area: focus the large one, not the explorer's.
        window = bed.status()["window"]; bed.click((window["width"] * 0.6, window["height"] * 0.5)); bed.wait(5)
        bed.invoke("git.show"); until(bed, lambda: "main" in bar(bed) and "3 changed (1 staged)" in bar(bed), "the branch and changes")
        bed.screenshot("status_bar_git")

        bed.invoke("terminal.new"); until(bed, lambda: "×" in bar(bed), "the terminal's size")
        bed.screenshot("status_bar_terminal")

        rows = [w for w in bed.find_all("demo.db")]; w = min(rows, key=lambda w: w["rect"][0])
        x0,y0,x1,y1 = w["rect"]; at={"move":{"x":(x0+x1)/2,"y":(y0+y1)/2}}
        bed.input(at, {"press":"Left"},{"release":"Left"},{"press":"Left"},{"release":"Left"})
        until(bed, lambda: "users (table) | rows 1–1,000 of 1,500" in bar(bed), "the table and its rows")
        bed.screenshot("status_bar_sqlite")

        bed.invoke("http_client.show"); until(bed, lambda: bed.exists("New Request"), "http panel")
        bed.click("New Request"); bed.type(f"127.0.0.1:{server.server_address[1]}/x"); bed.key("Return")
        until(bed, lambda: "200 OK" in bar(bed) and "GET 127.0.0.1" in bar(bed), "the request and response")
        bed.screenshot("status_bar_http")
