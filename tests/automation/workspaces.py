"""Workspaces switch in place: Close Folder goes to the default workspace and
Open Recent comes back, each rebuilding the App in the same window."""
import tempfile
from pathlib import Path
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    name = Path(folder).resolve().name
    bed.invoke("app.open_settings")
    window = bed.status()["window"]
    assert bed.exists(f"Workspace: {name}")

    took = bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
    assert took < 1, f"switching took {took:.2f}s"
    assert bed.status()["window"] == window, "the window changed size"
    # The default workspace has its own layout: Settings isn't open there.
    assert not bed.exists("Appearance")
    bed.invoke("app.open_settings")
    assert bed.exists("Default workspace")
    stores = sorted(p.name for p in (bed.config / "workspaces").iterdir())
    assert "default" in stores and len(stores) == 2, stores

    # Back to the folder, whose layout (Settings open) was saved.
    bed.relaunched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
    assert bed.exists(f"Workspace: {name}")
    assert not bed.status()["errors"]
    bed.screenshot("workspaces")
