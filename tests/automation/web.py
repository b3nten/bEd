"""Web browser: a new panel is blank; an address typed in its field loads,
and the page's title shows in the tab; Back returns to the page
before. The page is drawn in the panel by default, and by the system's web
view over it with `web.draw_pages` off.

The web view is a native view: the automation's clicks and keys don't reach
the page itself, only bed's own controls around it."""
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from bed import Bed

if sys.platform != "darwin":
    print("web pages show on macOS only: skipped")
    sys.exit(0)

PAGE = """<!doctype html><title>first page</title>
<body style="margin:0;font:20px sans-serif;background:linear-gradient(white,dodgerblue);height:100vh">
<h1 style="margin:0;padding:20px 80px">A page in the panel</h1>
<p style="padding:0 80px"><a href="/two">A link to the second page</a></p>
<div style="position:fixed;left:0;top:0;width:60px;height:60px;background:seagreen"></div>
<div style="position:fixed;right:0;bottom:0;width:60px;height:60px;background:crimson"></div>
"""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = (b"<title>second page</title>second" if self.path == "/two" else PAGE.encode())
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def until(bed, check, what, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(5)
    shown = [w.get("label") or w.get("value") for w in bed.widgets()]
    raise AssertionError(f"never: {what}; showing {[s for s in shown if s]}")


def go(bed, address):
    bed.click(bed.find(role="TextInput"))
    bed.key("Primary+A")
    bed.type(address)
    bed.key("Return")


server = HTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
site = f"127.0.0.1:{server.server_address[1]}"

for name, settings in [("web", {}), ("web_overlay", {"web.draw_pages": False})]:
    with Bed(settings=settings, name=name) as bed:
        bed.invoke("web.show")
        until(bed, lambda: bed.exists("Web"), "a blank panel shows")
        # The panel fades in where it's placed: its field is clicked once still.
        bed.wait(30)
        go(bed, site)
        until(bed, lambda: bed.exists("first page"), "the typed address loads")
        assert bed.find(role="TextInput")["value"] == f"http://{site}/"
        bed.wait(30)
        bed.screenshot(name)

        go(bed, site + "/two")
        until(bed, lambda: bed.exists("second page"), "the second page loads")
        bed.click("chevron-left")
        until(bed, lambda: bed.exists("first page"), "Back returns to the first page")
        bed.click("chevron-right")
        until(bed, lambda: bed.exists("second page"), "Forward goes on to the second")
        assert not bed.status()["errors"]

# From a terminal: `+web` takes anything as an address; `+open` takes what is
# plainly one. Each address gets a web panel of its own; one already showing
# it is focused instead.
with Bed(name="web_shell") as bed:
    bed.invoke("terminal.new")
    until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "a terminal opens")
    bed.wait(30)
    bed.type(f"+web {site}")
    bed.key("Enter")
    until(bed, lambda: bed.exists("first page"), "+web shows the address in a web panel")

    # Back to the terminal: the tab that's neither the page nor Workspaces.
    tabs = [w["label"][len("Close "):] for w in bed.widgets() if (w.get("label") or "").startswith("Close ")]
    bed.click(next(tab for tab in tabs if tab not in ("first page", "Workspaces")))
    bed.wait(10)
    bed.type(f"+open http://{site}/two")
    bed.key("Enter")
    until(bed, lambda: bed.exists("second page"), "+open shows a web address in a web panel")
    pages = lambda: [w["label"] for w in bed.widgets()
                     if (w.get("label") or "") in ("Close first page", "Close second page")]
    assert sorted(pages()) == ["Close first page", "Close second page"], "each address has its own panel"

    # The same address again: its panel is focused, no third one.
    tabs = [w["label"][len("Close "):] for w in bed.widgets() if (w.get("label") or "").startswith("Close ")]
    bed.click(next(tab for tab in tabs if tab not in ("first page", "second page", "Workspaces")))
    bed.wait(10)
    bed.type(f"+open http://{site}/two")
    bed.key("Enter")
    bed.wait(40)
    assert sorted(pages()) == ["Close first page", "Close second page"], "an open address opened again"
    assert not bed.status()["errors"]
