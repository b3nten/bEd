"""The default workspace opens on the Workspaces panel: folder workspaces only,
one row each; clicking a row opens it in this window, its trash button deletes
the workspace's saved data after asking."""
import tempfile
from pathlib import Path
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    name = Path(folder).resolve().name
    bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
    # Built-in layout 1 of the default workspace, which isn't listed itself
    # (its name is only in the status bar).
    assert bed.exists(name) and len(bed.find_all("Default workspace")) == 1
    bed.screenshot("workspaces-panel")

    # The default workspace has no settings layer of its own.
    bed.invoke("settings.open")
    assert not bed.exists(contains="Workspace:")

    stores = bed.config / "workspaces"
    folder_stores = lambda: [p.name for p in stores.iterdir() if p.name != "default"]
    assert len(folder_stores()) == 1
    bed.key("Primary+1")
    bed.invoke("app.open_workspaces")
    bed.click(f"Delete {name}")
    assert bed.exists("Delete its saved layout and settings?")
    bed.screenshot("workspaces-panel-confirm")
    bed.click("Delete")
    bed.wait(5)
    assert folder_stores() == [], folder_stores()
    assert Path(folder).is_dir(), "the folder itself must stay"
    # The recent list is saved with the rest of the app, about once a second.
    recent = bed.config / "global" / "recent.ron"
    for _ in range(100):
        if name not in recent.read_text():
            break
        bed.wait(5)
    assert name not in recent.read_text()
    assert not bed.exists(name)
    assert not bed.status()["errors"]
