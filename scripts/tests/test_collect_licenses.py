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
        self.record = {"revision": "012345", "file": "tree-sitter-NOTICES.txt",
                       "url": "https://example.invalid/012345/LICENSE"}
        self.notice = self.repository / "LICENSES/tree-sitter-NOTICES.txt"
        self.notice.parent.mkdir(parents=True)
        self.notice.write_bytes(b"Original copyright and permission notice\n")
        self.record["sha256"] = hashlib.sha256(self.notice.read_bytes()).hexdigest()
        self.record["original_notice_sha256"] = self.record["sha256"]
        self.vcs = self.source / ".cargo_vcs_info.json"
        self.vcs.write_text(json.dumps({"git": {"sha1": self.record["revision"]}}))

    def retain(self, records=None):
        if records is None:
            records = {(self.package["name"], self.package["version"]): self.record}
        return LICENSES.supplemental_notice(
            self.package, self.source, self.repository, self.output, records)

    def test_retains_original_notice_and_source_provenance_without_cache_changes(self):
        before = self.vcs.read_bytes()
        path, provenance = self.retain()
        self.assertEqual((self.output / path).read_bytes(), self.notice.read_bytes())
        self.assertEqual(provenance["revision"], self.record["revision"])
        self.assertEqual(provenance["url"], self.record["url"])
        self.assertEqual(provenance["original_notice_sha256"], self.record["original_notice_sha256"])
        self.assertEqual(provenance["bundle_sha256"], self.record["sha256"])
        self.assertEqual(path, "supplemental/tree-sitter-NOTICES.txt")
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

    def test_refuses_missing_shared_notice_bundle_before_copying(self):
        self.notice.unlink()
        with self.assertRaises(FileNotFoundError):
            self.retain()
        self.assertFalse(self.output.exists())

    def test_collector_copies_shared_bundle_once_and_references_it_for_each_package(self):
        second_source = self.repository / "second-registry-package"
        second_source.mkdir()
        (second_source / ".cargo_vcs_info.json").write_bytes(self.vcs.read_bytes())
        second_package = {"name": "tree-sitter-second", "version": "2.0.0"}
        records = [dict(self.record, **package) for package in (self.package, second_package)]
        (self.repository / "LICENSES/tree-sitter-sources.json").write_text(json.dumps(records))
        packages = [dict(package, id=package["name"], license="MIT", manifest_path=str(source / "Cargo.toml"))
                    for package, source in ((self.package, self.source), (second_package, second_source))]
        metadata = {"packages": packages, "workspace_members": []}
        script = self.repository / "scripts/lib/collect-licenses.py"
        with patch.object(LICENSES, "__file__", str(script)), patch.object(
                LICENSES.subprocess, "check_output", return_value=json.dumps(metadata)), patch.object(
                LICENSES.shutil, "copyfile", wraps=LICENSES.shutil.copyfile) as copying:
            LICENSES.collect(self.output)
        bundle_copies = [call for call in copying.call_args_list
                         if Path(call.args[0]).resolve() == self.notice.resolve()]
        self.assertEqual(len(bundle_copies), 1)
        output = self.output / "LICENSES/dependencies"
        self.assertEqual(sorted(path.relative_to(output).as_posix() for path in output.rglob("*") if path.is_file()),
                         ["index.json", "supplemental/tree-sitter-NOTICES.txt"])
        index = json.loads((output / "index.json").read_text())
        self.assertEqual(len(index), 2)
        for entry in index:
            self.assertEqual(entry["notices"], ["supplemental/tree-sitter-NOTICES.txt"])
            self.assertEqual(entry["notice_provenance"][0]["bundle_sha256"], self.record["sha256"])
        self.assertEqual((output / index[0]["notices"][0]).read_bytes(), self.notice.read_bytes())
        self.assertFalse((self.output / "LICENSES/tree-sitter-NOTICES.txt").exists())
        self.assertIn("are packaged at LICENSES/dependencies/supplemental/tree-sitter-NOTICES.txt.",
                      (self.output / "NOTICE").read_text())

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
        records = json.loads((repository / "LICENSES/tree-sitter-sources.json").read_text())
        bundle = (repository / "LICENSES/tree-sitter-NOTICES.txt").read_bytes()
        self.assertTrue(records)
        bundle_hash = hashlib.sha256(bundle).hexdigest()
        for record in records:
            with self.subTest(package=record["name"], version=record["version"]):
                self.assertEqual(record["file"], "tree-sitter-NOTICES.txt")
                self.assertEqual(record["sha256"], bundle_hash)
                heading = f"=== {record['name']} {record['version']} ===\n".encode()
                self.assertEqual(bundle.count(heading), 1)
                start = bundle.index(heading) + len(heading)
                marker = b"--- Original notice ---\n"
                start = bundle.index(marker, start) + len(marker)
                end = bundle.index(b"\n--- End of original notice ---", start)
                self.assertEqual(hashlib.sha256(bundle[start:end]).hexdigest(),
                                 record["original_notice_sha256"])


class SourceArchiveTests(unittest.TestCase):
    def test_collected_index_preserves_exact_registry_source_and_repository_without_mislabeling_patches(self):
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            script = repository / "scripts/lib/collect-licenses.py"
            script.parent.mkdir(parents=True)
            supplements = repository / "LICENSES/tree-sitter-sources.json"
            supplements.parent.mkdir(parents=True)
            supplements.write_text("[]", encoding="utf-8")
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
            index = json.loads((destination / "LICENSES/dependencies/index.json").read_text())
            entries = {entry["name"]: entry for entry in index}
            self.assertEqual(entries["symphonia-core"]["source_archive_url"],
                             "https://crates.io/api/v1/crates/symphonia-core/0.5.5/download")
            for name in sources:
                entry = entries[name]
                self.assertEqual(entry["repository"], f"https://example.invalid/{name}")
                self.assertEqual(entry["notices"], [f"cargo/{name}-0.5.5/LEGAL.txt"])
                notice = destination / "LICENSES/dependencies" / entry["notices"][0]
                self.assertEqual(notice.read_text(), f"Original notice: {name}\n")
                if name != "symphonia-core":
                    self.assertNotIn("source_archive_url", entry)
            self.assertIn("Source archive URLs for crates.io dependencies are listed in\n"
                          "LICENSES/dependencies/index.json.", (destination / "NOTICE").read_text())


if __name__ == "__main__":
    unittest.main()
