#!/usr/bin/env python3
"""Verify the packaged icon catalog, asset provenance, and retained notices."""
import hashlib
import json
import sys
from pathlib import Path


def verify(root):
    catalog = [line.split() for line in (root / "catalog.tsv").read_text().splitlines()
               if line and not line.startswith("#")]
    assert all(len(row) == 3 for row in catalog), "Malformed icon catalog"
    keys = {row[0] for row in catalog}
    assert len(keys) == len(catalog), "Duplicate icon key"
    records = json.loads((root / "sources.json").read_text())
    assert {record["key"] for record in records} == keys, "Icon provenance/catalog mismatch"
    assert {path.stem for path in root.glob("*.svg")} == keys, "SVG/catalog mismatch"
    for record in records:
        data = (root / f"{record['key']}.svg").read_bytes()
        assert hashlib.sha256(data).hexdigest() == record["sha256"], record["key"]
        license_type = record.get("brand", {}).get("license", {}).get("type")
        if license_type and license_type != "CC0-1.0":
            filename = f"{record['key']}-MIT.txt" if license_type == "MIT" else f"{license_type}.txt"
            assert (root / "licenses" / filename).stat().st_size > 0, filename
    for name in ["lucide-LICENSE.txt", "simple-icons-LICENSE.md", "simple-icons-DISCLAIMER.md", "ATTRIBUTION.txt"]:
        assert (root / name).stat().st_size > 0, name
    for record in json.loads((root / "licenses/sources.json").read_text()):
        assert hashlib.sha256((root / "licenses" / record["file"]).read_bytes()).hexdigest() == record["sha256"], record["file"]


if __name__ == "__main__":
    verify(Path(sys.argv[1]))
