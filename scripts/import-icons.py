#!/usr/bin/env python3
"""Import the pinned SVG subset and notices from cached upstream archives.

Download these exact archives into target/icon-sources before running:
https://codeload.github.com/simple-icons/simple-icons/tar.gz/98820a4dc8c363ca72fa2c0d294ea4a0a9bba75d
https://codeload.github.com/lucide-icons/lucide/tar.gz/a04f228cd01185e09c188b7227b9600c08c565ec
The SVG bytes remain unchanged; Bed applies monochrome tinting at render time.
"""
import hashlib
import json
import re
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "resources/icons"
SOURCES = {
    "simple-icons": ("simple-icons/simple-icons", "98820a4dc8c363ca72fa2c0d294ea4a0a9bba75d"),
    "lucide": ("lucide-icons/lucide", "a04f228cd01185e09c188b7227b9600c08c565ec"),
}


def main():
    catalog = [line.split() for line in (DESTINATION / "catalog.tsv").read_text().splitlines()
               if line and not line.startswith("#")]
    assert all(len(row) == 3 for row in catalog)
    assert len({row[0] for row in catalog}) == len(catalog)
    # Stage everything in memory before replacing any existing assets.
    output = {}
    records = []
    for upstream, (repository, revision) in SOURCES.items():
        archive = ROOT / f"target/icon-sources/{upstream}-{revision}.tar.gz"
        with tarfile.open(archive) as source:
            members = {member.name.split("/", 1)[1]: member for member in source.getmembers()
                       if "/" in member.name and member.isfile()}

            def read(name):
                return source.extractfile(members[name]).read()

            metadata = {}
            if upstream == "simple-icons":
                titles = {entry["title"]: entry for entry in json.loads(read("data/simple-icons.json"))}
                for title, slug in re.findall(r"\| `(.+?)` \| `(.+?)` \|", read("slugs.md").decode()):
                    metadata[slug] = titles[title]
                output["simple-icons-LICENSE.md"] = read("LICENSE.md")
                output["simple-icons-DISCLAIMER.md"] = read("DISCLAIMER.md")
            else:
                output["lucide-LICENSE.txt"] = read("LICENSE")
            for key, selected_source, slug in catalog:
                if selected_source != upstream:
                    continue
                data = read(f"icons/{slug}.svg")
                if upstream == "simple-icons":
                    license_type = metadata[slug].get("license", {}).get("type", "")
                    if "-NC-" in license_type or license_type == "custom":
                        raise ValueError(f"Use a Lucide category for {slug}: {license_type}")
                output[f"{key}.svg"] = data
                records.append({
                    "key": key,
                    "upstream": upstream,
                    "revision": revision,
                    "url": f"https://github.com/{repository}/blob/{revision}/icons/{slug}.svg",
                    "sha256": hashlib.sha256(data).hexdigest(),
                    **({"brand": metadata[slug]} if upstream == "simple-icons" else {}),
                })
    for path in DESTINATION.glob("*.svg"):
        if path.name not in output:
            path.unlink()
    for filename, contents in output.items():
        (DESTINATION / filename).write_bytes(contents)
    (DESTINATION / "sources.json").write_text(json.dumps(records, indent=2, ensure_ascii=False) + "\n")
    print(f"Imported {len(catalog)} icons and upstream notices.")


if __name__ == "__main__":
    main()
