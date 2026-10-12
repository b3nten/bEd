#!/usr/bin/env python3
"""Build an isolated desktop consumer and check its production dependencies."""
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tests/fixtures/desktop-app/Cargo.toml"
FORBIDDEN = {
    "bed", "bed-config", "bed-settings", "bed-editor-api", "bed-editor-workbench",
    "bed-editor", "bed-editor-ui", "bed-editing", "bed-document-session",
    "bed-files", "bed-remote", "bed-lsp", "bed-git", "bed-git-core",
    "bed-terminal", "bed-terminal-core", "bed-debug", "bed-debug-core",
    "bed-scenes", "bed-model-viewer", "bed-audio", "bevy_pbr", "bevy_gltf", "rodio",
}


def cargo(*arguments):
    return subprocess.check_output(
        ["cargo", *arguments, "--locked", "--manifest-path", str(MANIFEST)],
        cwd=MANIFEST.parent,
        env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target")},
        text=True,
    )


def main():
    for features in ("", "layout", "effects", "layout,effects"):
        options = ["--no-default-features"]
        if features:
            options += ["--features", features]
        tree = cargo("tree", *options, "--edges", "normal,build", "--prefix", "none")
        names = {line.split()[0] for line in tree.splitlines() if line.strip()}
        forbidden = FORBIDDEN | ({"bed-effects", "bed-effects-config"} if "effects" not in features else set())
        assert not names & forbidden, f"{features or 'base'} includes {sorted(names & forbidden)}"
        if "layout" not in features:
            assert not names & {"bed-workbench", "bed-workbench-api"}, "Base installs layout"
        cargo("check", *options, "--all-targets")
        print(f"Independent desktop consumer: {features or 'base'} passed.", flush=True)


if __name__ == "__main__":
    main()
