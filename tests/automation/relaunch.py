"""Relaunching builds a new App in the same window: same size, the workspace's
layout restored, and quickly."""
import tempfile
import time
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    bed.invoke("app.open_settings")
    window = bed.status()["window"]
    for attempt in range(3):
        frame = bed.status()["frame"]
        start = time.monotonic()
        bed.call("bedtime.invoke", {"command": "app.relaunch"})
        # The frame count restarts with the new App.
        while True:
            assert time.monotonic() - start < 10, "the new App didn't start drawing"
            status = bed.status()
            if status["frame"] < frame and status["frame"] >= 3 and bed.widgets():
                break
            time.sleep(0.01)
        took = time.monotonic() - start
        assert took < 1, f"relaunch took {took:.2f}s"
        assert status["window"] == window, f"window changed: {status['window']} != {window}"
        assert bed.exists("Appearance"), "the layout wasn't restored"
        assert not status["errors"], status["errors"]
    bed.screenshot("relaunch")
