"""A layout of one area with one tab is just that panel: no tab bar. A second
tab, or a second area, brings tab bars back."""
from bed import Bed


def tabs(bed):
    """Tab buttons (their bars sit just under the titlebar)."""
    return {w["label"]: w for w in bed.find_all(role="Button")
            if w["label"] in ("Workspaces", "Settings") and w["rect"][1] < 80}


with Bed() as bed:
    assert tabs(bed) == {}, "one tab in one area has no tab bar"
    assert bed.exists("Workspaces"), "the Workspaces panel is the only one"

    bed.invoke("app.open_settings")
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
