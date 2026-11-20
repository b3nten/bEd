"""The frame rate in the status bar: off by default, shown by its setting,
with frame times on hover."""
import re

from bed import Bed

with Bed() as bed:
    bed.wait(10)
    assert not bed.find_all(contains=" fps"), "off by default"

with Bed(settings={"status.frame_rate": True}) as bed:
    bed.wait(30)
    # Its text, then its rows (for automation and screen readers).
    counter = bed.find(contains=" fps")
    label = counter.get("label") or counter.get("value")
    assert re.match(r"\d+ fps\n", label), label
    x = sum(counter["rect"][0::2]) / 2
    y = sum(counter["rect"][1::2]) / 2
    bed.move(x, y)
    bed.wait(60)
    assert bed.exists("Update + render"), "the tooltip lists frame times"
    bed.screenshot("frame-rate-tooltip")
    assert not bed.status()["errors"]
    log = open(bed.log_path).read()
    assert "ERROR" not in log and "panicked" not in log, log[-2000:]
