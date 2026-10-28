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
    assert bed.exists("Overridden"), "the row shows the override marker"

    bed.click(contains="Workspace:")
    assert opacity(bed) == workspace_value, "the workspace keeps its own"
    assert not bed.exists("Overridden")

    # Set both to the same value: the workspace stops holding its own, so it
    # follows the global value again and nothing is marked overridden.
    bed.click("Global")
    drag_opacity(bed, 1.0)
    bed.click(contains="Workspace:")
    drag_opacity(bed, 1.0)
    assert opacity(bed) == 1.0
    bed.click("Global")
    assert not bed.exists("Overridden"), "the same value overrides nothing"
    bed.click(contains="Workspace:")
    assert not bed.exists("rotate-ccw"), "the workspace holds no value of its own"
