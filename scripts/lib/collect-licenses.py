#!/usr/bin/env python3
"""Retain available Cargo/native/grammar license notices in Bed packages."""
import hashlib
import json
import os
import shutil
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


def supplemental_notice(package, source, repository, output, supplements):
    record = supplements.get((package["name"], package["version"]))
    if record is None:
        raise ValueError(f"{package['name']} {package['version']} has no packaged license notice; "
                         "retain the exact published revision's notice in LICENSES/tree-sitter-NOTICES.txt first")
    vcs = json.loads((source / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
    if vcs["git"]["sha1"] != record["revision"]:
        raise ValueError(f"Supplemental notice revision mismatch for {package['name']}")
    notice_path = repository / "LICENSES" / record["file"]
    if hashlib.sha256(notice_path.read_bytes()).hexdigest() != record["sha256"]:
        raise ValueError(f"Supplemental notice checksum mismatch for {package['name']}")
    relative = Path("supplemental") / record["file"]
    target = output / relative
    if not target.is_file() or hashlib.sha256(target.read_bytes()).hexdigest() != record["sha256"]:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(notice_path, target)
    return relative.as_posix(), {"notice": relative.as_posix(), "url": record["url"],
                                "revision": record["revision"],
                                "original_notice_sha256": record["original_notice_sha256"],
                                "bundle_sha256": record["sha256"]}


def collect(destination):
    repository = Path(__file__).resolve().parents[2]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=repository,
    ))
    # Supplemental bundles are copied once below, after their source is verified.
    shutil.copytree(repository / "LICENSES", destination / "LICENSES", dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns("tree-sitter-NOTICES.txt"))
    output = destination / "LICENSES" / "dependencies"
    output.mkdir(parents=True, exist_ok=True)
    records = json.loads((repository / "LICENSES/tree-sitter-sources.json").read_text(encoding="utf-8"))
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
        copied = []
        for path in sorted(files):
            relative = directory / path.relative_to(source)
            target = output / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            copied.append(relative.as_posix())
        entry = {"name": package["name"], "version": package["version"],
                 "license": package.get("license"), "notices": copied,
                 "repository": package.get("repository")}
        if package.get("source") in {
            "registry+https://github.com/rust-lang/crates.io-index",
            "sparse+https://index.crates.io/",
        }:
            entry["source_archive_url"] = (
                f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"
            )
        if not copied and package["name"].startswith("tree-sitter"):
            relative, provenance = supplemental_notice(
                package, source, repository, output, supplements)
            copied.append(relative)
            entry["notice_provenance"] = [provenance]
        index.append(entry)
    # Patched native dependencies have notices below crate roots;
    # keep their path hierarchy rather than flattening conflicting filenames.
    for path in sorted(source_files(repository / "vendor")):
        if notice(path):
            target = output / path.relative_to(repository)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
    (output / "index.json").write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")
    with (destination / "NOTICE").open("a", encoding="utf-8") as notice_file:
        notice_file.write("\nPackaged license layout: vendor-relative notices above are retained under\n"
                          "LICENSES/dependencies/vendor/ with their original path hierarchy. Resolved\n"
                          "Cargo dependency notices and license metadata are retained under\n"
                          "LICENSES/dependencies/cargo/ and LICENSES/dependencies/index.json.\n"
                          "Supplemental Tree-sitter notices from LICENSES/tree-sitter-NOTICES.txt\n"
                          "are packaged at LICENSES/dependencies/supplemental/tree-sitter-NOTICES.txt.\n"
                          "Source archive URLs for crates.io dependencies are listed in\n"
                          "LICENSES/dependencies/index.json.\n")
    print(f"Retained license metadata/notices for {len(index)} resolved Cargo packages.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: collect-licenses.py PACKAGE_RESOURCE_ROOT")
    collect(Path(sys.argv[1]).resolve())
