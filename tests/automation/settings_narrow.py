"""In a narrow panel Settings scrolls sideways instead of clipping, and a
setting the workspace overrides shows a warning icon by its label."""
import tempfile
from bed import Bed

with tempfile.TemporaryDirectory() as folder, Bed(folder) as bed:
    bed.key("Primary+2")
    bed.invoke("settings.open")
    # Override window opacity in the workspace, then look at the global tab.
    bed.click(contains="Workspace:")
    x0, y0, x1, y1 = bed.find_all(role="Slider")[1]["rect"]
    bed.drag((x1 - 4, (y0 + y1) / 2), (x0 + (x1 - x0) * 0.6, (y0 + y1) / 2))
    bed.click("Global")
    marker = bed.find("Overridden")
    opacity = [w for w in bed.find_all("Opacity") if w["role"] != "Button"][0]
    assert abs(marker["rect"][1] - opacity["rect"][1]) < 8, "the marker sits by its label"
    assert marker["rect"][0] > opacity["rect"][2], "after the label"

    # Split Settings and Workspaces, then narrow the Settings area.
    bed.invoke("app.open_workspaces")
    tab = [w for w in bed.find_all("Workspaces", role="Button") if w["rect"][1] < 80][0]
    window = bed.status()["window"]
    centre = lambda r: ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2)
    bed.drag(centre(tab["rect"]), (window["width"] - 30, window["height"] / 2), steps=20)
    divider = window["width"] / 2
    bed.drag((divider, window["height"] / 2), (window["width"] * 0.29, window["height"] / 2), steps=15)
    bed.wait(10)

    before = bed.find("Theme")["rect"][0]
    bed.input({"move": {"x": window["width"] * 0.15, "y": window["height"] / 2}},
              {"scroll": {"x": -400, "y": 0}})
    bed.wait(10)
    after = bed.find_all("Theme", visible=False)[0]["rect"][0]
    assert after < before - 50, f"didn't scroll sideways: {before} -> {after}"
    bed.screenshot("settings-narrow")
