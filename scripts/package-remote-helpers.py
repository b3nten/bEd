#!/usr/bin/env python3
"""Stage and validate the static Linux helpers shipped with every Bed desktop."""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent
TARGETS = {"x86_64-unknown-linux-musl": 62, "aarch64-unknown-linux-musl": 183}
DEFAULT_SOURCE = REPOSITORY / "target" / "remote-helpers"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def protocol_version():
    source = (REPOSITORY / "crates/bed-remote/src/protocol.rs").read_text(encoding="utf-8")
    return int(re.search(r"pub const PROTOCOL_VERSION: u32 = (\d+);", source)[1])


def package_version():
    source = (REPOSITORY / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'^version = "([^"]+)"', source, re.MULTILINE)[1]


def validate_elf(binary, target):
    """Reject wrong-architecture or dynamically linked Linux executables."""
    data = binary.read_bytes()
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise ValueError(f"{binary}: expected a little-endian ELF64 executable")
    kind, machine = struct.unpack_from("<HH", data, 16)
    if kind not in (2, 3) or machine != TARGETS[target]:
        raise ValueError(f"{binary}: executable architecture does not match {target}")
    offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 54)
    if count == 0 or entry_size < 56 or offset + entry_size * count > len(data):
        raise ValueError(f"{binary}: invalid ELF program headers")
    for index in range(count):
        header = offset + index * entry_size
        segment = struct.unpack_from("<I", data, header)[0]
        start = struct.unpack_from("<Q", data, header + 8)[0]
        size = struct.unpack_from("<Q", data, header + 32)[0]
        if start + size > len(data):
            raise ValueError(f"{binary}: truncated ELF program segment")
        if segment == 3:  # PT_INTERP
            raise ValueError(f"{binary}: requires a dynamic loader; build a static musl helper")
        if segment == 2:  # Static PIE may have PT_DYNAMIC, but must have no DT_NEEDED.
            if size % 16 or start + size > len(data):
                raise ValueError(f"{binary}: invalid ELF dynamic segment")
            for cursor in range(start, start + size, 16):
                tag = struct.unpack_from("<q", data, cursor)[0]
                if tag == 0:
                    break
                if tag == 1:  # DT_NEEDED
                    raise ValueError(f"{binary}: depends on a shared library")
    section_offset = struct.unpack_from("<Q", data, 40)[0]
    section_size, section_count = struct.unpack_from("<HH", data, 58)
    if section_offset or section_count:
        if section_size < 64 or section_offset + section_size > len(data):
            raise ValueError(f"{binary}: truncated ELF section header table")
        if section_count == 0:  # Extended section count is stored in section zero's sh_size.
            section_count = struct.unpack_from("<Q", data, section_offset + 32)[0]
        if section_count == 0 or section_offset + section_size * section_count > len(data):
            raise ValueError(f"{binary}: truncated ELF section header table")
        for index in range(section_count):
            header = section_offset + index * section_size
            kind = struct.unpack_from("<I", data, header + 4)[0]
            if kind in (0, 8):  # SHT_NULL metadata and SHT_NOBITS allocate no file contents.
                continue
            start, size = struct.unpack_from("<QQ", data, header + 24)
            if start + size > len(data):
                raise ValueError(f"{binary}: truncated ELF section contents")


def validate_bundle(directory, target):
    binary = directory / "bed-headless"
    validate_elf(binary, target)
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    expected = {"format_version": 1, "target": target, "version": package_version(),
                "protocol_version": protocol_version(), "sha256": digest(binary)}
    for key, value in expected.items():
        if manifest.get(key) != value:
            raise ValueError(f"{directory}: manifest {key} does not match this Bed release")
    for name in ("LICENSE", "NOTICE", "UPSTREAM_REVISION", "LICENSES/musl-COPYRIGHT.txt",
                 "LICENSES/dependencies/index.json"):
        if not (directory / name).is_file():
            raise ValueError(f"{directory}: missing license/provenance file {name}")


def validate(source, targets):
    for target in targets:
        validate_bundle(source / target, target)


def revision():
    if os.environ.get("GITHUB_SHA"):
        return os.environ["GITHUB_SHA"]
    result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPOSITORY,
                            text=True, capture_output=True, check=False)
    return result.stdout.strip() if result.returncode == 0 else None


def stage(target, binary, output):
    validate_elf(binary, target)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".remote-helper-", dir=output) as temporary:
        directory = Path(temporary) / target
        directory.mkdir()
        shutil.copy2(binary, directory / "bed-headless")
        (directory / "bed-headless").chmod(0o755)
        for name in ("LICENSE", "NOTICE", "UPSTREAM_REVISION"):
            shutil.copy2(REPOSITORY / name, directory / name)
        shutil.copytree(REPOSITORY / "LICENSES", directory / "LICENSES")
        subprocess.run([sys.executable, str(REPOSITORY / "scripts/collect-licenses.py"),
                        str(directory)], cwd=REPOSITORY, check=True)
        manifest = {"format_version": 1, "target": target, "version": package_version(),
                    "protocol_version": protocol_version(), "sha256": digest(binary),
                    "source_revision": revision(), "cargo_lock_sha256": digest(REPOSITORY / "Cargo.lock"),
                    "upstream_revision": (REPOSITORY / "UPSTREAM_REVISION").read_text().strip()}
        (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        with (directory / "NOTICE").open("a", encoding="utf-8") as notice:
            notice.write("\nThis bed-headless helper is statically linked for " + target + ".\n"
                         "Dependency notices cover the resolved Bed workspace; this helper uses a subset.\n"
                         "manifest.json records its checksum, protocol, release and source provenance.\n")
        validate_bundle(directory, target)
        destination = output / target
        if destination.exists():
            shutil.rmtree(destination)
        shutil.move(str(directory), destination)
    print(f"Staged static remote helper: {destination}")


def copy(source, destination):
    validate(source, TARGETS)
    output = destination / "remote-helpers"
    output.mkdir(parents=True, exist_ok=True)
    for target in TARGETS:
        shutil.copytree(source / target, output / target, dirs_exist_ok=True)
        # CI artifact downloads and Windows archives do not retain POSIX modes.
        (output / target / "bed-headless").chmod(0o755)
    validate(output, TARGETS)
    print(f"Packaged remote helpers: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    staging = commands.add_parser("stage", help="stage one native musl release build with notices")
    staging.add_argument("--target", choices=TARGETS, required=True)
    staging.add_argument("--binary", type=Path)
    staging.add_argument("--output", type=Path, default=DEFAULT_SOURCE)
    validation = commands.add_parser("validate", help="validate both bundled targets by default")
    validation.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    validation.add_argument("--target", choices=TARGETS, action="append")
    copying = commands.add_parser("copy", help="copy both validated helpers into desktop resources")
    copying.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    copying.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "stage":
            binary = args.binary or REPOSITORY / "target" / args.target / "release/bed-headless"
            stage(args.target, binary, args.output)
        elif args.command == "validate":
            validate(args.source, args.target or TARGETS)
            print("Remote helper bundle validation passed.")
        else:
            copy(args.source, args.destination)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Remote helper packaging failed: {error}\n")


if __name__ == "__main__":
    main()
