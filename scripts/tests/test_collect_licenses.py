"""Check exact-source notice retention without writing into Cargo's cache."""
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "collect_licenses", Path(__file__).resolve().parents[1] / "lib/collect-licenses.py")
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
        self.record = {"revision": "012345",
                       "url": "https://example.invalid/012345/LICENSE"}
        self.notice = self.repository / "NOTICE"
        self.contents = b"Original copyright and permission notice\n"
        self.section = "tree-sitter-example 1.0.0"
        self.notice.write_bytes(LICENSES.notice_section(self.section, self.contents))
        self.record["original_notice_sha256"] = hashlib.sha256(self.contents).hexdigest()
        self.vcs = self.source / ".cargo_vcs_info.json"
        self.vcs.write_text(json.dumps({"git": {"sha1": self.record["revision"]}}))

    def retain(self, records=None):
        if records is None:
            records = {(self.package["name"], self.package["version"]): self.record}
        return LICENSES.supplemental_notice(
            self.package, self.source, self.notice.read_bytes(), records)

    def test_retains_original_notice_and_source_provenance_without_cache_changes(self):
        before = self.vcs.read_bytes()
        section, provenance = self.retain()
        self.assertEqual(LICENSES.original_notice(self.notice.read_bytes(), section), self.contents)
        self.assertEqual(provenance["revision"], self.record["revision"])
        self.assertEqual(provenance["url"], self.record["url"])
        self.assertEqual(provenance["original_notice_sha256"], self.record["original_notice_sha256"])
        self.assertEqual(provenance["notice"], "NOTICE")
        self.assertEqual(provenance["section"], self.section)
        self.assertEqual(section, self.section)
        self.assertEqual(self.vcs.read_bytes(), before)
        self.assertEqual(list(self.source.iterdir()), [self.vcs])

    def test_refuses_missing_release_record_revision_mismatch_or_changed_copyright(self):
        with self.assertRaisesRegex(ValueError, "no packaged license"):
            self.retain({})
        self.vcs.write_text(json.dumps({"git": {"sha1": "different-release"}}))
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            self.retain()
        self.vcs.write_text(json.dumps({"git": {"sha1": self.record["revision"]}}))
        self.notice.write_bytes(LICENSES.notice_section(self.section, b"changed notice"))
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.retain()
        self.assertFalse(self.output.exists())

    def test_refuses_missing_notice_file(self):
        self.notice.unlink()
        with self.assertRaises(FileNotFoundError):
            self.retain()
        self.assertFalse(self.output.exists())

    def test_collector_retains_each_supplement_once_and_references_its_section(self):
        second_source = self.repository / "second-registry-package"
        second_source.mkdir()
        (second_source / ".cargo_vcs_info.json").write_bytes(self.vcs.read_bytes())
        second_package = {"name": "tree-sitter-second", "version": "2.0.0"}
        with self.notice.open("ab") as bundle:
            bundle.write(LICENSES.notice_section("tree-sitter-second 2.0.0", self.contents))
        records = [dict(self.record, **package) for package in (self.package, second_package)]
        supplements = self.repository / "scripts/lib/tree-sitter-sources.json"
        supplements.parent.mkdir(parents=True)
        supplements.write_text(json.dumps(records))
        packages = [dict(package, id=package["name"], license="MIT", manifest_path=str(source / "Cargo.toml"))
                    for package, source in ((self.package, self.source), (second_package, second_source))]
        metadata = {"packages": packages, "workspace_members": []}
        script = self.repository / "scripts/lib/collect-licenses.py"
        with patch.object(LICENSES, "__file__", str(script)), patch.object(
                LICENSES.subprocess, "check_output", return_value=json.dumps(metadata)):
            LICENSES.collect(self.output)
            first_bundle = (self.output / "NOTICE").read_bytes()
            LICENSES.collect(self.output)
        self.assertEqual((self.output / "NOTICE").read_bytes(), first_bundle)
        self.assertEqual(sorted(path.name for path in self.output.iterdir()), ["NOTICE", "license-index.json"])
        index = json.loads((self.output / "license-index.json").read_text())
        self.assertEqual(len(index), 2)
        for entry in index:
            section = f"{entry['name']} {entry['version']}"
            self.assertEqual(entry["notice_file"], "NOTICE")
            self.assertEqual(entry["notices"], [section])
            self.assertEqual(entry["notice_provenance"][0]["original_notice_sha256"],
                             self.record["original_notice_sha256"])
            self.assertEqual(first_bundle.count(f"=== {section} ===\n".encode()), 1)
            self.assertEqual(LICENSES.original_notice(first_bundle, section), self.contents)

    def test_unrelated_notice_edits_do_not_invalidate_the_original_checksum(self):
        self.notice.write_bytes(b"Updated attribution overview\n" + self.notice.read_bytes())
        self.retain()

    def test_refuses_missing_duplicate_and_incomplete_supplemental_sections(self):
        original = self.notice.read_bytes()
        for bundle in (b"unrelated notice", original + original,
                       original.replace(b"--- End of original notice ---", b"missing end")):
            with self.subTest(bundle=bundle):
                self.notice.write_bytes(bundle)
                with self.assertRaisesRegex(ValueError, "NOTICE section"):
                    self.retain()

    def test_native_notice_walk_ignores_build_and_vcs_caches(self):
        for directory in ["native", "target", ".git", "__pycache__"]:
            path = self.source / directory / "LICENSE"
            path.parent.mkdir()
            path.write_text("notice")
        notices = [path.relative_to(self.source).as_posix()
                   for path in LICENSES.source_files(self.source) if LICENSES.notice(path)]
        self.assertEqual(notices, ["native/LICENSE"])


class CommittedNoticeBundleTests(unittest.TestCase):
    def test_each_original_notice_checksum_is_preserved_in_its_package_section(self):
        repository = Path(__file__).resolve().parents[2]
        records = json.loads((repository / "scripts/lib/tree-sitter-sources.json").read_text())
        bundle = (repository / "NOTICE").read_bytes()
        self.assertTrue(records)
        for record in records:
            with self.subTest(package=record["name"], version=record["version"]):
                contents = LICENSES.original_notice(bundle, f"{record['name']} {record['version']}")
                self.assertEqual(hashlib.sha256(contents).hexdigest(),
                                 record["original_notice_sha256"])


class SourceArchiveTests(unittest.TestCase):
    def test_collected_index_preserves_exact_registry_source_and_repository_without_mislabeling_patches(self):
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            script = repository / "scripts/lib/collect-licenses.py"
            script.parent.mkdir(parents=True)
            supplements = repository / "scripts/lib/tree-sitter-sources.json"
            supplements.write_text("[]", encoding="utf-8")
            (repository / "NOTICE").write_text("Project attribution\n", encoding="utf-8")
            packages = []
            sources = {
                "symphonia-core": "registry+https://github.com/rust-lang/crates.io-index",
                "patched-binding": None,
                "git-binding": "git+https://example.invalid/binding?rev=123#123",
                "other-registry": "registry+https://example.invalid/index",
            }
            for name, origin in sources.items():
                source = repository / "sources" / name
                source.mkdir(parents=True)
                (source / "LEGAL.txt").write_text(f"Original notice: {name}\n", encoding="utf-8")
                packages.append({"id": name, "name": name, "version": "0.5.5",
                                 "manifest_path": str(source / "Cargo.toml"),
                                 "license": "MPL-2.0", "license_file": "LEGAL.txt",
                                 "repository": f"https://example.invalid/{name}", "source": origin})
            metadata = {"packages": packages, "workspace_members": []}
            destination = repository / "package"
            destination.mkdir()
            (destination / "NOTICE").write_text("Project attribution\n", encoding="utf-8")
            with patch.object(LICENSES, "__file__", str(script)), patch.object(
                    LICENSES.subprocess, "check_output", return_value=json.dumps(metadata)):
                LICENSES.collect(destination)
            index = json.loads((destination / "license-index.json").read_text())
            entries = {entry["name"]: entry for entry in index}
            self.assertEqual(entries["symphonia-core"]["source_archive_url"],
                             "https://crates.io/api/v1/crates/symphonia-core/0.5.5/download")
            for name in sources:
                entry = entries[name]
                self.assertEqual(entry["repository"], f"https://example.invalid/{name}")
                self.assertEqual(entry["notices"], [f"cargo/{name}-0.5.5/LEGAL.txt"])
                self.assertEqual(entry["notice_file"], "NOTICE")
                notice = (destination / entry["notice_file"]).read_bytes()
                self.assertEqual(LICENSES.original_notice(notice, entry["notices"][0]),
                                 f"Original notice: {name}\n".encode())
                if name != "symphonia-core":
                    self.assertNotIn("source_archive_url", entry)
            self.assertIn("crates.io source archive URLs.", (destination / "NOTICE").read_text())


if __name__ == "__main__":
    unittest.main()
