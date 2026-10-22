#!/usr/bin/env python3
"""Native Files/preview/watcher/preferences smoke; all filesystem changes use a temp project.

Build bed2 with --features automation, then pass --binary to a pinned executable
(or set BED2_FILETREE_BINARY / BED2_BINARY). A private executable copy also protects
this run from concurrent builds replacing target/debug/bed2. No repository/project
file is used as a mutation fixture. Protocol exit is not used to test managed
shutdown: workspace.close must drain workers and flush preferences first.
Clipboard cases use the application's ordinary native clipboard (overwriting its
contents with fixture paths); injected external-drop messages are explicitly NOT
an OS Finder/Explorer drag. Internal drag and clipboard commands use real UI input.
"""
import argparse
import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

from automation import Application, AutomationError
from smoke_bed2 import open_panel, prepare_demo_layout


PANEL_KEY = "bedtime.filetree"
PRIMARY = "SuperLeft" if sys.platform == "darwin" else "ControlLeft"


def inspection(app):
    return app.request("app.inspect_filetree")


def state(app, panel):
    value = next((value for value in inspection(app)["panels"] if value["entity"] == panel), None)
    if value is None:
        raise AutomationError(f"Files panel disappeared: {panel}")
    return value


def wait_value(app, predicate, *, seconds=20):
    # Watchers, scanning and debounce use wall time, not configure.fixed_dt.
    deadline = time.monotonic() + seconds
    value = None
    while time.monotonic() < deadline:
        value = inspection(app)
        if predicate(value):
            return value
        app.request("wait_frames", frames=2)
    raise AutomationError(f"Files condition did not settle; last observation: {json.dumps(value)}")


def wait_tree(app, panel, predicate=lambda _: True):
    value = wait_value(app, lambda value: any(
        tree["entity"] == panel and predicate(tree) for tree in value["panels"]))
    tree = next(tree for tree in value["panels"] if tree["entity"] == panel)
    assert tree["error"] is None and tree["snapshot_error"] is None, tree
    assert tree["open_error"] is None, tree
    return tree


def paths(tree):
    return {row["path"] for row in tree["rows"]}


def key(app, code):
    app.input({"type": "key", "code": code, "pressed": True},
              {"type": "key", "code": code, "pressed": False})


def primary(app, code):
    app.input({"type": "key", "code": PRIMARY, "pressed": True},
              {"type": "key", "code": code, "pressed": True},
              {"type": "key", "code": code, "pressed": False},
              {"type": "key", "code": PRIMARY, "pressed": False})


def fixture_contents(directory):
    """Payload evidence; app metadata can autosave focus/layout during read-only UI.

    Visibility storage itself is asserted separately, including exact bytes/mtime.
    """
    return {path.relative_to(directory).as_posix(): None if path.is_dir() else path.read_bytes()
            for path in directory.rglob("*") if ".bed" not in path.relative_to(directory).parts}


def select_all_steps():
    return [{"type": "key", "code": PRIMARY, "pressed": True},
            {"type": "key", "code": "KeyA", "pressed": True},
            {"type": "key", "code": "KeyA", "pressed": False},
            {"type": "key", "code": PRIMARY, "pressed": False}]


def row_position(app, panel, path):
    app.request("panel.focus", panel=panel)
    # Animation uses wall time, not configure.fixed_dt. Observe its actual
    # completion before deriving click geometry, not a guessed frame duration.
    path = str(path)
    tree = wait_tree(app, panel, lambda tree: tree["presentation"]["visible"]
                     and not tree["animating"] and not tree["loading"] and path in paths(tree))
    if not any(row["path"] == path for row in tree["row_rects"]):
        # Expanded copy/drop fixtures may push a later row below the viewport.
        # Reveal it with real navigation, not by mutating retained scroll state.
        index = next(index for index, row in enumerate(tree["rows"]) if row["path"] == path)
        key(app, "Home")
        for _ in range(index):
            key(app, "ArrowDown")
        tree = wait_tree(app, panel, lambda tree: not tree["animating"]
                         and any(row["path"] == path for row in tree["row_rects"]))
    rect = next(row["rect"] for row in tree["row_rects"] if row["path"] == path)
    zoom = app.inspect()["ui"]["surfaces"][0]["zoom"]
    # Use the label side, not the disclosure triangle or the scrollbar.
    return [(rect["min"][0] + (rect["max"][0] - rect["min"][0]) * 0.60) * zoom,
            (rect["min"][1] + rect["max"][1]) * 0.5 * zoom]


def click_row(app, panel, path, *, button="Left"):
    app.input({"type": "click", "position": row_position(app, panel, path), "button": button})


def menu(app, panel, path, label):
    click_row(app, panel, path, button="Right")
    snapshot = app.wait_until(lambda snapshot: any(
        text["text"] == label or text["text"].startswith(label + "     ")
        for text in snapshot["ui"]["texts"]))
    full_label = next(text["text"] for text in snapshot["ui"]["texts"]
                      if text["text"] == label or text["text"].startswith(label + "     "))
    app.click_text(full_label)


def edit_name(app, name):
    # Inline fields acquire focus on their first draw. Exercise ordinary keyboard
    # selection/text/Enter rather than an application mutation command.
    app.request("wait_frames", frames=3)
    app.input(*select_all_steps(), {"type": "text", "text": name},
              {"type": "key", "code": "Enter", "pressed": True},
              {"type": "key", "code": "Enter", "pressed": False})


def preview(app, path, *, layout=None, pinned=None, content=None, kind=None):
    path = str(path)
    def matches(value):
        return (value["path"] == path and not value["loading"] and value["error"] is None
                and (layout is None or value["layout"] == layout)
                and (pinned is None or value["pinned"] == pinned)
                and (content is None or content in (value["content"] or ""))
                and (kind is None or value["kind"] == kind))
    value = wait_value(app, lambda value: any(matches(preview) for preview in value["previews"]))
    return next(preview for preview in value["previews"] if matches(preview))


def preference(value, workspace, root):
    return next((record for preferences in value["preferences"]
                 if preferences["workspace"] == workspace
                 for record in preferences["roots"] if record["root"] == str(root)), None)


def storage(value, workspace):
    return next((record for record in value["storage"] if record["workspace"] == workspace), None)


def capture(app, filename, *, different_from=None, validate=True):
    result = app.capture(filename)
    if not validate:
        return result  # Diagnostic pixels only; explicitly NOT a rendering pass.
    assert result["nontransparent_pixels"] > 0, (filename, result)
    if different_from is not None:
        assert (result["width"], result["height"]) == (
            different_from["width"], different_from["height"]), result
        assert result["hash"] != different_from["hash"], (filename, result)
    return result


def open_native(app):
    snapshot = app.wait_until(lambda snapshot: bool(snapshot["workbench"]["workspaces"])
                             and snapshot["render"]["pending_pipelines"] == 0
                             and bool(snapshot["ui"]["surfaces"]))
    render_frame = snapshot["render"]["frames"]
    app.input({"type": "click", "position": [300, 180]})
    snapshot = app.wait_until(lambda snapshot: snapshot["windows"][0]["focused"]
                             and snapshot["render"]["frames"] >= render_frame + 2
                             and snapshot["render"]["pending_pipelines"] == 0)
    wait_value(app, lambda value: any(root["session_ready"] for root in value["workspace_roots"]))
    snapshot = app.inspect()
    workspace = next(workspace for workspace in snapshot["workbench"]["workspaces"] if workspace["active"])
    # A restarted folder can now restore its previous panels. Give this focused
    # preferences scenario its own fresh layout instead of assuming no session.
    if inspection(app)["panels"]:
        app.request("layout.create", name="Files preference check", activate=True)
        snapshot = app.inspect()
        workspace = next(workspace for workspace in snapshot["workbench"]["workspaces"] if workspace["active"])
    layout = next(layout for layout in workspace["layouts"] if layout["active"])
    # The application's native View menu opens the scenario's tree.
    for viewport in snapshot["viewports"]:
        app.request("panel.close", panel=viewport["entity"])
    panel = open_panel(app, "Files")
    value = wait_value(app, lambda value: any(tree["entity"] == panel for tree in value["panels"]))
    return workspace["entity"], layout["entity"], panel


def run(binary, output, fixture, *, functional_only=False, wide_entries=0):
    def take_capture(app, filename, **kwargs):
        return capture(app, filename, validate=not functional_only, **kwargs)

    root = fixture / "project"
    alternate = fixture / "alternate"
    slow = fixture / "switch-in-flight"
    root.mkdir()
    alternate.mkdir()
    slow.mkdir()
    (root / "Alpha").mkdir()
    (root / "Alpha" / "nested.txt").write_text("NESTED_CONTENT\n", encoding="utf-8")
    (root / "a.txt").write_text("REAL_FILE_A\nnot the memory document demo\n", encoding="utf-8")
    (root / "b.txt").write_text("REAL_FILE_B\n", encoding="utf-8")
    (root / ".hidden.txt").write_text("HIDDEN_FIXTURE\n", encoding="utf-8")
    (root / "binary.bin").write_bytes(b"\x00\xff\x80binary")
    with (root / "large.bin").open("wb") as large:
        large.truncate(256 * 1024 * 1024 + 1)
    (alternate / "foreign.txt").write_text("INDEPENDENT_ROOT\n", encoding="utf-8")
    # Flat, finite fixture: a switch can overlap real worker directory I/O, but
    # the assertion never relies on a guessed scan duration or a required race.
    for index in range(512):
        (slow / f"old-{index:04}.txt").write_text("OLD_ROOT\n", encoding="utf-8")
    outside = fixture / "outside-project"
    outside.mkdir()
    incoming = outside / "incoming.txt"
    incoming.write_bytes(b"\x00EXTERNAL_DROP_FILE\xff")
    incoming_folder = outside / "IncomingFolder"
    incoming_folder.mkdir()
    (incoming_folder / "child.txt").write_text("EXTERNAL_FOLDER_CHILD\n", encoding="utf-8")
    (incoming_folder / "nested").mkdir()
    (incoming_folder / "nested" / "deep.bin").write_bytes(b"\xffDEEP\x00")
    root_drop = outside / "root-drop.txt"
    root_drop.write_text("ROOT_DROP\n", encoding="utf-8")
    child_drop = outside / "onto-child.txt"
    child_drop.write_text("CHILD_DROP\n", encoding="utf-8")
    read_only_drop = outside / "read-only-drop.txt"
    read_only_drop.write_text("MUST_NOT_COPY\n", encoding="utf-8")
    link = root / "link-to-a.txt"
    if sys.platform == "darwin":
        link.symlink_to(root / "a.txt")
    outside_before = fixture_contents(outside)
    wide = fixture / "wide-root"
    if wide_entries:
        wide.mkdir()
        for index in range(wide_entries):
            (wide / f"entry-{index:06}.txt").touch()
    wide_observation = None
    project_preferences = root / ".bed" / "filetree2.json"
    captures = []
    with Application([str(binary), str(root)], output=output / "first", timeout=60) as app:
        prepare_demo_layout(app, viewport=False)
        # Do not install configure.clipboard: on macOS these cases must exercise
        # the actual system backend and its native cut-ownership observation.
        app.request("configure", fixed_dt=1 / 60)
        workspace, layout, first = open_native(app)
        project_preferences = Path(storage(wait_value(app, lambda value: storage(value, workspace) is not None), workspace)["path"])
        initial = wait_tree(app, first, lambda tree: tree["root"] == str(root)
                            and not tree["loading"] and str(root / "a.txt") in paths(tree))
        assert str(root / ".hidden.txt") not in paths(initial), initial
        assert str(root / "Alpha" / "nested.txt") not in paths(initial), initial
        assert initial["cached_directories"] == 1, initial
        assert not (root / ".bed").exists(), "Startup must not create preference storage"
        captures.append(take_capture(app, "01-files.png"))
        # Root is a real collapsible row, with no duplicate name/refresh toolbar.
        click_row(app, first, root)
        wait_tree(app, first, lambda tree: len(tree["rows"]) == 1 and not tree["animating"])
        captures.append(take_capture(app, "01a-collapsed-root.png", different_from=captures[-1]))
        key(app, "ArrowRight")
        wait_tree(app, first, lambda tree: str(root / "a.txt") in paths(tree) and not tree["animating"])
        app.request("app.set_filetree_settings", panel=first, read_only=True, indent_size=28)
        assert state(app, first)["settings"] == {"read_only": True, "indent_size": 28.0}
        app.input({"type": "move", "position": row_position(app, first, root)})
        app.request("app.drop_files", paths=[str(read_only_drop)])
        app.request("wait_frames", frames=8)
        assert not (root / read_only_drop.name).exists() and state(app, first)["error"] is None
        captures.append(take_capture(app, "01b-read-only-indent.png", different_from=captures[-1]))
        app.request("app.set_filetree_settings", panel=first, read_only=False, indent_size=16)
        app.input({"type": "move", "position": row_position(app, first, root)})
        app.request("app.drop_files", paths=[str(root_drop)])
        wait_tree(app, first, lambda tree: (root / root_drop.name).is_file() and not tree["busy"])
        assert (root / root_drop.name).read_bytes() == root_drop.read_bytes()

        # Navigation uses native keyboard input and only loads expanded children.
        click_row(app, first, root / "Alpha")
        # The click already admits expansion. A fallback Right based on an older
        # snapshot races the load: it can select the new child just before Left,
        # making Left correctly select its parent instead of collapsing it.
        expanded = wait_tree(app, first, lambda tree: str(root / "Alpha" / "nested.txt") in paths(tree))
        assert expanded["selection"] == [str(root / "Alpha")], expanded
        assert expanded["cached_directories"] >= 2, expanded
        key(app, "ArrowLeft")
        wait_tree(app, first, lambda tree: str(root / "Alpha" / "nested.txt") not in paths(tree))
        key(app, "ArrowRight")
        wait_tree(app, first, lambda tree: str(root / "Alpha" / "nested.txt") in paths(tree))
        key(app, "ArrowLeft")
        wait_tree(app, first, lambda tree: str(root / "Alpha" / "nested.txt") not in paths(tree))

        # Single click opens/replaces one file preview. The source tree stays
        # beside it, allowing a real second click to pin the SAME viewer.
        click_row(app, first, root / "a.txt")
        a = preview(app, root / "a.txt", layout=layout, pinned=False, content="REAL_FILE_A")
        assert a["title"] == "a.txt", a
        app.request("wait_frames", frames=25)  # separate from the earlier click
        position = row_position(app, first, root / "a.txt")
        app.input({"type": "click", "position": position}, {"type": "wait", "frames": 1},
                  {"type": "click", "position": position})
        pinned_a = preview(app, root / "a.txt", layout=layout, pinned=True, content="REAL_FILE_A")
        assert pinned_a["entity"] == a["entity"], (a, pinned_a)
        wait_tree(app, first, lambda tree: tree["activations"]["DoubleClick"] >= 1)
        click_row(app, first, root / "b.txt")
        b = preview(app, root / "b.txt", layout=layout, pinned=False, content="REAL_FILE_B")
        assert b["entity"] != a["entity"], (a, b)
        key(app, "Enter")
        b = preview(app, root / "b.txt", layout=layout, pinned=True, content="REAL_FILE_B")
        wait_tree(app, first, lambda tree: tree["activations"]["Keyboard"] >= 1)
        menu(app, first, root / "a.txt", "Open")
        preview(app, root / "a.txt", layout=layout, pinned=True, content="REAL_FILE_A")
        tree = wait_tree(app, first, lambda tree: tree["activations"]["ContextMenu"] >= 1)
        assert tree["activations"]["SingleClick"] >= 2, tree
        local = [value for value in inspection(app)["previews"] if value["layout"] == layout]
        assert len(local) == 2 and len({value["path"] for value in local}) == 2, local
        captures.append(take_capture(app, "02-file-previews.png", different_from=captures[-1]))

        # Non-text and oversized sources never become editable/fake documents.
        click_row(app, first, root / "binary.bin")
        binary_preview = preview(app, root / "binary.bin", layout=layout, pinned=False, kind="metadata")
        assert "Binary" in binary_preview["content"], binary_preview
        click_row(app, first, root / "large.bin")
        large_preview = preview(app, root / "large.bin", layout=layout, pinned=False, kind="metadata")
        assert binary_preview["entity"] == large_preview["entity"], (binary_preview, large_preview)
        assert "256 MiB" in large_preview["content"], large_preview

        # External observations must arrive from the watcher/poll fallback, not
        # from an automation mutation or an explicit Refresh action.
        external = root / "watch-created.txt"
        external.write_text("WATCH_CREATE\n", encoding="utf-8")
        wait_tree(app, first, lambda tree: str(external) in paths(tree))
        renamed_external = root / "watch-renamed.txt"
        external.rename(renamed_external)
        observed = wait_tree(app, first, lambda tree: str(renamed_external) in paths(tree)
                             and str(external) not in paths(tree))
        assert observed["version"] > initial["version"], observed

        # Inline rename and creation go through the real context menu/text field.
        menu(app, first, root / "b.txt", "Rename…")
        edit_name(app, "a.txt")
        conflict = "An item with that name already exists"
        error_ui = app.wait_until(lambda snapshot: any(conflict in item["text"] for item in snapshot["ui"]["texts"]))
        assert not any("os error" in item["text"] for item in error_ui["ui"]["texts"]), "Technical diagnostics leaked into the default dialog"
        assert (root / "a.txt").read_text(encoding="utf-8").startswith("REAL_FILE_A")
        assert (root / "b.txt").read_text(encoding="utf-8") == "REAL_FILE_B\n"
        captures.append(take_capture(app, "02a-human-readable-error-dialog.png", different_from=captures[-1]))
        app.click_text("OK")
        app.wait_until(lambda snapshot: all(not surface["capture"]["modal"] for surface in snapshot["ui"]["surfaces"]))
        edit_name(app, "b-renamed.txt")
        renamed_b = root / "b-renamed.txt"
        wait_tree(app, first, lambda tree: str(renamed_b) in paths(tree)
                  and str(root / "b.txt") not in paths(tree))
        moved_b = preview(app, renamed_b, layout=layout, pinned=True, content="REAL_FILE_B")
        assert moved_b["entity"] == b["entity"] and moved_b["document"] == b["document"], (b, moved_b)
        assert moved_b["document_info"]["revision"] == b["document_info"]["revision"], (b, moved_b)
        assert renamed_b.read_text(encoding="utf-8") == "REAL_FILE_B\n"
        menu(app, first, root, "New File…")
        edit_name(app, "created-inline.txt")
        wait_tree(app, first, lambda tree: str(root / "created-inline.txt") in paths(tree))
        assert (root / "created-inline.txt").is_file()
        menu(app, first, root, "New Folder…")
        edit_name(app, "CreatedFolder")
        wait_tree(app, first, lambda tree: str(root / "CreatedFolder") in paths(tree))
        assert (root / "CreatedFolder").is_dir()
        target = root / "CreatedFolder"

        # Internal move uses native pointer drag through the tree's egui payload,
        # not the application drop hook or a direct filesystem operation.
        drag_source = root / "created-inline.txt"
        start = row_position(app, first, drag_source)
        end = row_position(app, first, target)
        app.input({"type": "drag", "from": start, "to": end, "frames": 18})
        wait_tree(app, first, lambda tree: not tree["busy"] and not drag_source.exists()
                  and (target / drag_source.name).is_file())
        assert fixture_contents(target) == {"created-inline.txt": b""}

        # External-drop bridge evidence: inject ONLY Bevy's upstream DroppedFile
        # messages, with the pointer over a directory row. bevy_egui must convert
        # them to raw.dropped_files and the ordinary tree UI must consume once.
        # This is intentionally not described as an OS Finder/Explorer drag.
        position = row_position(app, first, target)
        app.input({"type": "move", "position": position})
        drop = app.request("app.drop_files", paths=[str(incoming), str(incoming_folder)])
        assert drop["injected_events"] == 2 and "injected event, not OS drag" in drop["boundary"], drop
        expected_target = {"created-inline.txt": b"", "incoming.txt": incoming.read_bytes(),
                           "IncomingFolder": None}
        expected_target.update({"IncomingFolder/" + path: data
                                for path, data in fixture_contents(incoming_folder).items()})
        wait_tree(app, first, lambda tree: not tree["busy"]
                  and (target / "IncomingFolder" / "nested" / "deep.bin").is_file()
                  and (target / "incoming.txt").is_file())
        app.request("wait_frames", frames=25)
        assert fixture_contents(target) == expected_target, fixture_contents(target)
        assert fixture_contents(outside) == outside_before, "External drops must COPY, preserving sources"
        assert not (root / "incoming.txt").exists() and not (root / "IncomingFolder").exists(), \
            "Drops were routed to the root instead of the hovered folder"
        assert not list(root.rglob("* copy*")), "A replayed drop created an extra keep-both copy"
        # Hovering an expanded child file chooses its enclosing folder, never
        # the file or an unconditional root fallback.
        moved_child = target / "created-inline.txt"
        wait_tree(app, first, lambda tree: str(moved_child) in paths(tree) and not tree["loading"])
        app.input({"type": "move", "position": row_position(app, first, moved_child)})
        app.request("app.drop_files", paths=[str(child_drop)])
        wait_tree(app, first, lambda tree: not tree["busy"] and (target / child_drop.name).is_file())
        expected_target[child_drop.name] = child_drop.read_bytes()
        assert not (root / child_drop.name).exists()
        assert fixture_contents(target) == expected_target

        # Duplicate/Copy/Paste traverse actual context menu dispatch and the
        # shared clipboard. On macOS, reject a silent memory-backend fallback.
        clipboard_before = inspection(app)["clipboard"]
        assert clipboard_before is not None, "Application clipboard is unavailable"
        if sys.platform == "darwin":
            assert clipboard_before["mode"] == "System", clipboard_before
        menu(app, first, root / "binary.bin", "Duplicate")
        duplicate = root / "binary copy.bin"
        wait_tree(app, first, lambda tree: not tree["busy"] and duplicate.is_file())
        assert duplicate.read_bytes() == (root / "binary.bin").read_bytes()
        assert sorted(path.name for path in root.glob("binary*")) == ["binary copy.bin", "binary.bin"]
        menu(app, first, duplicate, "Copy")
        menu(app, first, target, "Paste")
        copied = target / duplicate.name
        wait_tree(app, first, lambda tree: not tree["busy"] and copied.is_file())
        assert duplicate.is_file() and copied.read_bytes() == duplicate.read_bytes()
        expected_target[duplicate.name] = duplicate.read_bytes()
        assert fixture_contents(target) == expected_target
        clipboard_after_copy = inspection(app)["clipboard"]
        assert clipboard_after_copy["revision"] > clipboard_before["revision"], clipboard_after_copy

        cut_evidence = "not run: native clipboard ownership is unsupported outside macOS"
        if sys.platform == "darwin":
            # Reveal the copied child without issuing an open/transfer command.
            if str(copied) not in paths(state(app, first)):
                click_row(app, first, target)
                wait_tree(app, first, lambda tree: str(copied) in paths(tree))
            menu(app, first, copied, "Cut")
            click_row(app, first, root / "Alpha")
            cut_destination = root / "Alpha" / duplicate.name
            primary(app, "KeyV")
            app.request("wait_frames", frames=5)
            assert copied.is_file() and not cut_destination.exists(), "Deferred operation shortcut still fired"
            menu(app, first, root / "Alpha", "Paste")
            wait_tree(app, first, lambda tree: not tree["busy"] and not copied.exists()
                      and cut_destination.is_file())
            assert cut_destination.read_bytes() == duplicate.read_bytes() and duplicate.is_file()
            del expected_target[duplicate.name]

            # Replace an admitted cut with a DIFFERENT file via its Copy menu.
            # Pasting must now copy that new file and leave the formerly cut file
            # intact; no old cut authority may survive clipboard replacement.
            if str(cut_destination) not in paths(state(app, first)):
                click_row(app, first, root / "Alpha")
                wait_tree(app, first, lambda tree: str(cut_destination) in paths(tree))
            menu(app, first, cut_destination, "Cut")
            menu(app, first, root / "binary.bin", "Copy")
            menu(app, first, target, "Paste")
            replacement = target / "binary.bin"
            wait_tree(app, first, lambda tree: not tree["busy"] and replacement.is_file())
            assert cut_destination.is_file(), "Replacing the clipboard must revoke the earlier cut"
            assert cut_destination.read_bytes() == duplicate.read_bytes()
            assert (root / "binary.bin").is_file() and replacement.read_bytes() == (root / "binary.bin").read_bytes()
            expected_target["binary.bin"] = replacement.read_bytes()
            # arboard canonicalizes this native payload. A valid local Cut must
            # still move the selected link, not its target or a copied target.
            assert link.is_symlink() and link.resolve() == root / "a.txt"
            menu(app, first, link, "Copy")
            menu(app, first, root / "Alpha", "Paste")
            copied_link = root / "Alpha" / link.name
            wait_tree(app, first, lambda tree: not tree["busy"] and copied_link.is_symlink())
            assert link.is_symlink() and copied_link.resolve() == root / "a.txt"
            menu(app, first, link, "Cut")
            menu(app, first, target, "Paste")
            moved_link = target / link.name
            wait_tree(app, first, lambda tree: not tree["busy"] and moved_link.is_symlink() and not link.exists())
            assert moved_link.resolve() == root / "a.txt"
            assert (root / "a.txt").read_text(encoding="utf-8").startswith("REAL_FILE_A")
            expected_target[link.name] = (root / "a.txt").read_bytes()
            cut_evidence = "macOS System clipboard: context Copy/Cut/Paste preserves selected symlinks and moves only original entries; removed shortcut is inert; Copy replacement revokes cut"
        app.request("wait_frames", frames=25)
        assert fixture_contents(target) == expected_target, fixture_contents(target)
        assert fixture_contents(outside) == outside_before
        assert inspection(app)["clipboard"]["mode"] == clipboard_before["mode"]
        captures.append(take_capture(app, "03-watcher-and-operations.png", different_from=captures[-1]))

        # Visibility changes are workspace preferences, not WorkspaceChanged or a
        # layout snapshot. They must be restored by actual app-owned file I/O.
        menu(app, first, root, "Show Hidden Files")
        wait_tree(app, first, lambda tree: str(root / ".hidden.txt") in paths(tree))
        menu(app, first, renamed_external, "Hide Selected")
        wait_tree(app, first, lambda tree: str(renamed_external) not in paths(tree))
        value = wait_value(app, lambda value: preference(value, workspace, root) is not None
                           and preference(value, workspace, root)["hide_hidden"] is False
                           and "watch-renamed.txt" in preference(value, workspace, root)["hidden_paths"])
        assert storage(value, workspace)["root"] == str(root), value
        wait_value(app, lambda value: not storage(value, workspace)["pending"]
                   and storage(value, workspace)["error"] is None)
        assert project_preferences.is_file()
        assert "version: 1" in project_preferences.read_text(encoding="utf-8")
        captures.append(take_capture(app, "04-visibility.png", different_from=captures[-1]))

        # Another panel/layout has independent root, selection and preview slots.
        other_layout = app.request("layout.create", name="Independent Files", activate=False)["layout"]
        second = open_panel(app, "Files", layout=other_layout)
        app.request("app.set_filetree_root", panel=second, path=str(alternate))
        independent = wait_tree(app, second, lambda tree: tree["root"] == str(alternate)
                                and str(alternate / "foreign.txt") in paths(tree) and not tree["loading"])
        before_switch = state(app, first)
        app.request("layout.activate", layout=other_layout)
        click_row(app, second, alternate / "foreign.txt")
        foreign_preview = preview(app, alternate / "foreign.txt", layout=other_layout, pinned=False, content="INDEPENDENT_ROOT")
        captures.append(take_capture(app, "05-independent-root.png", different_from=captures[-1]))
        assert state(app, first)["root"] == str(root)
        app.request("layout.activate", layout=layout)
        # Confirmed external deletion drops the source in active and inactive
        # layouts: close among siblings, unload the area's sole viewer tab.
        renamed_b.unlink()
        wait_value(app, lambda value: not any(item["entity"] == b["entity"] for item in value["previews"]))
        (alternate / "foreign.txt").unlink()
        wait_value(app, lambda value: any(item["entity"] == foreign_preview["entity"]
                            and item["unloaded"] and item["path"] is None and item["content"] is None
                            for item in value["previews"]))
        app.request("layout.activate", layout=other_layout)
        captures.append(take_capture(app, "05b-deleted-source-unloaded.png", different_from=captures[-1]))
        app.request("layout.activate", layout=layout)

        if wide_entries:
            started = time.monotonic()
            app.request("app.set_filetree_root", panel=first, path=str(wide))
            wide_tree = wait_tree(app, first, lambda tree: tree["root"] == str(wide)
                                  and not tree["loading"] and len(tree["rows"]) == wide_entries + 1)
            assert len(wide_tree["row_rects"]) <= 80, "Wide root painted more than its viewport"
            assert wide_tree["cached_directories"] <= 8, wide_tree["cached_directories"]
            ready_seconds = time.monotonic() - started
            app.request("panel.focus", panel=first)
            key(app, "End")
            last = str(wide / f"entry-{wide_entries - 1:06}.txt")
            bottom = wait_tree(app, first, lambda tree: last in tree["selection"]
                              and any(row["path"] == last for row in tree["row_rects"]))
            assert len(bottom["row_rects"]) <= 80
            wide_observation = {"entries": wide_entries, "ready_seconds": ready_seconds,
                                "viewport_rows": len(bottom["row_rects"]),
                                "cached_directories": bottom["cached_directories"]}
            app.request("app.set_filetree_root", panel=first, path=str(root))
            wait_tree(app, first, lambda tree: tree["root"] == str(root)
                      and str(root / "a.txt") in paths(tree) and not tree["loading"])

        # Switch during managed work and verify final root/generation exclusively.
        for path in (slow, alternate, root):
            app.request("app.set_filetree_root", panel=first, path=str(path))
        switched = wait_tree(app, first, lambda tree: tree["root"] == str(root)
                            and not tree["loading"] and str(root / "a.txt") in paths(tree)
                            and tree["generation"] >= before_switch["generation"] + 3)
        for _ in range(12):
            tree = state(app, first)
            assert tree["root"] == str(root) and all(Path(row["path"]).is_relative_to(root)
                                                     for row in tree["rows"]), tree
            app.request("wait_frames", frames=2)
        assert str(renamed_external) not in paths(switched), switched
        assert state(app, second)["root"] == independent["root"]
        assert state(app, second)["generation"] == independent["generation"]
        captures.append(take_capture(app, "06-root-switch.png", different_from=captures[-1]))

        # Admitted preferences survive native Workbench workspace shutdown and all
        # tree/preview components retire. Transport exit is only final process cleanup.
        app.request("workspace.close", workspace=workspace)
        value = inspection(app)
        assert not any(tree["workspace"] == workspace for tree in value["panels"]), value
        assert not any(viewer["workspace"] == workspace for viewer in value["previews"]), value
        wait_value(app, lambda value: all(not record["pending"] for record in value["storage"]))
        saved_bytes = project_preferences.read_bytes()
        assert saved_bytes

    # A fresh process must load preferences, with no startup write or directory
    # creation. Restored root preferences hide the entry but reveal dotfiles.
    before_mtime = project_preferences.stat().st_mtime_ns
    with Application([str(binary), str(root)], output=output / "restored", timeout=60) as app:
        app.request("configure", fixed_dt=1 / 60, clipboard="")
        workspace, _, panel = open_native(app)
        wait_value(app, lambda value: storage(value, workspace) is not None and storage(value, workspace)["loaded"])
        restored = wait_tree(app, panel, lambda tree: tree["root"] == str(root)
                            and str(root / ".hidden.txt") in paths(tree)
                            and str(root / "watch-renamed.txt") not in paths(tree) and not tree["loading"])
        value = inspection(app)
        preferences = preference(value, workspace, root)
        assert preferences["hide_hidden"] is False and "watch-renamed.txt" in preferences["hidden_paths"], value
        assert storage(value, workspace)["error"] is None, value
        # Opening/focusing a new Files panel legitimately updates the workspace
        # arrangement. Visibility must remain intact, and an idle arrangement
        # must stop writing after its buffered mutation is acknowledged.
        wait_value(app, lambda value: not storage(value, workspace)["pending"])
        idle_bytes = project_preferences.read_bytes()
        idle_mtime = project_preferences.stat().st_mtime_ns
        app.request("wait_frames", frames=30)
        assert project_preferences.read_bytes() == idle_bytes
        assert project_preferences.stat().st_mtime_ns == idle_mtime, "Idle workspace kept rewriting storage"
        captures.append(take_capture(app, "07-restored-preferences.png"))
        app.request("workspace.close", workspace=workspace)
        assert not inspection(app)["panels"]
    return {"captures": captures, "capture_validation": not functional_only,
            "wide_root": wide_observation, "restored_root_rows": len(restored["rows"]),
            "external_drop_evidence": drop,
            "clipboard_backend": clipboard_before, "cut_paste_evidence": cut_evidence,
            "watcher_error": observed["watcher_error"],
            "checks": ["application View creation menu", "collapsible root and header removal", "read-only filetree and indent configuration", "keyboard expand/collapse", "lazy cache", "single/double/keyboard/context opens",
                       "layout-local preview reuse/pinning", "bounded text/binary reads", "external watcher create/rename",
                       "native inline rename/create", "human-readable error dialog with retained rename draft", "native internal drag move",
                       "injected Bevy external-drop events copy file/folder exactly once, root and enclosing-folder routing (not OS Finder drag)",
                       "context-menu duplicate/copy/paste", "moved preview path", "hidden preferences disk roundtrip",
                       "external deletion closes sibling tabs and unloads sole viewers, including inactive layouts", "independent panel roots", "stale root generations", "managed workspace cleanup"],
            "fixture_cleanup": "TemporaryDirectory removes every mutation fixture after process shutdown"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=os.environ.get("BED2_FILETREE_BINARY",
                        os.environ.get("BED2_BINARY", "target/debug/bed2")))
    parser.add_argument("--output", default="target/filetree-smoke")
    parser.add_argument("--functional-only", action="store_true",
                        help="Validate native interactions but not captured pixels; records this limit in summary.json")
    parser.add_argument("--wide-entries", type=int, default=0,
                        help="Also check viewport-only painting and End navigation on a temporary wide root")
    args = parser.parse_args()
    if not 0 <= args.wide_entries <= 200000:
        parser.error("--wide-entries must be between 0 and 200000")
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").unlink(missing_ok=True)
    source = Path(args.binary).resolve(strict=True)
    pinned = output / ("bed2-filetree-smoke.exe" if os.name == "nt" else "bed2-filetree-smoke")
    if source != pinned:
        shutil.copy2(source, pinned)
    with tempfile.TemporaryDirectory(prefix="bed2-filetree-smoke-") as temporary:
        summary = run(pinned, output, Path(temporary).resolve(), functional_only=args.functional_only,
                      wide_entries=args.wide_entries)
    summary["binary"] = str(pinned)
    summary["fixture_removed"] = not Path(temporary).exists()
    assert summary["fixture_removed"]
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    scope = "functional smoke passed (rendering NOT validated)" if args.functional_only else "smoke passed"
    print(f"Files {scope}: {len(summary['captures'])} captures in {output}")


if __name__ == "__main__":
    main()
