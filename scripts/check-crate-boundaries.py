#!/usr/bin/env python3
"""Check bEd's workspace membership and production dependency boundaries."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PANEL_PLUGINS = {
    "bedtime-plugin-editor", "bedtime-plugin-debugger", "bedtime-plugin-file-explorer",
    "bedtime-plugin-file-search", "bedtime-plugin-terminal", "bedtime-plugin-workspaces",
    "bedtime-plugin-settings", "bedtime-plugin-git", "bedtime-plugin-color-picker",
}
VIEWER_PLUGINS = {
    "bedtime-plugin-source-outline", "bedtime-plugin-image-viewer", "bedtime-plugin-model-viewer",
    "bedtime-plugin-font-viewer", "bedtime-plugin-audio-player", "bedtime-plugin-csv-table",
    "bedtime-plugin-markdown-preview", "bedtime-plugin-json-tree", "bedtime-plugin-sqlite-browser",
    "bedtime-plugin-http-client",
}
CONTENT_PLUGINS = PANEL_PLUGINS | VIEWER_PLUGINS | {"bedtime-plugin-mascots"}
# These are allowed direct edges, not required implementation dependencies.
# Removing an unused dependency must not require weakening a boundary check.
ALLOWED_INTERNAL = {
    "bedside-table": {"bedtime", "bedtime-core", "bedtime-workbench", "bedtime-storage", "bedtime-sqlite"},
    "bedtime-sqlite": set(),
    "bed": {
        "bedtime-core", "bedtime-desktop", "bedtime-plugin-automation", "bedtime-plugin-idle",
        "bedtime-plugin-layouts", "bedtime-workbench",
        "bedtime", "bedtime-api", "bedtime-editing", "bedtime-files", "bedtime-highlight",
        "bedtime-lsp", "bedtime-documents", "bedtime-ui", "bedtime-editor-widgets",
        "bedtime-terminal", "bedtime-debugger", "bedtime-plugin-effects",
        "bedtime-remote", "bedtime-keybindings", "bedtime-storage",
    } | CONTENT_PLUGINS,
    "bedtime-api": {"bedtime-editing", "bedtime-documents", "bedtime-lsp", "bedtime-ui", "bedtime-keybindings", "bedtime-files", "bedtime-storage"},
    "bedtime-storage": set(),
    "bedtime-core": {
        "bedtime-plugin-layouts", "bedtime-desktop", "bedtime-workbench", "bedtime-api", "bedtime-documents",
        "bedtime-editing", "bedtime-ui", "bedtime-files", "bedtime-storage", "bedtime-lsp", "bedtime-keybindings", "bedtime-highlight", "bedtime-remote",
    },
    "bedtime": {"bedtime-api", "bedtime-core", "bedtime-desktop", "bedtime-ui"},
    "bedtime-desktop": {"bedtime-ui"},
    "bedtime-workbench": {"bedtime-api", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-layouts": {"bedtime-workbench", "bedtime-api"},
    "bedtime-example-layouts": {"bedtime", "bedtime-plugin-layouts", "bedtime-desktop", "bedtime-workbench", "bedtime-api"},
    "bedtime-example-terminal": {"bedtime", "bedtime-workbench", "bedtime-plugin-terminal"},
    "bedtime-example-editor": {"bedtime", "bedtime-plugin-editor", "bedtime-editing"},
    "bedtime-example-model": {"bedtime"},
    "bedtime-example-game": set(),
    "bedtime-example-game-runtime": {"bedtime-example-game"},
    "bedtime-example-game-editor": {"bedtime-example-game", "bedtime"},
    "bedtime-plugin-automation": {
        "bedtime-desktop", "bedtime-workbench", "bedtime-api", "bedtime-documents", "bedtime-editing",
    },
    "bedtime-plugin-idle": {"bedtime-desktop", "bedtime-plugin-mascots", "bedtime-api", "bedtime-ui"},
    "bedtime-plugin-editor": {
        "bedtime-workbench", "bedtime-editing", "bedtime-documents", "bedtime-ui", "bedtime-editor-widgets",
        "bedtime-highlight", "bedtime-lsp", "bedtime-api", "bedtime-keybindings",
    },
    "bedtime-plugin-debugger": {
        "bedtime-workbench", "bedtime-editing", "bedtime-documents", "bedtime-editor-widgets",
        "bedtime-debugger", "bedtime-ui", "bedtime-api", "bedtime-terminal", "bedtime-keybindings",
    },
    "bedtime-plugin-git": {
        "bedtime-git", "bedtime-editing", "bedtime-documents", "bedtime-editor-widgets", "bedtime-remote",
        "bedtime-ui", "bedtime-api", "bedtime-files",
    },
    "bedtime-plugin-file-explorer": {
        "bedtime-editing", "bedtime-files", "bedtime-remote", "bedtime-documents", "bedtime-ui",
        "bedtime-api", "bedtime-keybindings",
    },
    "bedtime-plugin-file-search": {
        "bedtime-editing", "bedtime-files", "bedtime-remote", "bedtime-documents", "bedtime-ui",
        "bedtime-api", "bedtime-keybindings",
    },
    "bedtime-plugin-terminal": {
        "bedtime-documents", "bedtime-api", "bedtime-terminal", "bedtime-ui", "bedtime-workbench",
        "bedtime-desktop", "bedtime-keybindings",
    },
    "bedtime-plugin-workspaces": { "bedtime-documents", "bedtime-ui", "bedtime-api", "bedtime-plugin-mascots", "bedtime-core", "bedtime-files", "bedtime-storage", "bedtime-workbench"},
    "bedtime-plugin-mascots": {"bedtime-api", "bedtime-documents", "bedtime-ui", "bedtime-desktop"},
    "bedtime-plugin-color-picker": {"bedtime-api", "bedtime-editing", "bedtime-ui"},
    "bedtime-plugin-settings": {"bedtime-ui", "bedtime-api", "bedtime-keybindings"},
    "bedtime-debugger": set(),
    "bedtime-editing": set(),
    "bedtime-files": {"bedtime-editing", "bedtime-remote"},
    "bedtime-highlight": {"bedtime-editing"},
    "bedtime-lsp": {"bedtime-editing", "bedtime-remote"},
    "bedtime-documents": {"bedtime-editing", "bedtime-files", "bedtime-highlight", "bedtime-lsp", "bedtime-remote"},
    "bedtime-ui": {"bedtime-highlight"},
    "bedtime-editor-widgets": {"bedtime-editing", "bedtime-documents", "bedtime-highlight", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-terminal": {"bedtime-editing", "bedtime-remote", "bedtime-ui"},
    "bedtime-plugin-effects": {"bedtime-desktop", "bedtime-api"},
    "bedtime-remote": {"bedtime-git", "bedtime-editing"},
    "bedtime-remote-agent": {"bedtime-remote", "bedtime-files"},
    "bedtime-git": set(),
    "bedtime-keybindings": set(),
    "bedtime-plugin-source-outline": {"bedtime-editing", "bedtime-highlight", "bedtime-api", "bedtime-ui"},
    "bedtime-plugin-image-viewer": {"bedtime-editing", "bedtime-api", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-model-viewer": {"bedtime-desktop", "bedtime-editing", "bedtime-api", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-font-viewer": {"bedtime-editing", "bedtime-api", "bedtime-ui"},
    "bedtime-plugin-audio-player": {"bedtime-editing", "bedtime-api", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-csv-table": {"bedtime-editing", "bedtime-api", "bedtime-documents", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-markdown-preview": {
        "bedtime-editing", "bedtime-api", "bedtime-plugin-image-viewer", "bedtime-remote", "bedtime-ui", "bedtime-documents", "bedtime-files",
    },
    "bedtime-plugin-json-tree": {"bedtime-editing", "bedtime-api", "bedtime-ui"},
    "bedtime-plugin-sqlite-browser": {"bedtime-sqlite","bedtime-editing", "bedtime-api", "bedtime-ui", "bedtime-keybindings"},
    "bedtime-plugin-http-client": {"bedtime-editing", "bedtime-api", "bedtime-ui"},
}
HEADLESS = {
    "bedtime-sqlite",
    "bedtime-editing", "bedtime-files", "bedtime-highlight", "bedtime-lsp", "bedtime-documents",
    "bedtime-remote", "bedtime-remote-agent", "bedtime-debugger", "bedtime-git",
    "bedtime-terminal", "bedtime-storage",
}
GUI = {
    "dear-imgui-rs", "dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu",
    "winit", "wgpu", "arboard", "rfd", "resvg", "muda", "objc2-app-kit",
}
NATIVE_BACKENDS = {
    "winit", "arboard", "rfd", "muda", "objc2-app-kit", "dear-imgui-winit", "dear-imgui-wgpu",
}
PORTABLE_UI = {
    "bedtime-ui", "bedtime-editor-widgets", "bedtime-api",
} | CONTENT_PLUGINS - {"bedtime-plugin-model-viewer", "bedtime-plugin-workspaces", "bedtime-plugin-mascots"}
IDE_SERVICES = {"bedtime-highlight", "bedtime-lsp", "bedtime-remote", "bedtime-git", "git2"}
FILE_BACKENDS = {"bedtime-remote", "bedtime-git", "git2"}
EXAMPLE_PLUGINS = {
    "bedtime-example-layouts": {"bedtime-plugin-layouts"},
    "bedtime-example-terminal": {"bedtime-plugin-terminal"},
    "bedtime-example-editor": {"bedtime-plugin-editor"},
    "bedtime-example-model": set(),
}


def dependency_tree(name, offline, features=None):
    # Resolve each consumer separately so desktop/PBR features cannot leak into
    # the API, workbench or headless services through workspace unification.
    tree = ["cargo", "tree", "--locked", "-p", name, "--no-default-features",
            "--edges", "normal,build", "--prefix", "none", "--format", "{p}"]
    if offline:
        tree.append("--offline")
    if features:
        tree += ["--features", features]
    output = subprocess.check_output(tree, cwd=ROOT, text=True)
    return {line.split()[0] for line in output.splitlines() if line.strip()} - {name}


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
    for package in packages.values():
        if package["source"] is not None or package["id"] in metadata["workspace_members"]:
            continue
        directory = Path(package["manifest_path"]).parent
        assert directory.is_relative_to(ROOT / "vendor"), f"Unexpected local dependency: {directory}"
        assert (directory / "BED_PATCHES.md").is_file(), f"Undocumented dependency fork: {directory}"
    for name, package in members.items():
        direct = {dependency["name"] for dependency in package["dependencies"] if dependency["kind"] != "dev"}
        unexpected = (direct & members.keys()) - ALLOWED_INTERNAL[name]
        assert not unexpected, f"{name}: forbidden direct dependencies: {sorted(unexpected)}"
        if name in EXAMPLE_PLUGINS or name == "bedtime-example-game-editor":
            assert "bedtime" in direct, f"{name} bypasses the owned launcher"
        names = dependency_tree(name, offline)
        forbidden = set()
        if name == "bedside-table":
            forbidden = {"bed", "bedtime-terminal", "bedtime-editor-widgets",
                         "bedtime-plugin-layouts", "bevy_pbr", "bevy_gltf", "rodio"} | CONTENT_PLUGINS | IDE_SERVICES
            # Core file operations currently share the local/remote backend
            # implementation. Its presence does not install SSH or IDE panels.
            forbidden -= FILE_BACKENDS
        elif name in {"bedtime-workbench", "bedtime-api", "bedtime-plugin-layouts", "bedtime-desktop"}:
            forbidden = {
                "bed", "bedtime-core", "bedtime", "bedtime-files", "bedtime-remote",
                "bedtime-editing", "bedtime-documents", "bedtime-lsp", "bedtime-git", "bedtime-terminal",
                "bevy_pbr", "bevy_gltf", "rodio",
            } | CONTENT_PLUGINS
            if name in {"bedtime-workbench", "bedtime-api", "bedtime-plugin-layouts"}:
                # Provider identities, lightweight file algorithms and state contracts
                # are foundations; authenticated transports remain feature gated.
                forbidden -= {"bedtime-files", "bedtime-editing"}
            if name != "bedtime-desktop":
                forbidden |= NATIVE_BACKENDS | {"bedtime-desktop", "bedtime-plugin-effects", "wgpu"}
        elif name == "bedtime-core" or name in EXAMPLE_PLUGINS:
            forbidden = {
                "bed", "bedtime", "bedtime-plugin-effects", "bedtime-plugin-idle",
                "bedtime-plugin-automation", "bevy_pbr", "bevy_gltf", "rodio", "bedtime-debugger",
            } | IDE_SERVICES | CONTENT_PLUGINS | {"bedtime-plugin-layouts", "bedtime-terminal"}
            forbidden -= FILE_BACKENDS
            allowed = EXAMPLE_PLUGINS.get(name, set())
            forbidden -= allowed
            if name in EXAMPLE_PLUGINS:
                forbidden.discard("bedtime")
            if "bedtime-plugin-terminal" in allowed:
                forbidden -= {"bedtime-terminal"}
        elif name in HEADLESS:
            forbidden = GUI
        elif name in PORTABLE_UI:
            forbidden = NATIVE_BACKENDS | {"bed", "bedtime", "bedtime-core", "bedtime-terminal", "bedtime-plugin-effects"}
            if name in {"bedtime-plugin-terminal", "bedtime-plugin-debugger"}:
                forbidden.remove("bedtime-terminal")
            if name == "bedtime-ui":
                forbidden |= {
                    "bedtime-documents", "bedtime-editor-widgets", "bedtime-highlight", "bedtime-lsp", "bedtime-api", "wgpu",
                } | CONTENT_PLUGINS
            elif name == "bedtime-editor-widgets":
                assert "bedtime-lsp" not in direct, "Editor widgets must not depend directly on LSP"
                forbidden |= {"bedtime-api"} | CONTENT_PLUGINS
            elif name == "bedtime-plugin-editor":
                forbidden |= IDE_SERVICES
        elif name in {"bedtime-plugin-model-viewer", "bedtime-plugin-workspaces", "bedtime-plugin-mascots"}:
            # The renderer uses desktop-owned memory assets and the shared GPU.
            forbidden = {"bed", "bedtime", "bedtime-plugin-effects"} | (CONTENT_PLUGINS - {name})
            if name == "bedtime-plugin-model-viewer": forbidden.add("bedtime-core")
            else: forbidden -= {"bedtime-plugin-mascots"}
        elif name == "bedtime":
            # The reusable launcher contains no bEd policy or optional feature plugin.
            forbidden = {"bed", "bedtime-terminal", "bedtime-debugger", "bevy_pbr", "bevy_gltf", "rodio"} | CONTENT_PLUGINS
            assert not direct & NATIVE_BACKENDS, (
                f"Owned launcher bypasses desktop APIs: {sorted(direct & NATIVE_BACKENDS)}"
            )
        if name in {"bedtime-example-game", "bedtime-example-game-runtime"}:
            forbidden |= set(ALLOWED_INTERNAL) - {name, "bedtime-example-game"}
            forbidden |= {"dear-imgui-rs", "dear-imgui-sys"}
        assert not names & forbidden, f"{name} crosses its dependency boundary: {sorted(names & forbidden)}"
    # The merged API must stay lean with documents disabled, including when only
    # GPU contracts are requested. Document contracts add no IDE services by default.
    for features in ("gpu", "documents", "documents,gpu", "documents,language-services"):
        names = dependency_tree("bedtime-api", offline, features)
        forbidden = NATIVE_BACKENDS | {"bed", "bedtime", "bedtime-core", "bedtime-desktop", "bedtime-terminal"} | CONTENT_PLUGINS
        if "documents" not in features:
            forbidden |= {"bedtime-documents"} | IDE_SERVICES
        elif "language-services" not in features:
            forbidden |= IDE_SERVICES
        if "gpu" not in features:
            forbidden.add("wgpu")
        assert not names & forbidden, f"bedtime-api/{features} crosses its boundary: {sorted(names & forbidden)}"
    for features in ("", "file-services", "remote"):
        names=dependency_tree("bedtime-documents", offline, features or None)
        forbidden=GUI | {"bedtime-highlight", "bedtime-lsp"}
        if not features: forbidden |= FILE_BACKENDS
        assert not names & forbidden, f"bedtime-documents/{features or 'base'} crosses its boundary: {sorted(names & forbidden)}"
    remote_core=dependency_tree("bedtime-core", offline, "remote")
    assert not remote_core & {"bedtime-highlight", "bedtime-lsp"}, "Remote core requires language services"
    assert not dependency_tree("bedtime", offline, "remote") & (CONTENT_PLUGINS | {"bed", "bedtime-highlight", "bedtime-lsp"}), "Remote launcher requires optional panels or language services"
    for features in (None, "documents"):
        names = dependency_tree("bedtime-plugin-automation", offline, features)
        forbidden = {"bed", "bedtime", "bedtime-core", "bedtime-plugin-workspaces"} | CONTENT_PLUGINS
        if features is None:
            # bedtime-api file identities include lightweight bedtime-editing algorithms.
            forbidden |= {"bedtime-documents"} | IDE_SERVICES
        assert not names & forbidden, f"Automation/{features or 'base'} requires application policy: {sorted(names & forbidden)}"
    assert not dependency_tree("bedtime-plugin-settings", offline) & {"bed", "bedtime", "bedtime-core", "bedtime-documents", "bedtime-lsp", "bedtime-remote"}, "Settings frontend requires application or editing services"
    assert not dependency_tree("bedtime-plugin-color-picker", offline) & {"bedtime-documents", "bedtime-lsp", "bedtime-core"}, "Settings metadata requires documents"
    print(f"All {len(members)} crates respect their production dependency boundaries.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    check(parser.parse_args().offline)
