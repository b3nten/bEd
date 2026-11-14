"""Generate Bed's original, self-contained glTF GPU/importer fixtures."""
import base64
import json
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parent
faces = [
    ([0, 0, 1], [(-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]),
    ([1, 0, 0], [(1, -1, 1), (1, -1, -1), (1, 1, -1), (1, 1, 1)]),
    ([0, 0, -1], [(1, -1, -1), (-1, -1, -1), (-1, 1, -1), (1, 1, -1)]),
    ([-1, 0, 0], [(-1, -1, -1), (-1, -1, 1), (-1, 1, 1), (-1, 1, -1)]),
    ([0, 1, 0], [(-1, 1, 1), (1, 1, 1), (1, 1, -1), (-1, 1, -1)]),
    ([0, -1, 0], [(-1, -1, -1), (1, -1, -1), (1, -1, 1), (-1, -1, 1)]),
]


def chunk(kind, payload):
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload))


png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(bytes([
    0, 255, 180, 70, 255, 70, 150, 255, 255,
    0, 70, 150, 255, 255, 255, 180, 70, 255,
]))) + chunk(b"IEND", b"")

data = bytearray()
views = []


def append(payload):
    data.extend(b"\0" * (-len(data) % 4))
    views.append({"buffer": 0, "byteOffset": len(data), "byteLength": len(payload)})
    data.extend(payload)


append(struct.pack("<72f", *(v for _, corners in faces for p in corners for v in p)))
append(struct.pack("<72f", *(v for normal, _ in faces for _ in range(4) for v in normal)))
append(struct.pack("<48f", *([0, 1, 1, 1, 1, 0, 0, 0] * 6)))
append(struct.pack("<36H", *(i + face * 4 for face in range(6) for i in [0, 1, 2, 0, 2, 3])))
append(png)
asset = {
    "asset": {"version": "2.0", "generator": "Bed GPU viewer test fixture"},
    "scene": 0,
    "scenes": [{"nodes": [0]}],
    "nodes": [{"name": "Textured cube", "mesh": 0}],
    "buffers": [{"byteLength": len(data)}],
    "bufferViews": views,
    "accessors": [
        {"bufferView": 0, "componentType": 5126, "count": 24, "type": "VEC3", "min": [-1, -1, -1], "max": [1, 1, 1]},
        {"bufferView": 1, "componentType": 5126, "count": 24, "type": "VEC3"},
        {"bufferView": 2, "componentType": 5126, "count": 24, "type": "VEC2"},
        {"bufferView": 3, "componentType": 5123, "count": 36, "type": "SCALAR"},
    ],
    "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2}, "indices": 3, "material": 0}]}],
    "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 1}}],
    "textures": [{"source": 0}],
    "images": [{"bufferView": 4, "mimeType": "image/png"}],
}
json_bytes = json.dumps(asset, separators=(",", ":")).encode()
json_bytes += b" " * (-len(json_bytes) % 4)
binary = bytes(data) + b"\0" * (-len(data) % 4)
glb = struct.pack("<III", 0x46546C67, 2, 28 + len(json_bytes) + len(binary))
glb += struct.pack("<II", len(json_bytes), 0x4E4F534A) + json_bytes
glb += struct.pack("<II", len(binary), 0x004E4942) + binary
(ROOT / "cube.glb").write_bytes(glb)
asset["buffers"][0]["uri"] = "data:application/octet-stream;base64," + base64.b64encode(data).decode()
(ROOT / "cube.gltf").write_text(json.dumps(asset, indent=2) + "\n")
