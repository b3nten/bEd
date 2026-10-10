#!/usr/bin/env python3
"""Retain available Cargo/native/grammar license notices in Bed packages."""
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path


def notice(path):
    name = path.name.upper()
    return name.startswith(("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")) or name in {"FTL.TXT", "BED_PATCHES.MD"}


def source_files(directory):
    for root, directories, files in os.walk(directory):
        directories[:] = [name for name in directories if name not in {"target", ".git", "__pycache__"}]
        for name in files:
            path = Path(root) / name
            if path.is_file():
                yield path


def notice_section(section, contents):
    return (f"\n=== {section} ===\n--- Original notice ---\n".encode()
            + contents + b"\n--- End of original notice ---\n")


def original_notice(bundle, section):
    heading = f"=== {section} ===\n".encode()
    if bundle.count(heading) != 1:
        raise ValueError(f"Missing or duplicate NOTICE section: {section}")
    contents = bundle.split(heading, 1)[1].split(b"\n=== ", 1)[0]
    opening = b"--- Original notice ---\n"
    closing = b"\n--- End of original notice ---"
    if contents.count(opening) != 1 or contents.count(closing) != 1:
        raise ValueError(f"Incomplete NOTICE section: {section}")
    start = contents.index(opening) + len(opening)
    end = contents.find(closing, start)
    if end == -1:
        raise ValueError(f"Incomplete NOTICE section: {section}")
    return bytes(contents[start:end])


def supplemental_notice(package, source, bundle, supplements):
    record = supplements.get((package["name"], package["version"]))
    if record is None:
        raise ValueError(f"{package['name']} {package['version']} has no packaged license notice; "
                         "retain the exact published revision's notice in NOTICE and record its source "
                         "in scripts/lib/tree-sitter-sources.json first")
    vcs = json.loads((source / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
    if vcs["git"]["sha1"] != record["revision"]:
        raise ValueError(f"Supplemental notice revision mismatch for {package['name']}")
    section = f"{package['name']} {package['version']}"
    contents = original_notice(bundle, section)
    if hashlib.sha256(contents).hexdigest() != record["original_notice_sha256"]:
        raise ValueError(f"Supplemental notice checksum mismatch for {package['name']}")
    return section, {"notice": "NOTICE", "section": section, "url": record["url"],
                     "revision": record["revision"],
                     "original_notice_sha256": record["original_notice_sha256"]}


def collect(destination):
    repository = Path(__file__).resolve().parents[2]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=repository,
    ))
    # Always start from the committed notices so recollection does not duplicate sections.
    bundle = bytearray((repository / "NOTICE").read_bytes())
    records = json.loads((repository / "scripts/lib/tree-sitter-sources.json").read_text(encoding="utf-8"))
    supplements = {(record["name"], record["version"]): record for record in records}
    index = []
    for package in metadata["packages"]:
        if package["id"] in metadata["workspace_members"]:
            continue
        source = Path(package["manifest_path"]).parent
        files = {path for path in source_files(source) if notice(path)}
        if package.get("license_file"):
            path = source / package["license_file"]
            if path.is_file():
                files.add(path)
        directory = Path("cargo") / (package["name"] + "-" + package["version"])
        sections = []
        for path in sorted(files):
            section = (directory / path.relative_to(source)).as_posix()
            bundle += notice_section(section, path.read_bytes())
            sections.append(section)
        entry = {"name": package["name"], "version": package["version"],
                 "license": package.get("license"), "notice_file": "NOTICE", "notices": sections,
                 "repository": package.get("repository")}
        if package.get("source") in {
            "registry+https://github.com/rust-lang/crates.io-index",
            "sparse+https://index.crates.io/",
        }:
            entry["source_archive_url"] = (
                f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"
            )
        if not sections and package["name"].startswith("tree-sitter"):
            section, provenance = supplemental_notice(package, source, bundle, supplements)
            sections.append(section)
            entry["notice_provenance"] = [provenance]
        index.append(entry)
    # Retain original paths in section headings to distinguish native notices.
    for path in sorted(source_files(repository / "vendor")):
        if notice(path):
            bundle += notice_section(path.relative_to(repository).as_posix(), path.read_bytes())
    bundle += (b"\nPackaged dependency notices are retained verbatim in the sections above.\n"
               b"license-index.json lists Cargo license metadata, NOTICE section names,\n"
               b"supplemental source provenance and crates.io source archive URLs.\n")
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "NOTICE").write_bytes(bundle)
    (destination / "license-index.json").write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")
    print(f"Retained license metadata/notices for {len(index)} resolved Cargo packages.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: collect-licenses.py PACKAGE_RESOURCE_ROOT")
    collect(Path(sys.argv[1]).resolve())
