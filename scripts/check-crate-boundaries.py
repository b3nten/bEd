#!/usr/bin/env python3
"""Check Bed's workspace membership and production dependency boundaries."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODULES = {
    "bed-editor", "bed-debug", "bed-explorer",
    "bed-search", "bed-terminal", "bed-projects",
    "bed-settings", "bed-git", "bed-color",
}
LINKED_FEATURES = {
    "bed-outline", "bed-image", "bed-model-viewer",
    "bed-font", "bed-audio", "bed-csv",
    "bed-markdown", "bed-json", "bed-sqlite", "bed-http",
}
# These are allowed direct edges, not required implementation dependencies.
# Removing an unused dependency must not require weakening a boundary check.
ALLOWED_INTERNAL = {
    "bed": {
        "bed-desktop", "bed-automation", "bed-idle", "bed-workbench",
        "bedterm", "bed-runtime",
        "bed-editor-workbench", "bed-editor-api", "bed-editing", "bed-files",
        "bed-highlight", "bed-lsp", "bed-document-session", "bed-ui",
        "bed-editor-ui", "bed-config", "bed-terminal-core", "bed-debug-core",
        "bed-effects", "bed-effects-config", "bed-remote", "bed-editor-api",
    } | MODULES | LINKED_FEATURES | {"bed-scenes"},
    "bedterm": {"bed-editing", "bed-editor-api", "bed-terminal-core"},
    "bed-runtime": set(),
    "bed-desktop": {"bed-ui"},
    "bed-automation": {"bed-desktop", "bed-workbench", "bed-workbench-api", "bed-editor-api", "bed-editor-workbench", "bed-document-session", "bed-editing"},
    "bed-idle": {"bed-desktop", "bed-scenes", "bed-editor-api", "bed-ui"},
    "bed-workbench-api": set(),
    "bed-workbench": {"bed-workbench-api"},
    "bed-layout-app": {"bed-desktop", "bed-workbench", "bed-workbench-api", "bed-ui"},
    "bed-editor-workbench": {
        "bed-workbench", "bed-workbench-api",
        "bed-editor-api", "bed-editing", "bed-files", "bed-highlight",
        "bed-lsp", "bed-document-session", "bed-ui", "bed-editor-ui",
        "bed-config", "bed-remote", "bed-runtime",
    },
    "bed-editor-api": {"bed-editing", "bed-document-session", "bed-lsp", "bed-workbench-api", "bed-ui"},
    "bed-editor": {
        "bed-editing", "bed-document-session", "bed-ui", "bed-editor-ui",
        "bed-lsp", "bed-editor-api",
    },
    "bed-debug": {
        "bed-editing", "bed-document-session", "bed-editor-ui", "bed-debug-core", "bed-ui",
        "bed-editor-api", "bed-terminal-core",
    },
    "bed-git": {
        "bed-git-core", "bed-editing", "bed-document-session", "bed-editor-ui",
        "bed-editor", "bed-editor-api", "bed-remote", "bed-ui",
    },
    "bed-git-core": set(),
    "bed-explorer": {
        "bed-editing", "bed-files", "bed-remote", "bed-document-session",
        "bed-ui", "bed-editor-api",
    },
    "bed-search": {
        "bed-editing", "bed-files", "bed-remote", "bed-document-session", "bed-ui",
        "bed-editor-api",
    },
    "bed-terminal": {"bed-document-session", "bed-editor-api", "bed-terminal-core", "bed-ui", "bed-workbench", "bed-desktop"},
    "bed-projects": {
        "bed-document-session", "bed-ui", "bed-editor-api", "bed-scenes",
    },
    "bed-scenes": {"bed-editor-api", "bed-document-session", "bed-runtime", "bed-ui"},
    "bed-color": {"bed-editor-api", "bed-editing", "bed-ui"},
    "bed-settings": {
        "bed-editing", "bed-document-session", "bed-config", "bed-ui",
        "bed-editor-api",
    },
    "bed-config": {"bed-highlight", "bed-ui", "bed-effects-config"},
    "bed-debug-core": set(),
    "bed-editing": set(),
    "bed-files": {"bed-editing", "bed-remote"},
    "bed-highlight": {"bed-editing"},
    "bed-lsp": {"bed-editing", "bed-remote"},
    "bed-document-session": {
        "bed-editing", "bed-files", "bed-highlight", "bed-lsp", "bed-remote",
    },
    "bed-ui": set(),
    "bed-editor-ui": {
        "bed-editing", "bed-document-session", "bed-highlight", "bed-ui",
    },
    "bed-terminal-core": {"bed-editing", "bed-remote", "bed-ui"},
    "bed-effects": {"bed-effects-config", "bed-desktop"},
    "bed-effects-config": set(),
    "bed-remote": {"bed-git-core", "bed-editing"},
    "bed-headless": {"bed-remote", "bed-files"},
    "bed-outline": {
        "bed-editing", "bed-highlight", "bed-editor-api", "bed-ui",
    },
    "bed-image": {"bed-editing", "bed-editor-api", "bed-ui"},
    "bed-model-viewer": {"bed-editing", "bed-editor-api", "bed-ui", "bed-runtime"},
    "bed-font": {"bed-editing", "bed-editor-api", "bed-ui"},
    "bed-audio": {"bed-editing", "bed-editor-api", "bed-ui"},
    "bed-csv": {"bed-editing", "bed-editor-api", "bed-document-session", "bed-ui"},
    "bed-markdown": {
        "bed-editing", "bed-editor-api", "bed-editor-api", "bed-image",
        "bed-remote", "bed-ui", "bed-document-session",
    },
    "bed-json": {"bed-editing", "bed-editor-api"},
    "bed-sqlite": {"bed-editing", "bed-editor-api"},
    "bed-http": {"bed-editing", "bed-editor-api", "bed-ui"},
}
# Portable shortcut data and scoped input are shared by hosts and frontends.
ALLOWED_INTERNAL["bed-keybindings"] = set()
for consumer in MODULES | LINKED_FEATURES | {
    "bed", "bed-config", "bed-editor-ui", "bed-editor-workbench",
    "bed-workbench", "bed-workbench-api", "bed-scenes",
}:
    ALLOWED_INTERNAL[consumer].add("bed-keybindings")
# Feature plugins register actual panel factories and command systems here.
for feature in MODULES | LINKED_FEATURES | {"bed-scenes"}:
    ALLOWED_INTERNAL[feature].add("bed-workbench-api")
HEADLESS = {
    "bed-editing", "bed-files", "bed-highlight", "bed-lsp",
    "bed-document-session", "bed-remote", "bed-headless", "bed-debug-core",
    "bed-effects-config", "bed-git-core",
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
    "bed-ui", "bed-editor-ui", "bed-config", "bed-editor-api",
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
    for name, package in members.items():
        direct = {
            dependency["name"] for dependency in package["dependencies"]
            if dependency["kind"] != "dev"
        }
        internal = direct & members.keys()
        unexpected = internal - ALLOWED_INTERNAL[name]
        assert not unexpected, f"{name}: forbidden direct dependencies: {sorted(unexpected)}"
        # Resolve each consumer separately. Workspace-wide feature unification
        # would incorrectly attribute the desktop's Bevy window/PBR features to
        # graphics-free services and the layout-only example.
        tree = ["cargo", "tree", "--locked", "-p", name, "--no-default-features",
                "--edges", "normal,build", "--prefix", "none", "--format", "{p}"]
        if offline:
            tree.append("--offline")
        output = subprocess.check_output(tree, cwd=ROOT, text=True)
        names = {line.split()[0] for line in output.splitlines() if line.strip()} - {name}
        forbidden = set()
        if name in {"bed-workbench", "bed-workbench-api", "bed-layout-app", "bed-desktop"}:
            forbidden = {
                "bed", "bed-config", "bed-files", "bed-remote",
                "bed-editor-api", "bed-editor-workbench", "bed-editing",
                "bed-document-session", "bed-lsp", "bed-git-core", "bed-terminal-core",
                "bed-scenes", "bevy_pbr", "bevy_gltf", "rodio",
            } | MODULES | LINKED_FEATURES
            if name in {"bed-workbench", "bed-workbench-api"}:
                forbidden |= NATIVE_BACKENDS | {"bed-desktop", "bed-effects"}
        elif name in HEADLESS:
            forbidden = GUI
        elif name in PORTABLE_UI:
            forbidden = NATIVE_BACKENDS | {
                "bed", "bed-editor-workbench", "bed-terminal-core", "bed-effects",
            }
            if name in {"bed-terminal", "bed-debug"}:
                # These systems consume the canonical terminal backend as typed
                # world state; window and clipboard adapters remain forbidden.
                forbidden.remove("bed-terminal-core")
            if name == "bed-ui":
                forbidden |= {
                    "bed-document-session", "bed-editor-ui", "bed-highlight",
                    "bed-lsp", "bed-editor-api", "wgpu",
                } | MODULES | LINKED_FEATURES | {"bed-scenes"}
            elif name == "bed-editor-ui":
                # Document services may use LSP; the widget has no direct LSP or
                # workbench dependency and only consumes neutral presentation data.
                assert "bed-lsp" not in direct, "Editor widgets must not depend directly on LSP"
                forbidden |= {"bed-editor-api"} | MODULES | LINKED_FEATURES | {"bed-scenes"}
        elif name == "bed-runtime":
            forbidden = NATIVE_BACKENDS | {
                "bed", "bed-editor-workbench", "bed-editor-api", "dear-imgui-rs",
                "dear-imgui-sys", "bed-editor-ui", "bed-config", "bed-effects",
            } | MODULES | LINKED_FEATURES
        elif name == "bed-editor-workbench":
            # Dialogs/Trash belong to editor transitions. Native windows, clipboard and the
            # ImGui platform/renderer backends remain with the desktop host.
            forbidden = {
                "bed", "winit", "arboard", "muda", "dear-imgui-winit",
                "dear-imgui-wgpu", "bed-effects",
            } | LINKED_FEATURES | MODULES | {"bed-terminal-core"}
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
