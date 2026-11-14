"""HTTP client: a new request's URL is typed and sent with Enter; the
response's status, JSON body (indented) and headers show; the request is
saved in the workspace's store. An ISO-8859-1 HTML page shows as text,
decoded from its charset, not as a binary body; a PNG shows as a picture.
JSON can show as a tree; other bytes show as hex."""
import json
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from bed import Bed

FIXTURES = Path(__file__).resolve().parents[1] / "fixtures"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/bytes"):
            body = bytes(range(256))
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path.startswith("/picture"):
            body = (FIXTURES / "images" / "sample.png").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path.startswith("/latin"):
            body = "<html><p>Fran\u00e7ais</p></html>".encode("latin-1")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=ISO-8859-1")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = json.dumps({"path": self.path, "items": [1, 2, 3], "ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("X-Bed-Test", "yes")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def until(bed, check, what, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    bed.screenshot("http_client_failed")
    raise AssertionError(f"never: {what}")


server = HTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
url = f"127.0.0.1:{server.server_address[1]}/users?id=7"
latin = f"127.0.0.1:{server.server_address[1]}/latin"
picture = f"127.0.0.1:{server.server_address[1]}/picture"
binary = f"127.0.0.1:{server.server_address[1]}/bytes"

with tempfile.TemporaryDirectory() as folder:
    # Effects keep the window drawing while it is in the background.
    with Bed(folder, settings={"effects.enabled": True}) as bed:
        # Panels open in the focused area: focus the large one, not the
        # explorer's.
        window = bed.status()["window"]
        bed.click((window["width"] * 0.6, window["height"] * 0.5))
        bed.wait(5)
        bed.invoke("http_client.show")
        until(bed, lambda: bed.exists("New Request"), "the empty panel shows")
        bed.click("New Request")
        bed.type(url)
        bed.key("Return")
        until(bed, lambda: bed.exists(contains="200 OK"), "the response arrives")
        until(bed, lambda: bed.exists(contains='"path": "/users?id=7"'), "the body is indented")
        bed.screenshot("http_client_body")

        # The same JSON as a tree.
        bed.click("Tree")
        until(bed, lambda: bed.exists("items: 3 items"), "the JSON shows as a tree")
        bed.screenshot("http_client_tree")
        bed.click("JSON")

        bed.click(contains="Headers (")
        until(bed, lambda: bed.exists("X-Bed-Test") or bed.exists("x-bed-test"), "headers show")
        bed.screenshot("http_client_headers")

        def saved():
            files = list(Path(bed.config).glob("workspaces/*/http_requests.ron"))
            return files and url in files[0].read_text()

        until(bed, saved, "the request is saved in the workspace")

        # The same request, now to a Latin-1 page.
        bed.click(url, role="TextInput")
        bed.key("Primary+A")
        bed.type(latin)
        bed.key("Return")
        until(bed, lambda: bed.exists(contains="/latin", role="Label"), "the Latin-1 page arrives")
        # The response's Body tab: the lower of the two.
        bed.click(max(bed.find_all("Body"), key=lambda w: w["rect"][1]))
        until(bed, lambda: bed.exists(contains="Fran\u00e7ais"), "the Latin-1 page shows as text")
        assert bed.exists(contains="Decoded from windows-1252"), "the charset is noted"
        assert not bed.exists(contains="Binary body"), "it isn't called binary"
        bed.screenshot("http_client_latin1")

        # A PNG shows as a picture, with its size.
        bed.click(latin, role="TextInput")
        bed.key("Primary+A")
        bed.type(picture)
        bed.key("Return")
        until(bed, lambda: bed.exists(contains="· PNG"), "the PNG shows as a picture")
        bed.screenshot("http_client_image")

        # Other bytes show as hex.
        bed.click(picture, role="TextInput")
        bed.key("Primary+A")
        bed.type(binary)
        bed.key("Return")
        until(bed, lambda: bed.exists(contains="Binary body: 256 B"), "the bytes arrive")
        until(bed, lambda: bed.exists(contains="0x0"), "they show as hex")
        bed.screenshot("http_client_hex")

server.shutdown()
print("ok")
