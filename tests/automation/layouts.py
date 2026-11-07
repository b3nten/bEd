"""Five numbered layouts per workspace; a saved default is shared by every
workspace and builds slots that haven't been opened yet."""
import tempfile
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    bed.invoke("settings.open")
    assert bed.exists("Appearance")
    bed.key("Primary+2")
    assert not bed.exists("Appearance"), "layout 2 should start empty"
    bed.key("Primary+1")
    assert bed.exists("Appearance"), "layout 1 kept its panel"

    bed.invoke("layout.save_default")
    # Switching saves every store, as quitting does.
    # The default workspace hasn't opened layout 1 yet: it's built from the default.
    bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
    assert bed.exists("Appearance"), "the default didn't build layout 1"
    bed.key("Primary+3")
    assert not bed.exists("Appearance")
    bed.screenshot("layouts")
    # Layout defaults are their own file in the global store.
    assert "Settings" in (bed.config / "global" / "layout_defaults.ron").read_text()
    assert not bed.status()["errors"]
