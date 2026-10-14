#!/usr/bin/env python3
"""Build an isolated desktop consumer and check its production dependencies."""
import argparse
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "bedtime/tests/fixtures/desktop-app/Cargo.toml"
FORBIDDEN = {
    "bed", "bedtime-plugin-settings", "bedtime", "bedtime-core",
    "bedtime-plugin-editor", "bedtime-editor-widgets", "bedtime-editing", "bedtime-documents",
    "bedtime-files", "bedtime-storage", "bedtime-remote", "bedtime-lsp", "bedtime-plugin-git", "bedtime-git",
    "bedtime-plugin-terminal", "bedtime-terminal", "bedtime-plugin-debugger", "bedtime-debugger",
    "bedtime-plugin-mascots", "bedtime-plugin-model-viewer", "bedtime-plugin-audio-player", "bevy_pbr", "bevy_gltf", "rodio",
}


def cargo(*arguments, offline=False):
    return subprocess.check_output(
        ["cargo", *arguments, *(["--offline"] if offline else []), "--locked", "--manifest-path", str(MANIFEST)],
        cwd=MANIFEST.parent,
        env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target")},
        text=True,
    )


def main(offline=False):
    for features in ("", "layout", "effects", "layout,effects"):
        options = ["--no-default-features"]
        if features:
            options += ["--features", features]
        tree = cargo("tree", *options, "--edges", "normal,build", "--prefix", "none", offline=offline)
        names = {line.split()[0] for line in tree.splitlines() if line.strip()}
        forbidden = FORBIDDEN | ({"bedtime-plugin-effects"} if "effects" not in features else set())
        # Layout and effects use shared API contracts (including preferences).
        # Those contracts do not install document services or a workbench.
        if features:
            forbidden -= {"bedtime-files", "bedtime-storage", "bedtime-editing"}
        forbidden |= {name for name in names if name.startswith("bedtime-plugin-")} - ({"bedtime-plugin-effects"} if "effects" in features else set())
        assert not names & forbidden, f"{features or 'base'} includes {sorted(names & forbidden)}"
        if "layout" not in features:
            assert "bedtime-workbench" not in names, "Composition installs a workbench"
        if not features:
            assert "bedtime-api" not in names, "Base includes application contracts"
        cargo("check", *options, "--all-targets", offline=offline)
        print(f"Independent desktop consumer: {features or 'base'} passed.", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    main(parser.parse_args().offline)
