#!/usr/bin/env python3
"""Check bEd's workspace membership and production dependency boundaries."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PANEL_PLUGINS = {
    "bed-plugin-editor", "bed-plugin-debugger", "bed-plugin-file-explorer",
    "bed-plugin-file-search", "bed-plugin-terminal", "bed-plugin-workspaces",
    "bed-plugin-settings", "bed-plugin-git", "bed-plugin-color-picker",
}
VIEWER_PLUGINS = {
    "bed-plugin-source-outline", "bed-plugin-image-viewer", "bed-plugin-model-viewer",
    "bed-plugin-font-viewer", "bed-plugin-audio-player", "bed-plugin-csv-table",
    "bed-plugin-markdown-preview", "bed-plugin-json-tree", "bed-plugin-sqlite-browser",
    "bed-plugin-http-client",
}
CONTENT_PLUGINS = PANEL_PLUGINS | VIEWER_PLUGINS | {"bed-plugin-mascots"}
APP_PROJECTIONS = {
    "bed-plugin-file-explorer", "bed-plugin-git", "bed-plugin-debugger",
    "bed-plugin-http-client",
}
# These are allowed direct edges, not required implementation dependencies.
# Removing an unused dependency must not require weakening a boundary check.
ALLOWED_INTERNAL = {
    "bed": {
        "bed-core", "bed-desktop", "bed-plugin-automation", "bed-plugin-idle",
        "bed-plugin-layouts", "bed-workbench", "bed-app-shell", "bed-runtime",
        "bed-app", "bed-api", "bed-editing", "bed-files", "bed-highlight",
        "bed-lsp", "bed-documents", "bed-ui", "bed-editor-widgets", "bed-app-config",
        "bed-terminal", "bed-debugger", "bed-plugin-effects", "bed-effects-config",
        "bed-remote", "bed-keybindings", "bed-storage",
    } | CONTENT_PLUGINS,
    "bed-api": {"bed-editing", "bed-documents", "bed-lsp", "bed-ui", "bed-keybindings", "bed-files", "bed-storage"},
    "bed-storage": set(),
    "bed-core": {
        "bed-desktop", "bed-runtime", "bed-workbench", "bed-api", "bed-documents",
        "bed-editing", "bed-ui", "bed-files", "bed-storage", "bed-lsp",
    },
    "bed-app": {
        "bed-plugin-layouts", "bed-workbench", "bed-api", "bed-editing", "bed-files",
        "bed-highlight", "bed-lsp", "bed-documents", "bed-ui", "bed-editor-widgets",
        "bed-app-config", "bed-remote", "bed-runtime", "bed-keybindings", "bed-core", "bed-storage", "bed-plugin-workspaces", "bed-terminal",
    } | APP_PROJECTIONS,
    "bed-app-config": {
        "bed-plugin-layouts", "bed-highlight", "bed-ui", "bed-effects-config",
        "bed-keybindings",
    },
    "bed-app-shell": {"bed-terminal"},
    "bed-runtime": set(),
    "bed-desktop": {"bed-ui", "bed-runtime"},
    "bed-workbench": {"bed-api", "bed-ui", "bed-keybindings"},
    "bed-plugin-layouts": {"bed-workbench", "bed-api"},
    "bed-example-layouts": {"bed-core", "bed-plugin-layouts", "bed-desktop", "bed-workbench", "bed-api"},
    "bed-example-terminal": {"bed-core", "bed-workbench", "bed-plugin-terminal"},
    "bed-example-editor": {"bed-core", "bed-plugin-editor", "bed-editing"},
    "bed-example-model": {"bed-core"},
    "bed-example-game": set(),
    "bed-example-game-runtime": {"bed-example-game"},
    "bed-example-game-editor": {"bed-example-game", "bed-core", "bed-desktop", "bed-ui"},
    "bed-plugin-automation": {
        "bed-desktop", "bed-workbench", "bed-api", "bed-app", "bed-documents", "bed-editing", "bed-plugin-workspaces",
    },
    "bed-plugin-idle": {"bed-desktop", "bed-plugin-mascots", "bed-api", "bed-ui"},
    "bed-plugin-editor": {
        "bed-workbench", "bed-editing", "bed-documents", "bed-ui", "bed-editor-widgets",
        "bed-highlight", "bed-lsp", "bed-api", "bed-keybindings",
    },
    "bed-plugin-debugger": {
        "bed-workbench", "bed-editing", "bed-documents", "bed-editor-widgets",
        "bed-debugger", "bed-ui", "bed-api", "bed-terminal", "bed-keybindings",
    },
    "bed-plugin-git": {
        "bed-git", "bed-editing", "bed-documents", "bed-editor-widgets", "bed-remote",
        "bed-ui", "bed-api", "bed-files",
    },
    "bed-plugin-file-explorer": {
        "bed-editing", "bed-files", "bed-remote", "bed-documents", "bed-ui",
        "bed-api", "bed-keybindings",
    },
    "bed-plugin-file-search": {
        "bed-editing", "bed-files", "bed-remote", "bed-documents", "bed-ui",
        "bed-api", "bed-keybindings",
    },
    "bed-plugin-terminal": {
        "bed-documents", "bed-api", "bed-terminal", "bed-ui", "bed-workbench",
        "bed-desktop", "bed-app-shell", "bed-keybindings",
    },
    "bed-plugin-workspaces": {"bed-documents", "bed-ui", "bed-api", "bed-plugin-mascots", "bed-core", "bed-files", "bed-storage", "bed-workbench"},
    "bed-plugin-mascots": {"bed-api", "bed-documents", "bed-runtime", "bed-ui", "bed-desktop"},
    "bed-plugin-color-picker": {"bed-api", "bed-editing", "bed-ui"},
    "bed-plugin-settings": {
        "bed-editing", "bed-documents", "bed-app-config", "bed-ui", "bed-api", "bed-keybindings",
    },
    "bed-debugger": set(),
    "bed-editing": set(),
    "bed-files": {"bed-editing", "bed-remote"},
    "bed-highlight": {"bed-editing"},
    "bed-lsp": {"bed-editing", "bed-remote"},
    "bed-documents": {"bed-editing", "bed-files", "bed-highlight", "bed-lsp", "bed-remote"},
    "bed-ui": {"bed-highlight"},
    "bed-editor-widgets": {"bed-editing", "bed-documents", "bed-highlight", "bed-ui", "bed-keybindings"},
    "bed-terminal": {"bed-editing", "bed-remote", "bed-ui"},
    "bed-plugin-effects": {"bed-effects-config", "bed-desktop"},
    "bed-effects-config": set(),
    "bed-remote": {"bed-git", "bed-editing"},
    "bed-remote-agent": {"bed-remote", "bed-files"},
    "bed-git": set(),
    "bed-keybindings": set(),
    "bed-plugin-source-outline": {"bed-editing", "bed-highlight", "bed-api", "bed-ui"},
    "bed-plugin-image-viewer": {"bed-editing", "bed-api", "bed-ui", "bed-keybindings"},
    "bed-plugin-model-viewer": {"bed-desktop", "bed-editing", "bed-api", "bed-ui", "bed-runtime", "bed-keybindings"},
    "bed-plugin-font-viewer": {"bed-editing", "bed-api", "bed-ui"},
    "bed-plugin-audio-player": {"bed-editing", "bed-api", "bed-ui", "bed-keybindings"},
    "bed-plugin-csv-table": {"bed-editing", "bed-api", "bed-documents", "bed-ui", "bed-keybindings"},
    "bed-plugin-markdown-preview": {
        "bed-editing", "bed-api", "bed-plugin-image-viewer", "bed-remote", "bed-ui", "bed-documents", "bed-files",
    },
    "bed-plugin-json-tree": {"bed-editing", "bed-api"},
    "bed-plugin-sqlite-browser": {"bed-editing", "bed-api", "bed-keybindings"},
    "bed-plugin-http-client": {"bed-editing", "bed-api", "bed-ui"},
}
HEADLESS = {
    "bed-editing", "bed-files", "bed-highlight", "bed-lsp", "bed-documents",
    "bed-remote", "bed-remote-agent", "bed-debugger", "bed-effects-config", "bed-git",
    "bed-terminal", "bed-storage",
}
GUI = {
    "dear-imgui-rs", "dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu",
    "winit", "wgpu", "arboard", "rfd", "resvg", "muda", "objc2-app-kit",
}
NATIVE_BACKENDS = {
    "winit", "arboard", "rfd", "muda", "objc2-app-kit", "dear-imgui-winit", "dear-imgui-wgpu",
}
PORTABLE_UI = {
    "bed-ui", "bed-editor-widgets", "bed-app-config", "bed-api", "bed-app-shell",
} | CONTENT_PLUGINS - {"bed-plugin-model-viewer", "bed-plugin-workspaces", "bed-plugin-mascots"}
IDE_SERVICES = {"bed-highlight", "bed-lsp", "bed-remote", "bed-git", "git2"}
FILE_BACKENDS = {"bed-remote", "bed-git", "git2"}
EXAMPLE_PLUGINS = {
    "bed-example-layouts": {"bed-plugin-layouts"},
    "bed-example-terminal": {"bed-plugin-terminal"},
    "bed-example-editor": {"bed-plugin-editor"},
    "bed-example-model": set(),
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
        names = dependency_tree(name, offline)
        forbidden = set()
        if name in {"bed-workbench", "bed-api", "bed-plugin-layouts", "bed-desktop"}:
            forbidden = {
                "bed", "bed-core", "bed-app", "bed-app-config", "bed-files", "bed-remote",
                "bed-editing", "bed-documents", "bed-lsp", "bed-git", "bed-terminal",
                "bevy_pbr", "bevy_gltf", "rodio",
            } | CONTENT_PLUGINS
            if name in {"bed-workbench", "bed-api", "bed-plugin-layouts"}:
                # Provider identities, lightweight file algorithms and state contracts
                # are foundations; authenticated transports remain feature gated.
                forbidden -= {"bed-files", "bed-editing"}
            if name != "bed-desktop":
                forbidden |= NATIVE_BACKENDS | {"bed-desktop", "bed-plugin-effects", "wgpu"}
        elif name == "bed-core" or name in EXAMPLE_PLUGINS:
            forbidden = {
                "bed", "bed-app", "bed-app-config", "bed-plugin-effects", "bed-plugin-idle",
                "bed-plugin-automation", "bevy_pbr", "bevy_gltf", "rodio", "bed-debugger",
            } | IDE_SERVICES | CONTENT_PLUGINS | {"bed-plugin-layouts", "bed-terminal", "bed-app-shell"}
            forbidden -= FILE_BACKENDS
            allowed = EXAMPLE_PLUGINS.get(name, set())
            forbidden -= allowed
            if "bed-plugin-terminal" in allowed:
                forbidden -= {"bed-terminal", "bed-app-shell"}
        elif name in HEADLESS:
            forbidden = GUI
        elif name in PORTABLE_UI:
            forbidden = NATIVE_BACKENDS | {"bed", "bed-app", "bed-core", "bed-terminal", "bed-plugin-effects"}
            if name in {"bed-plugin-terminal", "bed-plugin-debugger", "bed-app-shell"}:
                forbidden.remove("bed-terminal")
            if name == "bed-ui":
                forbidden |= {
                    "bed-documents", "bed-editor-widgets", "bed-highlight", "bed-lsp", "bed-api", "wgpu",
                } | CONTENT_PLUGINS
            elif name == "bed-editor-widgets":
                assert "bed-lsp" not in direct, "Editor widgets must not depend directly on LSP"
                forbidden |= {"bed-api"} | CONTENT_PLUGINS
            elif name == "bed-plugin-editor":
                forbidden |= IDE_SERVICES
        elif name in {"bed-plugin-model-viewer", "bed-plugin-workspaces", "bed-plugin-mascots"}:
            # The renderer uses desktop-owned memory assets and the shared GPU.
            forbidden = {"bed", "bed-app", "bed-app-config", "bed-plugin-effects"} | (CONTENT_PLUGINS - {name})
            if name == "bed-plugin-model-viewer": forbidden.add("bed-core")
            else: forbidden -= {"bed-plugin-mascots"}
        elif name == "bed-runtime":
            forbidden = NATIVE_BACKENDS | {
                "bed", "bed-app", "bed-api", "dear-imgui-rs", "dear-imgui-sys",
                "bed-editor-widgets", "bed-app-config", "bed-plugin-effects",
            } | CONTENT_PLUGINS
        elif name == "bed-app":
            forbidden = {
                "bed", "bed-plugin-effects", "bed-terminal",
            } | CONTENT_PLUGINS
            forbidden -= {"bed-plugin-workspaces"}
            assert not direct & (NATIVE_BACKENDS - {"rfd"}), (
                f"Application integration imports native host APIs: {sorted(direct & (NATIVE_BACKENDS - {'rfd'}))}"
            )
        if name in {"bed-example-game", "bed-example-game-runtime"}:
            forbidden |= set(ALLOWED_INTERNAL) - {name, "bed-example-game"}
            forbidden |= {"dear-imgui-rs", "dear-imgui-sys"}
        assert not names & forbidden, f"{name} crosses its dependency boundary: {sorted(names & forbidden)}"
    # The merged API must stay lean with documents disabled, including when only
    # GPU contracts are requested. Document contracts add no IDE services by default.
    for features in ("gpu", "documents", "documents,gpu", "documents,language-services"):
        names = dependency_tree("bed-api", offline, features)
        forbidden = NATIVE_BACKENDS | {"bed", "bed-app", "bed-app-config", "bed-core", "bed-desktop", "bed-terminal"} | CONTENT_PLUGINS
        if "documents" not in features:
            forbidden |= {"bed-documents"} | IDE_SERVICES
        elif "language-services" not in features:
            forbidden |= IDE_SERVICES
        if "gpu" not in features:
            forbidden.add("wgpu")
        assert not names & forbidden, f"bed-api/{features} crosses its boundary: {sorted(names & forbidden)}"
    for features in ("", "file-services", "remote"):
        names=dependency_tree("bed-documents", offline, features or None)
        forbidden=GUI | {"bed-highlight", "bed-lsp"}
        if not features: forbidden |= FILE_BACKENDS
        assert not names & forbidden, f"bed-documents/{features or 'base'} crosses its boundary: {sorted(names & forbidden)}"
    remote_core=dependency_tree("bed-core", offline, "remote")
    assert not remote_core & {"bed-highlight", "bed-lsp"}, "Remote core requires language services"
    assert not dependency_tree("bed-plugin-automation", offline, "app") & {"bed-plugin-mascots"}, "Application automation requires workspace presentation"
    assert not dependency_tree("bed-plugin-color-picker", offline) & {"bed-documents", "bed-lsp", "bed-core"}, "Settings metadata requires documents"
    print(f"All {len(members)} crates respect their production dependency boundaries.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use cached Cargo dependencies only")
    check(parser.parse_args().offline)
