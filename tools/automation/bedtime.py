"""Standard-library client for local Bedtime automation protocol version 1."""
from __future__ import annotations

import base64
from dataclasses import dataclass
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time


class AutomationError(RuntimeError):
    def __init__(self, error):
        self.kind = error.get("kind", "Failed")
        super().__init__(error.get("message", str(error)))


@dataclass(frozen=True)
class Panel:
    handle: str
    document: str | None = None


@dataclass(frozen=True)
class Capture:
    frame: dict
    images: list

    def save(self, path, index=0):
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(base64.b64decode(self.images[index]["png"], validate=True))
        return path


class Operation:
    """A retained completion receipt. Release after dependent captures are submitted."""
    def __init__(self, client, operation):
        self.client = client
        self.id = operation
        self._result = None
        self._finished = False
        self._released = False

    def wait(self, timeout=90):
        if self._released:
            raise RuntimeError("Operation receipt has been released")
        deadline = time.monotonic() + timeout
        while not self._finished:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"Operation {self.id} did not complete")
            response = self.client._request("poll", {"operation": self.id}, timeout=remaining)
            if response["state"] == "complete":
                self._result = response
                self._finished = True
            else:
                time.sleep(min(0.02, max(0, deadline - time.monotonic())))
        if "error" in self._result:
            raise AutomationError(self._result["error"])
        return self._result["value"]

    def release(self):
        if not self._released:
            self.client._request("release", {"operation": self.id})
            self._released = True

    def __enter__(self):
        return self

    def __exit__(self, exception_type, *_):
        try:
            self.release()
        except (OSError, ConnectionError):
            if exception_type is None:
                raise


class Client:
    def __init__(self, connection, *, timeout=90):
        self._socket = connection
        self._buffer = bytearray()
        self._sequence = 0
        self.timeout = timeout
        self.process = None
        self._temporary = None
        self._log = None
        self.log_path = None

    @classmethod
    def connect(cls, socket_path, *, name="agent script", timeout=300):
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(timeout)
        try:
            connection.connect(os.fspath(socket_path))
            client = cls(connection)
            deadline = time.monotonic() + timeout
            status = client._request("hello", {"protocol": 1, "name": name}, timeout=timeout)
            while status["state"] == "pending":
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Connection was not approved")
                time.sleep(min(0.05, remaining))
                status = client._request("connection_status", {}, timeout=max(0.001, deadline - time.monotonic()))
            client.application = status["application"]
            client.connection_id = status["connection"]
            return client
        except BaseException:
            connection.close()
            raise

    @classmethod
    def launch(cls, binary, *, args=(), config_dir=None, name="smoke test", approve=True,
               timeout=90, log_path=None):
        temporary = tempfile.TemporaryDirectory(prefix="bta-", dir="/tmp")
        directory = Path(temporary.name)
        directory.chmod(0o700)
        endpoint = directory / "control.sock"
        config = Path(config_dir).resolve() if config_dir else directory / "config"
        log_path = Path(log_path).resolve() if log_path else directory / "application.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log = log_path.open("wb")
        command = [str(Path(binary).resolve()), "--config-dir", str(config),
                   "--allow-automation", "--automation-socket", str(endpoint)]
        if approve:
            command += [f"--approve-automation={name}"]
        command += [os.fspath(arg) for arg in args]
        process = None
        try:
            process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + timeout
            while not endpoint.exists():
                if process.poll() is not None:
                    raise RuntimeError(f"Application exited before creating its endpoint:\n{log_path.read_text(errors='replace')[-8000:]}")
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"No automation endpoint; see {log_path}")
                time.sleep(0.02)
            client = cls.connect(endpoint, name=name, timeout=max(0.001, deadline-time.monotonic()))
            client.process, client._temporary, client._log = process, temporary, log
            client.log_path = log_path
            client.config_dir = config
            return client
        except BaseException:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            log.close()
            temporary.cleanup()
            raise

    def _request(self, method, params, *, timeout=None):
        self._sequence += 1
        request_id = self._sequence
        payload = json.dumps({"id": request_id, "method": method, "params": params},
                             separators=(",", ":"), allow_nan=False).encode() + b"\n"
        if len(payload) > 1024 * 1024:
            raise ValueError("Request exceeds 1 MiB")
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        try:
            self._socket.settimeout(max(0.001, deadline-time.monotonic()))
            self._socket.sendall(payload)
            while b"\n" not in self._buffer:
                self._socket.settimeout(max(0.001, deadline-time.monotonic()))
                chunk = self._socket.recv(65536)
                if not chunk:
                    raise ConnectionError("Automation connection closed")
                self._buffer.extend(chunk)
                if len(self._buffer) > 96 * 1024 * 1024:
                    raise ValueError("Response exceeds 96 MiB")
            line, _, remaining = self._buffer.partition(b"\n")
            self._buffer = bytearray(remaining)
            response = json.loads(line)
            if response.get("id") != request_id:
                raise ConnectionError("Unexpected response ID")
        except (OSError, ValueError, ConnectionError):
            # A partially exchanged request cannot be retried safely on this stream.
            self._socket.close()
            raise
        if "error" in response:
            raise AutomationError(response["error"])
        return response["result"]

    def submit(self, operation, **arguments):
        return Operation(self, self._request("submit", {"operation": operation, "arguments": arguments})["operation"])

    def call(self, operation, *, timeout=90, **arguments):
        with self.submit(operation, **arguments) as receipt:
            return receipt.wait(timeout)

    def describe(self):
        return self.call("describe")

    def inspect(self):
        return self.call("inspect")

    def wait_for(self, predicate, *, timeout=90, description="condition"):
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"Timed out waiting for {description}")
            snapshot = self.call("inspect", timeout=remaining)
            result = predicate(snapshot)
            if result:
                return result
            time.sleep(min(0.02, max(0, deadline-time.monotonic())))

    @staticmethod
    def _handle(panel):
        return panel.handle if isinstance(panel, Panel) else panel

    def wait_ready(self, panel, *, timeout=90):
        handle = self._handle(panel)
        def ready(snapshot):
            observation = next((p for p in snapshot["panels"] if p["handle"] == handle), None)
            if observation is None:
                raise AutomationError({"kind": "StaleHandle", "message": "Panel closed"})
            state = observation["readiness"]
            if state["state"] == "failed":
                raise AutomationError({"message": state["error"]})
            return observation if state["state"] == "ready" and observation["visible_content"] else False
        return self.wait_for(ready, timeout=timeout, description=f"panel {handle} readiness")

    def open_panel(self, panel_type, *, state=None, area=None):
        arguments = {"panel_type": panel_type, "state": state}
        if area is not None:
            arguments["area"] = area
        return Panel(self.call("open_panel", **arguments)["panel"])

    def open_file(self, path, *, viewer=None):
        value = self.call("bedtime.open_file", path=os.fspath(path), viewer=viewer)
        return Panel(value["panel"], value.get("document"))

    def focus(self, panel):
        self.call("bedtime.focus_window")
        return self.call("focus_panel", panel=self._handle(panel))

    def split(self, panel, *, axis=0, high=True):
        value = self.call("split_panel", panel=self._handle(panel), axis=axis, high=high)
        return Panel(value["panel"])

    def close_panel(self, panel):
        return self.call("close_panel", panel=self._handle(panel))

    def invoke(self, command, *, source=None, arguments=None):
        return self.call("invoke", command=command,
                         source=self._handle(source), arguments=arguments)

    def input(self, steps):
        return self.call("input", steps=steps)

    def type_text(self, text):
        return self.input([[{"type": "text", "text": text}]])

    def click(self, x, y, *, button="left"):
        return self.input([[{"type": "pointer", "position": [x, y]}],
                           [{"type": "button", "button": button, "pressed": True}],
                           [{"type": "button", "button": button, "pressed": False}]])

    def shortcut(self, physical, *, named=None, character=None, primary=False, shift=False):
        key = {"type": "key", "physical": physical}
        key.update({"named": named} if named else {"character": character})
        modifiers = {"type": "modifiers", "shift": shift}
        if primary:
            import sys
            modifiers["super" if sys.platform == "darwin" else "ctrl"] = True
        return self.input([[modifiers], [dict(key, pressed=True)], [dict(key, pressed=False)],
                           [{"type": "modifiers"}]])

    def capture(self, panel=None, *, after=None, timeout=30):
        value = self.call("capture", targets=[self._handle(panel) if panel else "window"],
                          after=after.id if after else None, timeout_ms=int(timeout*1000), timeout=timeout+1)
        return Capture(value["frame"], value["images"])

    def document_handle(self, document):
        if isinstance(document, Panel):
            if document.document:
                return document.document
            handle = document.handle
            info = next(p for p in self.inspect()["panels"] if p["handle"] == handle)
            if not info.get("document"):
                raise ValueError("Panel has no document")
            return info["document"]
        return document

    def read_document(self, document):
        value = self.call("read_document", document=self.document_handle(document))
        value["bytes"] = base64.b64decode(value["bytes"], validate=True)
        return value

    def apply_edits(self, document, revision, edits):
        return self.call("bedtime.apply_edits", document=self.document_handle(document),
                         revision=revision, edits=[{"start": start, "end": end,
                             "bytes": base64.b64encode(data.encode() if isinstance(data, str) else data).decode()}
                             for start, end, data in edits])

    def save(self, *documents):
        return self.call("bedtime.save_documents", documents=[self.document_handle(d) for d in documents])

    def save_as(self, document, path):
        return self.call("bedtime.save_as", document=self.document_handle(document), path=os.fspath(path))

    def undo(self, document):
        return self.call("bedtime.undo", document=self.document_handle(document))

    def redo(self, document):
        return self.call("bedtime.redo", document=self.document_handle(document))

    def settings(self):
        return self.call("bedtime.settings.inspect")

    def set_setting(self, owner, key, value):
        return self.call("bedtime.settings.set", owner=owner, key=key, value=value)

    def quit(self, *, timeout=15):
        """Request an orderly exit; owned launches also verify process completion."""
        try:
            self.submit("bedtime.quit")
        except ConnectionError:
            if self.process is None:
                raise
        if self.process is not None:
            status = self.process.wait(timeout=timeout)
            if status:
                raise RuntimeError(f"Application exited with status {status}; see {self.log_path}")

    def close(self):
        self._socket.close()
        if self.process is not None:
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
            self._log.close()
            self._temporary.cleanup()
            self.process = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
