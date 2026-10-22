#!/usr/bin/env python3
"""Reimport pinned grammar sources from an already populated Cargo registry.

No network, parser generator, Node, or package download runs during builds.
Example: python3 tools/import_grammars.py ~/.cargo/registry/src/<registry>
Licenses are retained from the reviewed upstream sources in sources.json.
"""
import argparse
import gzip
import hashlib
import json
import re
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("registry", type=Path)
    parser.add_argument("--check", action="store_true", help="Verify vendored hashes only")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    sources = json.loads((root / "sources.json").read_text())
    for source in sources:
        target = root / "grammars" / source["language"]
        if not args.check:
            package = args.registry / source["package"]
            archive = args.registry.parent.parent / "cache" / args.registry.name / (source["package"] + ".crate")
            checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
            assert checksum == source["package_checksum"], f"Unexpected package: {package}"
            grammar = package / ("tree-sitter-markdown-inline" if source["language"] == "markdown_inline" else "grammar")
            for path in grammar.rglob("*"):
                if not path.is_file() or path.suffix not in (".c", ".h"):
                    continue
                text = re.sub(r"\btree_sitter_[A-Za-z0-9_]+", lambda match: "bed2_" + match[0], path.read_text())
                output = target / path.relative_to(grammar)
                output.parent.mkdir(parents=True, exist_ok=True)
                if output.name == "parser.c":
                    output.with_suffix(".c.gz").write_bytes(gzip.compress(text.encode(), compresslevel=9, mtime=0))
                else:
                    output.write_text(text)
            queries = grammar / "queries" if source["language"] == "markdown_inline" else package / "queries"
            for path in queries.glob("*.scm"):
                (target / path.name).write_bytes(path.read_bytes())
        for name, expected in source["files"].items():
            assert hashlib.sha256(gzip.decompress((target / name).read_bytes()) if name.endswith(".gz") else (target / name).read_bytes()).hexdigest() == expected, f"Changed: {source['language']}/{name}"
    print(f"Verified {len(sources)} bundled grammars")


if __name__ == "__main__":
    main()
