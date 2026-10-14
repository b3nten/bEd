#!/usr/bin/env python3
"""Run the independent application's seeded native and fresh-process proofs."""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bedside-table"))
    parser.add_argument("--capture-directory", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="bedside-native-") as temporary:
        config = Path(temporary) / "config"
        for flag, name in [("--smoke", "initial"), ("--smoke-reopen", "restart")]:
            command = [str(binary), flag, "--config-dir", str(config)]
            if args.capture_directory:
                args.capture_directory.mkdir(parents=True, exist_ok=True)
                command += ["--capture-frame", str(args.capture_directory.resolve() / f"bedside-{name}.png")]
            result = subprocess.run(command, timeout=90, check=False)
            if result.returncode:
                raise SystemExit(result.returncode)
    print("bedside-table native initial + restart PASS")


if __name__ == "__main__":
    main()
