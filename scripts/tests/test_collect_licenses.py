"""Check exact-source notice retention without writing into Cargo's cache."""
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "collect_licenses", Path(__file__).resolve().parents[1] / "collect-licenses.py")
LICENSES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LICENSES)


class SupplementalNoticeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.repository = Path(temporary.name)
        self.source = self.repository / "registry"
        self.source.mkdir()
        self.output = self.repository / "package"
        self.package = {"name": "tree-sitter-example", "version": "1.0.0"}
        self.record = {"revision": "012345", "file": "example-LICENSE.txt",
                       "url": "https://example.invalid/012345/LICENSE"}
        self.notice = self.repository / "LICENSES/tree-sitter/example-LICENSE.txt"
        self.notice.parent.mkdir(parents=True)
        self.notice.write_bytes(b"Original copyright and permission notice\n")
        self.record["sha256"] = hashlib.sha256(self.notice.read_bytes()).hexdigest()
        self.vcs = self.source / ".cargo_vcs_info.json"
        self.vcs.write_text(json.dumps({"git": {"sha1": self.record["revision"]}}))

    def retain(self, records=None):
        if records is None:
            records = {(self.package["name"], self.package["version"]): self.record}
        return LICENSES.supplemental_notice(
            self.package, self.source, self.repository, self.output,
            Path("cargo/tree-sitter-example-1.0.0"), records)

    def test_retains_original_notice_and_source_provenance_without_cache_changes(self):
        before = self.vcs.read_bytes()
        path, provenance = self.retain()
        self.assertEqual((self.output / path).read_bytes(), self.notice.read_bytes())
        self.assertEqual(provenance["revision"], self.record["revision"])
        self.assertEqual(provenance["url"], self.record["url"])
        self.assertEqual(self.vcs.read_bytes(), before)
        self.assertEqual(list(self.source.iterdir()), [self.vcs])

    def test_refuses_missing_release_record_revision_mismatch_or_changed_copyright(self):
        with self.assertRaisesRegex(ValueError, "no packaged license"):
            self.retain({})
        self.vcs.write_text(json.dumps({"git": {"sha1": "different-release"}}))
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            self.retain()
        self.vcs.write_text(json.dumps({"git": {"sha1": self.record["revision"]}}))
        self.notice.write_bytes(b"changed notice")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.retain()
        self.assertFalse(self.output.exists())

    def test_native_notice_walk_ignores_build_and_vcs_caches(self):
        for directory in ["native", "target", ".git", "__pycache__"]:
            path = self.source / directory / "LICENSE"
            path.parent.mkdir()
            path.write_text("notice")
        notices = [path.relative_to(self.source).as_posix()
                   for path in LICENSES.source_files(self.source) if LICENSES.notice(path)]
        self.assertEqual(notices, ["native/LICENSE"])


if __name__ == "__main__":
    unittest.main()
