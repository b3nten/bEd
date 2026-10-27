#!/usr/bin/env python3
"""Check native toolbar geometry and actual Metal transparent-window pixels."""
import argparse
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
import zlib

from automation import Application
from smoke_bed import ready


def rgba_rows(path):
    """Decode the renderer's noninterlaced RGBA8 PNG captures without dependencies."""
    data = Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    offset, packed = 8, bytearray()
    while offset < len(data):
        length, kind = struct.unpack_from(">I4s", data, offset)
        chunk = data[offset + 8:offset + 8 + length]
        offset += length + 12
        if kind == b"IHDR":
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", chunk)
            assert (depth, color, compression, filtering, interlace) == (8, 6, 0, 0, 0)
        elif kind == b"IDAT":
            packed.extend(chunk)
    raw = zlib.decompress(packed)
    stride = width * 4
    assert len(raw) == height * (stride + 1)
    previous = bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        mode = raw[start]
        row = bytearray(raw[start + 1:start + stride + 1])
        for x in range(stride):
            a = row[x - 4] if x >= 4 else 0
            b = previous[x]
            c = previous[x - 4] if x >= 4 else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            paeth = a if pa <= pb and pa <= pc else b if pb <= pc else c
            predictor = (0, a, b, (a + b) // 2, paeth)[mode]
            row[x] = (row[x] + predictor) % 256
        yield row
        previous = row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/bed")
    parser.add_argument("--output", default="target/bed-titlebar-smoke")
    args = parser.parse_args()
    assert sys.platform == "darwin", "This regression exercises AppKit/Metal composition"
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    previous_config = os.environ.get("XDG_CONFIG_HOME")
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix="config-", dir=output) as config:
            os.environ["XDG_CONFIG_HOME"] = config
            with Application([str(Path(args.binary).resolve())], output=output, timeout=60) as app:
                snapshot = ready(app)
                tools = app.request("app.inspect_titlebar")["tools"]
                window = snapshot["windows"][0]
                surface = snapshot["ui"]["surfaces"][0]
                zoom = surface["zoom"]
                scale = surface["pixels_per_point"]
                rects = sorted((tool["rect"] for tool in tools), key=lambda r: r["min"][0])
                assert {tool["key"] for tool in tools} == {
                    "bedtime.terminal", "bed.git", "bedtime.filetree", "bed.structure", "bed.settings"}
                for rect in rects:
                    assert abs(rect["max"][0] - rect["min"][0] - 26) < 0.01
                    assert abs(rect["max"][1] - rect["min"][1] - 22) < 0.01
                for a, b in zip(rects, rects[1:]):
                    assert abs(b["min"][0] - a["max"][0] - 2) < 0.01
                assert abs(window["size"][0] / zoom - rects[-1]["max"][0] - 10) < 0.01
                x, y = (round(v * scale) for v in rects[0]["min"])
                right, bottom = (round(v * scale) for v in rects[-1]["max"])
                for theme in ["Dark", "Light"]:
                    for opacity, blur in [(1.0, False), (0.6, False), (0.0, False), (0.6, True)]:
                        app.request("app.set_settings", values={
                            "theme": theme, "window_opacity": opacity, "window_blur": blur})
                        app.request("wait_frames", frames=30)
                        capture = app.capture(f"{theme}-{opacity}-blur-{blur}.png", crop=[x, y, right-x, bottom-y])
                        assert capture["nontransparent_pixels"] > 0, capture
                        pixels = [row[i:i+4] for row in rgba_rows(capture["path"]) for i in range(0, len(row), 4)]
                        # Display-premultiplied RGB cannot exceed coverage. Before
                        # the fix the toolbar exceeded alpha by up to 67/255.
                        assert max(max(p[:3]) - p[3] for p in pixels) <= 1, capture
                        if opacity == 0:
                            assert any(0 < p[3] < 255 for p in pixels), "No antialiased icon edges"
                            assert any(p[3] == 0 for p in pixels), "Window did not become transparent"
                        captures.append(capture)
            assert "ERROR" not in (output / "stderr.log").read_text(), "Renderer reported an error"
    finally:
        if previous_config is None:
            os.environ.pop("XDG_CONFIG_HOME", None)
        else:
            os.environ["XDG_CONFIG_HOME"] = previous_config
    (output / "summary.json").write_text(json.dumps({"captures": captures}, indent=2))
    print(f"Toolbar geometry and {len(captures)} GPU alpha captures passed")


if __name__ == "__main__":
    main()
