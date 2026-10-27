#!/usr/bin/env python3
"""Check corner splits and joins against visible panel corners at changing gaps."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from automation import Application
from smoke_bed import ready, session


def areas(snapshot):
    return next(layout["areas"] for layout in session(snapshot)["layouts"] if layout["active"])


def corner(surface, high, inset=6.0):
    return [surface["max" if high[i] else "min"][i] + (-inset if high[i] else inset)
            for i in range(2)]


def drag(app, snapshot, start, end):
    zoom = snapshot["ui"]["surfaces"][0]["zoom"]
    app.input({"type": "drag", "from": [v * zoom for v in start],
               "to": [v * zoom for v in end], "frames": 10})
    app.request("wait_frames", frames=30)
    return app.inspect()


def split_and_join(app, source, high, *, halo=False):
    before = app.inspect()
    original = next(a for a in areas(before) if a["entity"] == source)
    previous = {a["entity"] for a in areas(before)}
    start = corner(original["surface"], high)
    if halo:
        # egui's interaction radius also offers the handle just into the gutter.
        # Stay near the corner tip, clear of the divider's direct resize target.
        start = corner(original["surface"], high, inset=1.0)
        start[0] = original["surface"]["max" if high[0] else "min"][0] + (1 if high[0] else -1)
    end = [(a + b) / 2 for a, b in zip(original["surface"]["min"], original["surface"]["max"])]
    after = drag(app, before, start, end)
    created = [a for a in areas(after) if a["entity"] not in previous]
    assert len(areas(after)) == len(previous) + 1 and len(created) == 1, (high, halo, start, areas(after))
    retained = next(a for a in areas(after) if a["entity"] == source)
    split_axis = next(i for i in range(2) if retained["rect"]["max"][i] != original["rect"]["max"][i])
    join_corner = list(high)
    join_corner[split_axis] = True
    start = corner(retained["surface"], join_corner)
    end = start.copy()
    end[split_axis] = created[0]["surface"]["min"][split_axis] + 20
    joined = drag(app, after, start, end)
    assert {a["entity"] for a in areas(joined)} == previous, (high, halo, areas(joined))
    restored = next(a for a in areas(joined) if a["entity"] == source)
    assert restored["rect"] == original["rect"], (restored, original)
    assert [p["entity"] for p in restored["panels"]] == [p["entity"] for p in original["panels"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/bed-corner-smoke")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    previous_config = os.environ.get("XDG_CONFIG_HOME")
    checked = []
    try:
        with tempfile.TemporaryDirectory(prefix="config-", dir=output) as config:
            os.environ["XDG_CONFIG_HOME"] = config
            with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
                snapshot = ready(app)
                source = areas(snapshot)[0]["entity"]
                right = app.request("area.split", area=source, axis="horizontal")["area"]
                app.request("area.split", area=source, axis="vertical")
                app.request("area.split", area=right, axis="vertical")
                app.request("wait_frames", frames=60)
                for gap in [0.0, 5.0, 16.0, 32.0]:
                    app.request("app.set_settings", values={"panel_gap": gap})
                    app.request("wait_frames", frames=5)
                    for high in [(False, False), (True, False), (False, True), (True, True)]:
                        split_and_join(app, source, high)
                        checked.append({"gap": gap, "corner": high, "halo": False})
                        if gap > 0:
                            split_and_join(app, source, high, halo=True)
                            checked.append({"gap": gap, "corner": high, "halo": True})
                capture = app.capture("corners-large-gap.png")
                assert capture["nontransparent_pixels"] > 0, capture
        result = {"passed": True, "split_join_cases": checked, "capture": capture}
        (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"Corner smoke passed: {len(checked)} split/join cases in {output}")
    finally:
        if previous_config is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous_config


if __name__ == "__main__":
    main()
