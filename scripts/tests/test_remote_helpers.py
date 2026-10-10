"""Portable release checks: architecture, shared-library independence and integrity."""
import importlib.util
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "remote_helpers", Path(__file__).resolve().parents[1] / "lib/remote-helpers.py")
HELPERS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)


def elf(machine, segment=1, dynamic_tag=0):
    data = bytearray(136)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<HH", data, 16, 3, machine)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HH", data, 54, 56, 1)
    struct.pack_into("<I", data, 64, segment)
    if segment == 2:
        struct.pack_into("<Q", data, 72, 120)
        struct.pack_into("<Q", data, 96, 16)
        struct.pack_into("<qQ", data, 120, dynamic_tag, 0)
    return bytes(data)


def bundle(source, target):
    directory = source / target
    directory.mkdir()
    binary = directory / "bed-headless"
    binary.write_bytes(elf(HELPERS.TARGETS[target]))
    manifest = {"format_version": 1, "target": target, "version": HELPERS.package_version(),
                "protocol_version": HELPERS.protocol_version(), "sha256": HELPERS.digest(binary)}
    (directory / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    for name in ("LICENSE", "NOTICE", "license-index.json"):
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("notice\n=== musl ===\n" if name == "NOTICE" else "notice", encoding="utf-8")
    return directory


class StaticHelperTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_accepts_static_executables_and_static_pie_on_both_architectures(self):
        for target, machine in HELPERS.TARGETS.items():
            binary = self.root / target
            for segment in (1, 2):
                binary.write_bytes(elf(machine, segment))
                HELPERS.validate_elf(binary, target)

    def test_rejects_dynamic_loader_and_shared_library_dependencies(self):
        binary = self.root / "helper"
        for data in (elf(183, 3), elf(183, 2, 1)):
            binary.write_bytes(data)
            with self.assertRaises(ValueError):
                HELPERS.validate_elf(binary, "aarch64-unknown-linux-musl")

    def test_rejects_wrong_architecture_and_truncated_headers(self):
        binary = self.root / "helper"
        for data in (elf(62), elf(183)[:100], b"MZ-not-a-Linux-helper"):
            binary.write_bytes(data)
            with self.assertRaises(ValueError):
                HELPERS.validate_elf(binary, "aarch64-unknown-linux-musl")

    def test_rejects_valid_elf_prefix_missing_segments_or_section_trailer(self):
        binary = self.root / "helper"
        missing_segment = bytearray(elf(183))
        struct.pack_into("<Q", missing_segment, 96, len(missing_segment) + 1)
        missing_table = bytearray(elf(183))
        struct.pack_into("<Q", missing_table, 40, len(missing_table))
        struct.pack_into("<HH", missing_table, 58, 64, 1)
        missing_section = bytearray(elf(183)) + bytearray(64)
        struct.pack_into("<Q", missing_section, 40, 136)
        struct.pack_into("<HH", missing_section, 58, 64, 1)
        struct.pack_into("<I", missing_section, 140, 1)
        struct.pack_into("<QQ", missing_section, 160, len(missing_section), 1)
        for data in (missing_segment, missing_table, missing_section):
            binary.write_bytes(data)
            with self.assertRaisesRegex(ValueError, "truncated ELF"):
                HELPERS.validate_elf(binary, "aarch64-unknown-linux-musl")

    def test_missing_target_damaged_binary_and_missing_notices_fail_validation(self):
        target = "aarch64-unknown-linux-musl"
        directory = bundle(self.root, target)
        HELPERS.validate(self.root, [target])
        with self.assertRaisesRegex(ValueError, "Missing prebuilt remote helper bundle"):
            HELPERS.validate(self.root, HELPERS.TARGETS)
        binary = directory / "bed-headless"
        original = binary.read_bytes()
        binary.write_bytes(original + b"changed")
        with self.assertRaisesRegex(ValueError, "sha256"):
            HELPERS.validate(self.root, [target])
        binary.write_bytes(original)
        notice = directory / "NOTICE"
        original_notice = notice.read_bytes()
        notice.write_text("Missing musl notice\n")
        with self.assertRaisesRegex(ValueError, "musl notice"):
            HELPERS.validate(self.root, [target])
        notice.write_bytes(original_notice)
        (directory / "license-index.json").unlink()
        with self.assertRaisesRegex(ValueError, "license-index.json"):
            HELPERS.validate(self.root, [target])

    def test_manifest_rejects_mismatched_protocol(self):
        target = "aarch64-unknown-linux-musl"
        directory = bundle(self.root, target)
        path = directory / "manifest.json"
        manifest = json.loads(path.read_text())
        manifest["protocol_version"] += 1
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "protocol_version"):
            HELPERS.validate(self.root, [target])

    def test_missing_bundles_cli_reports_packaging_prerequisites_without_errno(self):
        result = subprocess.run(
            [sys.executable, str(Path(HELPERS.__file__)), "validate", "--source", str(self.root)],
            text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Building the desktop binary does not build or stage", result.stderr)
        self.assertIn("bash scripts/lib/build-remote-helpers.sh", result.stderr)
        self.assertIn("requires running Docker", result.stderr)
        self.assertIn("C compiler run only in Linux containers", result.stderr)
        self.assertIn("matching Bed release", result.stderr)
        self.assertIn("--source PATH", result.stderr)
        self.assertNotIn("[Errno", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        for target in HELPERS.TARGETS:
            self.assertIn(str(self.root / target / "bed-headless"), result.stderr)
            self.assertIn(f"bed-remote-helper-{target}", result.stderr)

    def test_partial_bundle_copy_fails_before_creating_desktop_resources(self):
        bundle(self.root, "aarch64-unknown-linux-musl")
        resources = self.root / "desktop-resources"
        with self.assertRaises(ValueError) as raised:
            HELPERS.copy(self.root, resources)
        self.assertIn(str(self.root / "x86_64-unknown-linux-musl" / "bed-headless"),
                      str(raised.exception))
        self.assertFalse(resources.exists())

    def test_missing_manifest_reports_the_incomplete_bundle(self):
        target = "aarch64-unknown-linux-musl"
        directory = bundle(self.root, target)
        (directory / "manifest.json").unlink()
        with self.assertRaisesRegex(ValueError, "manifest.json") as raised:
            HELPERS.validate(self.root, [target])
        self.assertIn("manifest and license files", str(raised.exception))

    def test_resource_copy_keeps_both_targets_and_restores_executable_modes(self):
        for target in HELPERS.TARGETS:
            directory = bundle(self.root, target)
            (directory / "bed-headless").chmod(0o644)
        resources = self.root / "desktop-resources"
        HELPERS.copy(self.root, resources)
        HELPERS.validate(resources / "remote-helpers", HELPERS.TARGETS)
        for target in HELPERS.TARGETS:
            self.assertTrue((resources / "remote-helpers" / target / "bed-headless").stat().st_mode & 0o111)


if __name__ == "__main__":
    unittest.main()
