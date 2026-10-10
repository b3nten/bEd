#!/usr/bin/env python3
"""Check Bed's workspace membership and production dependency boundaries."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODULES = {
    "bed-module-editor", "bed-module-debug", "bed-module-explorer",
    "bed-module-search", "bed-module-terminal", "bed-module-projects",
    "bed-module-settings", "bed-module-git", "bed-module-color",
}
LINKED_FEATURES = {
    "bed-plugin-structure", "bed-plugin-image", "bed-plugin-gltf",
    "bed-plugin-font", "bed-plugin-audio", "bed-plugin-csv",
    "bed-plugin-markdown", "bed-plugin-json", "bed-plugin-sqlite", "bed-plugin-http",
}
# These are allowed direct edges, not required implementation dependencies.
# Removing an unused dependency must not require weakening a boundary check.
ALLOWED_INTERNAL = {
    "bed": {
        "bedterm",
        "bed-workbench", "bed-workbench-api", "bed-editing", "bed-files",
        "bed-highlight", "bed-lsp", "bed-document-session", "bed-ui",
        "bed-editor-ui", "bed-settings", "bed-terminal", "bed-debug",
        "bed-effects", "bed-effects-config", "bed-remote", "bed-plugin",
    } | MODULES | LINKED_FEATURES | {"bed-scenes"},
    "bedterm": {"bed-editing", "bed-workbench-api", "bed-terminal"},
    "bed-workbench": {
        "bed-workbench-api", "bed-editing", "bed-files", "bed-highlight",
        "bed-lsp", "bed-document-session", "bed-ui", "bed-editor-ui",
        "bed-settings", "bed-terminal", "bed-remote",
    } | MODULES,
    "bed-workbench-api": {"bed-editing", "bed-document-session"},
    "bed-module-editor": {
        "bed-editing", "bed-document-session", "bed-ui", "bed-editor-ui",
        "bed-lsp", "bed-workbench-api",
    },
    "bed-module-debug": {
        "bed-editing", "bed-document-session", "bed-editor-ui", "bed-debug", "bed-ui",
        "bed-workbench-api",
    },
    "bed-module-git": {
        "bed-git", "bed-editing", "bed-document-session", "bed-editor-ui",
        "bed-module-editor", "bed-workbench-api", "bed-remote", "bed-ui",
    },
    "bed-git": set(),
    "bed-module-explorer": {
        "bed-editing", "bed-files", "bed-remote", "bed-document-session",
        "bed-ui", "bed-workbench-api",
    },
    "bed-module-search": {
        "bed-editing", "bed-files", "bed-remote", "bed-document-session", "bed-ui",
        "bed-workbench-api",
    },
    "bed-module-terminal": {"bed-document-session", "bed-workbench-api"},
    "bed-module-projects": {
        "bed-document-session", "bed-ui", "bed-workbench-api", "bed-scenes",
    },
    "bed-scenes": {"bed-workbench-api", "bed-document-session"},
    "bed-module-color": {"bed-workbench-api", "bed-editing", "bed-ui"},
    "bed-module-settings": {
        "bed-editing", "bed-document-session", "bed-settings", "bed-ui",
        "bed-workbench-api",
    },
    "bed-settings": {
        "bed-editing", "bed-document-session", "bed-highlight", "bed-ui",
        "bed-effects-config",
    },
    "bed-debug": set(),
    "bed-editing": set(),
    "bed-files": {"bed-editing", "bed-remote"},
    "bed-highlight": {"bed-editing"},
    "bed-lsp": {"bed-editing", "bed-remote"},
    "bed-document-session": {
        "bed-editing", "bed-files", "bed-highlight", "bed-lsp", "bed-remote",
    },
    "bed-ui": {"bed-editing"},
    "bed-editor-ui": {
        "bed-editing", "bed-document-session", "bed-highlight", "bed-ui",
    },
    "bed-terminal": {"bed-editing", "bed-remote"},
    "bed-effects": {"bed-effects-config"},
    "bed-effects-config": set(),
    "bed-remote": {"bed-git", "bed-editing"},
    "bed-headless": {"bed-remote", "bed-files"},
    "bed-plugin": {"bed-workbench-api"},
    "bed-plugin-structure": {
        "bed-editing", "bed-highlight", "bed-plugin", "bed-ui",
    },
    "bed-plugin-image": {"bed-editing", "bed-plugin", "bed-ui"},
    "bed-plugin-gltf": {"bed-editing", "bed-plugin", "bed-ui"},
    "bed-plugin-font": {"bed-editing", "bed-plugin", "bed-ui"},
    "bed-plugin-audio": {"bed-editing", "bed-plugin", "bed-ui"},
    "bed-plugin-csv": {"bed-editing", "bed-plugin", "bed-document-session", "bed-ui"},
    "bed-plugin-markdown": {
        "bed-editing", "bed-plugin", "bed-workbench-api", "bed-plugin-image",
        "bed-remote", "bed-ui",
    },
    "bed-plugin-json": {"bed-editing", "bed-plugin"},
    "bed-plugin-sqlite": {"bed-editing", "bed-workbench-api"},
    "bed-plugin-http": {"bed-editing", "bed-workbench-api"},
}
HEADLESS = {
    "bed-editing", "bed-files", "bed-highlight", "bed-lsp",
    "bed-document-session", "bed-remote", "bed-headless", "bed-debug",
    "bed-effects-config", "bed-git",
}
GUI = {
    "dear-imgui-rs", "dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu",
    "winit", "wgpu", "arboard", "rfd", "resvg", "muda", "objc2-app-kit",
}
NATIVE_BACKENDS = {
    "winit", "arboard", "rfd", "muda", "objc2-app-kit",
    "dear-imgui-winit", "dear-imgui-wgpu",
}
PORTABLE_UI = {
    "bed-ui", "bed-editor-ui", "bed-settings", "bed-workbench-api", "bed-plugin",
} | MODULES | LINKED_FEATURES | {"bed-scenes"}


def check(offline=False):
    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if offline:
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
    packages = {p["id"]: p for p in metadata["packages"]}
    members = {packages[pid]["name"]: packages[pid] for pid in metadata["workspace_members"]}
    assert members.keys() == ALLOWED_INTERNAL.keys(), (
        f"Unexpected workspace members: {sorted(members.keys() ^ ALLOWED_INTERNAL.keys())}"
    )
    # Prefer published dependencies; local forks must document an active Bed patch.
    for package in packages.values():
        if package["source"] is not None or package["id"] in metadata["workspace_members"]:
            continue
        directory = Path(package["manifest_path"]).parent
        assert directory.is_relative_to(ROOT / "vendor"), f"Unexpected local dependency: {directory}"
        assert (directory / "BED_PATCHES.md").is_file(), f"Undocumented dependency fork: {directory}"
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}

    def production_dependencies(pid):
        # Dev dependencies bring linked feature fixtures into the shell tests.
        return {
            dep["pkg"] for dep in nodes[pid]["deps"]
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"])
        }

    for name, package in members.items():
        direct = {
            packages[pid]["name"] for pid in production_dependencies(package["id"])
        }
        internal = direct & members.keys()
        unexpected = internal - ALLOWED_INTERNAL[name]
        assert not unexpected, f"{name}: forbidden direct dependencies: {sorted(unexpected)}"
        seen = set()
        pending = list(production_dependencies(package["id"]))
        while pending:
            pid = pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            pending.extend(production_dependencies(pid) - seen)
        names = {packages[pid]["name"] for pid in seen}
        forbidden = set()
        if name in HEADLESS:
            forbidden = GUI
        elif name in PORTABLE_UI:
            forbidden = NATIVE_BACKENDS | {
                "bed", "bed-workbench", "bed-terminal", "bed-effects",
            }
            if name == "bed-ui":
                forbidden |= {
                    "bed-document-session", "bed-editor-ui", "bed-highlight",
                    "bed-lsp", "bed-workbench-api", "wgpu",
                } | MODULES | LINKED_FEATURES | {"bed-scenes"}
            elif name == "bed-editor-ui":
                # Document services may use LSP; the widget has no direct LSP or
                # workbench dependency and only consumes neutral presentation data.
                assert "bed-lsp" not in direct, "Editor widgets must not depend directly on LSP"
                forbidden |= {"bed-workbench-api"} | MODULES | LINKED_FEATURES | {"bed-scenes"}
        elif name == "bed-workbench":
            # Dialogs/Trash are shell services. Native windows, clipboard and the
            # ImGui platform/renderer backends remain with the desktop host.
            forbidden = {
                "bed", "winit", "arboard", "muda", "dear-imgui-winit",
                "dear-imgui-wgpu", "bed-effects",
            } | LINKED_FEATURES
            # rfd can itself use AppKit; the shell must not import AppKit directly.
            assert not direct & (NATIVE_BACKENDS - {"rfd"}), (
                f"Workbench imports native host APIs: {sorted(direct & (NATIVE_BACKENDS - {'rfd'}))}"
            )
        assert not names & forbidden, (
            f"{name} crosses its dependency boundary: {sorted(names & forbidden)}"
        )
    print(f"All {len(members)} crates respect their production dependency boundaries.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    check(parser.parse_args().offline)
