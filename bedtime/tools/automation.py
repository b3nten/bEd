#!/usr/bin/env python3
"""Standard-library client for bedtime-automation's JSON-lines protocol."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import queue
import subprocess
import sys
import threading


class AutomationError(RuntimeError):
    pass


class Application:
    def __init__(self, command, *, output="target/automation2", timeout=30):
        self.output = Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.timeout = timeout
        self._next_id = 0
        self._exit_requested = False
        self._responses = queue.Queue()
        self._log = (self.output / "stderr.log").open("w")
        self._transcript = (self.output / "transcript.jsonl").open("w")
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self._log, text=True, encoding="utf-8", bufsize=1,
        )
        threading.Thread(target=self._read, daemon=True).start()
        try:
            self.ready = self._receive(timeout)
            if self.ready.get("event") != "ready" or self.ready.get("protocol") != 1:
                raise AutomationError(f"Unexpected greeting: {self.ready}")
        except BaseException:
            self.close(check=False)
            raise

    def _record(self, direction, value):
        self._transcript.write(json.dumps({direction: value}) + "\n")
        self._transcript.flush()

    def _read(self):
        try:
            for line in self.process.stdout:
                try:
                    self._responses.put(json.loads(line))
                except ValueError:
                    self._responses.put(AutomationError(f"Non-JSON stdout: {line!r}"))
                    return
        finally:
            self._responses.put(AutomationError(
                f"Application closed stdout; see {self.output / 'stderr.log'}"
            ))

    def _receive(self, timeout):
        try:
            value = self._responses.get(timeout=timeout)
        except queue.Empty as error:
            raise AutomationError(
                f"No response within {timeout}s; see {self.output / 'stderr.log'}"
            ) from error
        if isinstance(value, Exception):
            raise value
        self._record("response", value)
        return value

    def request(self, op, *, timeout=None, **arguments):
        timeout = self.timeout if timeout is None else timeout
        self._next_id += 1
        value = {"id": self._next_id, "op": op,
                 "timeout_ms": min(int(timeout * 1000), 120000), **arguments}
        self._record("request", value)
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()
        response = self._receive(timeout + 5)
        if response.get("id") != value["id"]:
            raise AutomationError(f"Unexpected response: {response}")
        if "error" in response:
            raise AutomationError(f"{op}: {response['error']['message']}")
        if op == "exit":
            self._exit_requested = True
        return response["result"]

    def inspect(self, **arguments):
        return self.request("inspect", **arguments)

    def wait_until(self, predicate, *, frames=300):
        """Bounded frame polling; no wall-clock sleeps or assumed frame rate."""
        for _ in range(frames):
            snapshot = self.inspect()
            if predicate(snapshot):
                return snapshot
            self.request("wait_frames", frames=1)
        raise AutomationError(f"Condition did not become true in {frames} observations")

    def input(self, *steps):
        return self.request("input", steps=list(steps))

    def click_text(self, text, *, index=0):
        return self.input({"type": "click_text", "text": text, "index": index})

    def capture(self, filename, **arguments):
        return self.request("capture", path=str(self.output / filename), **arguments)

    def close(self, *, check=True):
        try:
            if self.process.poll() is None:
                try:
                    if not self._exit_requested:
                        self.request("exit", timeout=5)
                    self.process.stdin.close()
                    self.process.wait(timeout=10)
                except (AutomationError, BrokenPipeError, subprocess.TimeoutExpired):
                    self.process.terminate()
                    try:
                        self.process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        self.process.kill()
                        self.process.wait()
        finally:
            self._log.close()
            self._transcript.close()
        if check and self.process.returncode != 0:
            raise AutomationError(f"Application exited with {self.process.returncode}; see {self.output / 'stderr.log'}")

    def __enter__(self):
        return self

    def __exit__(self, exception_type, *_):
        self.close(check=exception_type is None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", default="target/automation2")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("provide an automation-enabled executable after --")
    with Application(command, output=args.output) as app:
        print(json.dumps(app.ready), flush=True)
        for line in sys.stdin:
            if line.strip():
                request = json.loads(line)
                op = request.pop("op")
                request.pop("id", None)
                print(json.dumps(app.request(op, **request)), flush=True)


if __name__ == "__main__":
    main()
