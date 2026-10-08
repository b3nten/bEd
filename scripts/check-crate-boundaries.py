#!/usr/bin/env python3
"""Check Bed's workspace membership and production dependency boundaries."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXPECTED = {
    "bed": {"bed-core", "bed-files", "bed-highlight", "bed-lsp", "bed-session", "bed-ui", "bed-terminal", "bed-debug", "bed-effects", "bed-remote", "bed-plugin", "bed-plugin-structure", "bed-plugin-image", "bed-plugin-gltf", "bed-plugin-font", "bed-plugin-audio", "bed-plugin-csv"},
    "bed-debug": set(),
    "bed-core": set(),
    "bed-files": {"bed-core", "bed-remote"},
    "bed-highlight": {"bed-core"},
    "bed-lsp": {"bed-core", "bed-remote"},
    "bed-session": {"bed-core", "bed-files", "bed-highlight", "bed-lsp", "bed-remote"},
    "bed-ui": {"bed-core", "bed-session", "bed-highlight", "bed-lsp"},
    "bed-terminal": {"bed-core", "bed-remote"},
    "bed-effects": set(),
    "bed-remote": set(),
    "bed-headless": {"bed-remote", "bed-files"},
    "bed-plugin": {"bed-core", "bed-session"},
    "bed-plugin-structure": {"bed-core", "bed-highlight", "bed-plugin", "bed-ui"},
    "bed-plugin-image": {"bed-core", "bed-plugin", "bed-ui"},
    "bed-plugin-gltf": {"bed-core", "bed-plugin", "bed-ui"},
    "bed-plugin-font": {"bed-core", "bed-plugin", "bed-ui"},
    "bed-plugin-audio": {"bed-core", "bed-plugin"},
    "bed-plugin-csv": {"bed-core", "bed-plugin", "bed-session"},
}
HEADLESS = {"bed-core", "bed-files", "bed-highlight", "bed-lsp", "bed-session", "bed-remote", "bed-headless", "bed-debug"}
GUI = {"dear-imgui-rs", "dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu", "winit", "wgpu", "arboard", "rfd", "resvg", "muda", "objc2-app-kit"}


def check(offline=False):
    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if offline:
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(
        command, cwd=ROOT,
    ))
    packages = {p["id"]: p for p in metadata["packages"]}
    members = {packages[pid]["name"]: packages[pid] for pid in metadata["workspace_members"]}
    assert members.keys() == EXPECTED.keys(), f"Unexpected workspace members: {sorted(members)}"
    # Prefer published dependencies; local forks must document an active Bed patch.
    for package in packages.values():
        if package["source"] is not None or package["id"] in metadata["workspace_members"]:
            continue
        directory = Path(package["manifest_path"]).parent
        assert directory.is_relative_to(ROOT / "vendor"), f"Unexpected local dependency: {directory}"
        assert (directory / "BED_PATCHES.md").is_file(), f"Undocumented dependency fork: {directory}"
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}

    def production_dependencies(pid):
        # Dev dependencies may deliberately bring GUI fixtures into an integration suite.
        return {dep["pkg"] for dep in nodes[pid]["deps"]
                if any(kind["kind"] != "dev" for kind in dep["dep_kinds"])}

    for name, package in members.items():
        internal = {packages[pid]["name"] for pid in production_dependencies(package["id"])
                    if packages[pid]["name"] in EXPECTED}
        assert internal == EXPECTED[name], f"{name}: expected {EXPECTED[name]}, found {internal}"
        seen = set()
        pending = list(production_dependencies(package["id"]))
        while pending:
            pid = pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            pending.extend(production_dependencies(pid) - seen)
        names = {packages[pid]["name"] for pid in seen}
        portable_ui = {"bed-ui", "bed-plugin", "bed-plugin-structure", "bed-plugin-image", "bed-plugin-gltf", "bed-plugin-font", "bed-plugin-audio", "bed-plugin-csv"}
        forbidden = set()
        if name in HEADLESS:
            forbidden = GUI
        elif name in portable_ui:
            forbidden = {"bed", "bed-terminal", "bed-effects", "winit", "arboard", "rfd"}
            if name == "bed-ui":
                forbidden.add("wgpu")
        assert not names & forbidden, f"{name} crosses its dependency boundary: {sorted(names & forbidden)}"
    print(f"All {len(EXPECTED)} crates respect their production dependency boundaries.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    check(parser.parse_args().offline)
