"""Mascots: View's Bed opens a Bed panel; `+ducky` in a terminal opens Ducky.
Both come back with the workspace. (They aren't clicked: that plays sound.)"""
import tempfile
from bed import Bed


def until(bed, check, what):
    for _ in range(300):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def tab(bed, name):
    return any((w.get("label") or "") == f"Close {name}" for w in bed.widgets())


with tempfile.TemporaryDirectory() as folder:
    with Bed(folder) as bed:
        bed.invoke("mascots.show_bed")
        until(bed, lambda: tab(bed, "Bed"), "Bed opens")
        bed.wait(30)
        bed.screenshot("mascots-bed")

        bed.invoke("terminal.new")
        until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "a terminal opens")
        bed.wait(60)
        bed.type("+ducky")
        bed.key("Enter")
        until(bed, lambda: tab(bed, "Ducky"), "Ducky opens")
        bed.wait(30)
        bed.screenshot("mascots-ducky")
        assert not bed.status()["errors"], bed.status()["errors"]

        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.close_folder"}))
        bed.switched(lambda: bed.call("bedtime.invoke", {"command": "app.open_recent"}))
        until(bed, lambda: tab(bed, "Bed") and tab(bed, "Ducky"), "both come back")
