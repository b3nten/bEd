#!/usr/bin/env python3
"""Stage the standalone helper with attribution and package it for CI upload."""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main():
    repository = Path(__file__).resolve().parent.parent
    binary_name = "bed-headless.exe" if os.name == "nt" else "bed-headless"
    binary = repository / "target" / "release" / binary_name
    if not binary.is_file():
        raise SystemExit(f"Missing release helper: {binary}. Run cargo build --release -p bed-headless first.")
    target = repository / "target"
    destination = target / "headless-package"
    with tempfile.TemporaryDirectory(prefix="headless-package-", dir=target) as temporary:
        stage = Path(temporary) / "bed-headless"
        stage.mkdir()
        shutil.copy2(binary, stage / binary_name)
        for name in ("LICENSE", "NOTICE", "UPSTREAM_REVISION"):
            shutil.copy2(repository / name, stage / name)
        shutil.copytree(repository / "LICENSES", stage / "LICENSES")
        subprocess.run(
            [sys.executable, str(repository / "scripts" / "collect-licenses.py"), str(stage)],
            cwd=repository,
            check=True,
        )
        (stage / "README.txt").write_text(
            "Bed standalone headless helper\n\n"
            "Use the helper matching the remote host's operating system and CPU architecture,\n"
            "and the same Bed release as the client. This standalone artifact is for\n"
            "embedding and direct protocol use. Desktop Bed automatically installs its\n"
            "bundled helper when connecting; no executable path is configured.\n\n"
            "Running this prebuilt helper needs no Rust or C compiler. The host's system\n"
            "runtime libraries must be compatible with this build. Connecting never builds\n"
            "the helper or installs a compiler.\n\n"
            "The helper runs without a display server or GPU. Install development tools such\n"
            "as Git, configured language servers and shells separately on the remote host.\n"
            "The desktop uses its system SSH client and existing SSH authentication.\n\n"
            "LICENSE, NOTICE and LICENSES retain attribution and dependency license notices.\n"
            "Dependency notices cover the full resolved Bed workspace; the helper uses a\n"
            "subset of these dependencies. UPSTREAM_REVISION records the translated source.\n",
            encoding="utf-8",
        )
        if destination.exists():
            shutil.rmtree(destination)
        shutil.move(str(stage), destination)
    archive = shutil.make_archive(str(destination), "zip", root_dir=target, base_dir=destination.name)
    print(f"Packaged standalone helper: {archive}")


if __name__ == "__main__":
    main()
