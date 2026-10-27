#!/usr/bin/env python3
"""Check actual stdio framing, EOF draining and broken-pipe exit without a GUI."""
import argparse
import json
from pathlib import Path
import subprocess


def exchange(binary, lines):
    result = subprocess.run([binary], input=lines, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=15)
    assert result.returncode == 0, result.stderr.decode(errors="replace")
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    assert responses[0]["event"] == "ready", responses
    return responses[1:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/examples/headless")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    commands = [
        b"{bad json}",
        json.dumps({"id": 1, "op": "wait_frames", "frames": 5}).encode(),
        json.dumps({"id": 2, "op": "inspect"}).encode(),
        json.dumps({"id": 3, "op": "unknown"}).encode(),
        json.dumps({"id": 4, "op": "input", "steps": []}).encode(),
        json.dumps({"id": 5, "op": "describe"}).encode(),
    ]
    responses = exchange(binary, b"\n".join(commands) + b"\n")
    assert responses[0]["id"] is None and "error" in responses[0]
    assert [r["id"] for r in responses[1:]] == [1, 2, 3, 4, 5], responses
    assert responses[1]["result"]["waited"] == 5
    assert responses[2]["result"]["windows"] == []
    assert "error" in responses[3] and "error" in responses[4]
    assert "describe" in responses[5]["result"]["operations"]
    assert exchange(binary, b"\xff\n")[0]["id"] is None
    oversized = exchange(binary, b"x" * (1024 * 1024 + 1) + b"\n")
    assert "exceeds" in oversized[0]["error"]["message"]
    process = subprocess.Popen([binary], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    process.stdout.close()
    assert process.wait(timeout=10) != 0, "Broken stdout must terminate with an error"
    process.stdin.close()
    print("automation protocol smoke passed: framing, ordering, EOF, malformed/oversized input and broken stdout")


if __name__ == "__main__":
    main()
