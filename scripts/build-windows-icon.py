#!/usr/bin/env python3
"""Package Bed's existing PNG icon frames in a Windows ICO without changing pixels."""
import struct
from pathlib import Path


def build(source, destination):
    data = source.read_bytes()
    if data[:4] != b"icns" or len(data) < 8 or struct.unpack_from(">I", data, 4)[0] != len(data):
        raise ValueError("Invalid source ICNS header")
    frames = {}
    cursor = 8
    while cursor < len(data):
        if cursor + 8 > len(data):
            raise ValueError("Truncated ICNS chunk header")
        size = struct.unpack_from(">I", data, cursor + 4)[0]
        if size < 8 or cursor + size > len(data):
            raise ValueError("Invalid ICNS chunk size")
        png = data[cursor + 8:cursor + size]
        if png.startswith(b"\x89PNG\r\n\x1a\n"):
            width, height = struct.unpack_from(">II", png, 16)
            if width == height and width in (16, 32, 48, 64, 128, 256):
                frames.setdefault(width, png)
        cursor += size
    if not frames or 256 not in frames:
        raise ValueError("ICNS must contain PNG frames including 256 x 256")
    header = struct.pack("<HHH", 0, 1, len(frames))
    directory = bytearray()
    payloads = bytearray()
    offset = len(header) + 16 * len(frames)
    for width, png in sorted(frames.items()):
        directory.extend(struct.pack("<BBBBHHII", width % 256, width % 256, 0, 0,
                                     1, 32, len(png), offset))
        payloads.extend(png)
        offset += len(png)
    destination.write_bytes(header + directory + payloads)
    print(f"Packaged {len(frames)} unchanged PNG frames: {destination}")


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    build(root / "resources/icons/bed.icns", root / "resources/icons/bed.ico")
