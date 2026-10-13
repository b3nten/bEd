#!/usr/bin/env python3
"""Build an isolated desktop consumer and check its production dependencies."""
import argparse
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tests/fixtures/desktop-app/Cargo.toml"
FORBIDDEN = {
    "bed", "bed-app-config", "bed-plugin-settings", "bed-app", "bed-core",
    "bed-plugin-editor", "bed-editor-widgets", "bed-editing", "bed-documents",
    "bed-files", "bed-remote", "bed-lsp", "bed-plugin-git", "bed-git",
    "bed-plugin-terminal", "bed-terminal", "bed-plugin-debugger", "bed-debugger",
    "bed-plugin-mascots", "bed-plugin-model-viewer", "bed-plugin-audio-player", "bevy_pbr", "bevy_gltf", "rodio",
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
        forbidden = FORBIDDEN | ({"bed-plugin-effects", "bed-effects-config"} if "effects" not in features else set())
        forbidden |= {name for name in names if name.startswith("bed-plugin-")} - ({"bed-plugin-effects"} if "effects" in features else set())
        assert not names & forbidden, f"{features or 'base'} includes {sorted(names & forbidden)}"
        if "layout" not in features:
            assert not names & {"bed-workbench", "bed-api"}, "Base installs layout"
        cargo("check", *options, "--all-targets", offline=offline)
        print(f"Independent desktop consumer: {features or 'base'} passed.", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    main(parser.parse_args().offline)
