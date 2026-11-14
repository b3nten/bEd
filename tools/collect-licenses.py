#!/usr/bin/env python3
"""Gather the notices a bEd package carries: the repository's NOTICE, every
Cargo dependency's license files, and the notices kept beside embedded assets
(fonts, icons, grammars, themes), vendored forks and adapted source. Writes
NOTICE and license-index.json into the destination folder.

    python3 tools/collect-licenses.py <destination> [--target <triple>]
"""
import argparse
import json
import os
import subprocess
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parent.parent
# Where bEd keeps the notices of what it embeds or adapts.
ROOTS = ("bed", "bedtime/crates", "vendor", "licenses")
SKIPPED = {"target", ".git", "__pycache__", "tests", "fixtures", "node_modules"}


def notice(path):
    name = path.name.upper()
    return (any(word in name for word in ("LICENSE", "LICENCE", "NOTICE", "COPYING", "COPYRIGHT", "OFL"))
            or path.name in {"BED_PATCHES.md", "UPSTREAM.md", "sources.json"})


def files(directory):
    for root, directories, names in os.walk(directory):
        directories[:] = sorted(name for name in directories if name not in SKIPPED)
        for name in sorted(names):
            path = Path(root) / name
            if path.is_file():
                yield path


def section(name, contents):
    return (f"\n=== {name} ===\n--- Original notice ---\n".encode()
            + contents + b"\n--- End of original notice ---\n")


def collect(destination, target=None):
    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if target:
        command += ["--filter-platform", target]
    metadata = json.loads(subprocess.check_output(command, cwd=REPOSITORY))
    bundle = bytearray((REPOSITORY / "NOTICE").read_bytes())
    index = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] in metadata["workspace_members"]:
            continue
        source = Path(package["manifest_path"]).parent
        found = {path for path in files(source) if notice(path) and path.parent == source}
        if package.get("license_file") and (source / package["license_file"]).is_file():
            found.add(source / package["license_file"])
        sections = []
        for path in sorted(found):
            name = f"cargo/{package['name']}-{package['version']}/{path.relative_to(source).as_posix()}"
            bundle += section(name, path.read_bytes())
            sections.append(name)
        entry = {"name": package["name"], "version": package["version"],
                 "license": package.get("license"), "notices": sections,
                 "repository": package.get("repository")}
        if (package.get("source") or "").startswith(("registry+", "sparse+")):
            entry["source_archive_url"] = (
                f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download")
        index.append(entry)
    for root in ROOTS:
        for path in files(REPOSITORY / root):
            if notice(path):
                bundle += section(path.relative_to(REPOSITORY).as_posix(), path.read_bytes())
    bundle += (b"\nDependency and asset notices are retained verbatim above. license-index.json\n"
               b"lists each Cargo package's license, its NOTICE sections and its source archive.\n")
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "NOTICE").write_bytes(bundle)
    (destination / "license-index.json").write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")
    missing = sum(1 for entry in index if not entry["notices"])
    print(f"Notices for {len(index)} Cargo packages ({missing} ship no license file; see license-index.json).")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--target", help="Only dependencies built for this target")
    arguments = parser.parse_args()
    collect(arguments.destination.resolve(), arguments.target)
