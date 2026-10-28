"""The default workspace opens on the Workspaces panel: folder workspaces only,
to open in this window or delete."""
import tempfile
from pathlib import Path
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    name = Path(folder).resolve().name
    bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
    # Built-in layout 1 of the default workspace, which isn't listed itself.
    assert bed.exists(name) and not bed.exists("Default workspace")
    bed.screenshot("workspaces-panel")

    # The default workspace has no settings layer of its own.
    bed.invoke("app.open_settings")
    assert not bed.exists(contains="Workspace:")

    stores = bed.config / "workspaces"
    folder_stores = lambda: [p.name for p in stores.iterdir() if p.name != "default"]
    assert len(folder_stores()) == 1
    bed.key("Primary+1")
    bed.invoke("app.open_workspaces")
    bed.click("Delete")
    bed.screenshot("workspaces-panel-confirm")
    bed.click("Delete saved layout and settings")
    bed.wait(5)
    assert folder_stores() == [], folder_stores()
    assert Path(folder).is_dir(), "the folder itself must stay"
    assert name not in (bed.config / "global" / "recent.ron").read_text()
    assert not bed.exists(name)
    assert not bed.status()["errors"]
