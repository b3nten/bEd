"""Settings: opens with its shortcut, rows line up, and choosing an installed
font applies it."""
from bed import Bed


def centre(widget):
    return (widget["rect"][1] + widget["rect"][3]) / 2


with Bed() as bed:
    bed.key("Primary+Comma")
    assert bed.exists("Appearance"), "Settings didn't open"
    # Installed fonts are found in the background.
    for _ in range(100):
        if not bed.exists("Loading installed fonts…"):
            break
        bed.wait(5)
    bed.screenshot("settings")

    # Each label is centred on its control.
    for label, control in [("Theme", "Everforest Dark Medium"), ("Font", "Bedfont"), ("Font size", "15.5 pt")]:
        offset = centre(bed.find(label)) - centre(bed.find(control))
        assert abs(offset) < 1, f"{label} is {offset:.1f}pt off its control"

    bed.click("Bedfont")
    bed.click("Menlo")
    bed.wait(5)
    assert bed.exists("Menlo"), "Menlo wasn't chosen"
    assert bed.exists("rotate-ccw"), "the font row has no reset button"
    bed.screenshot("settings-menlo")

    # The Font row's own reset button: other rows may have one too (a
    # setting the script was started with).
    font = centre(bed.find("Font"))
    on_font_row = lambda: [w for w in bed.find_all("rotate-ccw") if abs(centre(w) - font) < 8]
    bed.click(on_font_row()[0])
    assert bed.exists("Bedfont") and not on_font_row(), "reset didn't clear the font"

    # Toggles can be switched on: one that's off (some default to on).
    off = [w for w in bed.find_all(role="CheckBox") if w["toggled"] == "False"]
    assert off, "no toggle is off"
    toggle = off[0]
    bed.click(toggle)
    now = [w for w in bed.find_all(role="CheckBox") if w["rect"] == toggle["rect"]]
    assert now and now[0]["toggled"] == "True", "the toggle didn't switch on"
    assert bed.exists("rotate-ccw"), "switching the toggle didn't set the setting"
