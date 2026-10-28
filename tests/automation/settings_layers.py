"""In a folder workspace, the Global tab edits the global value even when the
workspace sets its own, and says so."""
import tempfile
from bed import Bed


def opacity(bed):
    return bed.find_all(role="Slider")[1]["numeric_value"]


def drag_opacity(bed, fraction):
    x0, y0, x1, y1 = bed.find_all(role="Slider")[1]["rect"]
    y = (y0 + y1) / 2
    start = x0 + 4 if opacity(bed) < 0.5 else x1 - 4
    bed.drag((start, y), (x0 + (x1 - x0) * fraction, y))


with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    bed.key("Primary+2")
    bed.invoke("app.open_settings")
    bed.click(contains="Workspace:")
    drag_opacity(bed, 0.6)
    workspace_value = opacity(bed)
    assert 0.5 < workspace_value < 0.7, workspace_value

    bed.click("Global")
    assert opacity(bed) == 1.0, "the global tab shows the global value"
    drag_opacity(bed, 0.2)
    assert opacity(bed) < 0.3, "the global value can be changed"
    assert bed.exists("Set differently in this workspace")

    bed.click(contains="Workspace:")
    assert opacity(bed) == workspace_value, "the workspace keeps its own"
    assert not bed.exists("Set differently in this workspace")
