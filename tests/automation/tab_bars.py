"""A layout of one area with one tab is just that panel: no tab bar. A second
tab, or a second area, brings tab bars back. A press on a bar outside its tabs
focuses that area's panel."""
from bed import Bed


def tabs(bed):
    """Tab buttons (their bars sit just under the titlebar)."""
    return {w["label"]: w for w in bed.find_all(role="Button")
            if w["label"] in ("Workspaces", "Settings") and w["rect"][1] < 80}


with Bed(settings={"effects.enabled": True}) as bed:
    assert tabs(bed) == {}, "one tab in one area has no tab bar"
    assert bed.exists("Workspaces"), "the Workspaces panel is the only one"

    bed.invoke("settings.open")
    assert set(tabs(bed)) == {"Workspaces", "Settings"}

    # Drag Settings out to the right edge: two areas of one tab each.
    x0, y0, x1, y1 = tabs(bed)["Settings"]["rect"]
    window = bed.status()["window"]
    bed.drag(((x0 + x1) / 2, (y0 + y1) / 2), (window["width"] - 40, window["height"] / 2), steps=20)
    bed.wait(20)
    split = tabs(bed)
    assert set(split) == {"Workspaces", "Settings"}, "each area keeps its tab bar"
    assert split["Settings"]["rect"][0] > window["width"] / 3, "Settings moved to its own area"
    assert bed.exists("Close Settings")
    bed.screenshot("tab-bars-split")

    # Focus Settings (a press in its content), then press the left bar past
    # its tab: that focuses Workspaces, so Close closes it, not Settings.
    sx0, sy0, sx1, sy1 = split["Settings"]["rect"]
    bed.click((sx0 + 20, window["height"] - 60))
    bed.wait(5)
    x0, y0, x1, y1 = split["Workspaces"]["rect"]
    bed.click((x1 + 60, (y0 + y1) / 2))
    bed.wait(5)
    bed.key("Primary+W")
    bed.wait(20)
    assert "Workspaces" not in tabs(bed), "the bar's press focused its area"
    assert bed.exists(contains="Settings"), "Settings stayed open"
    bed.screenshot("tab-bars-focus")
