"""HTTP client: a new request's URL is typed and sent with Enter; the
response's status, JSON body (indented) and headers show; the request is
saved in the workspace's store."""
import json
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from bed import Bed


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
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
    raise AssertionError(f"never: {what}")


server = HTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
url = f"127.0.0.1:{server.server_address[1]}/users?id=7"

with tempfile.TemporaryDirectory() as folder:
    with Bed(folder) as bed:
        bed.invoke("http_client.show")
        until(bed, lambda: bed.exists("New Request"), "the empty panel shows")
        bed.click("New Request")
        bed.type(url)
        bed.key("Return")
        until(bed, lambda: bed.exists(contains="200 OK"), "the response arrives")
        until(bed, lambda: bed.exists(contains='"path": "/users?id=7"'), "the body is indented")
        bed.screenshot("http_client_body")

        bed.click(contains="Headers (")
        until(bed, lambda: bed.exists("X-Bed-Test") or bed.exists("x-bed-test"), "headers show")
        bed.screenshot("http_client_headers")

        def saved():
            files = list(Path(bed.config).glob("workspaces/*/http_requests.ron"))
            return files and url in files[0].read_text()

        until(bed, saved, "the request is saved in the workspace")

server.shutdown()
print("ok")
