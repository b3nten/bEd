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
    for label, control in [("Theme", "Dark"), ("Font", "Bedfont"), ("Font size", "15.5 pt")]:
        offset = centre(bed.find(label)) - centre(bed.find(control))
        assert abs(offset) < 1, f"{label} is {offset:.1f}pt off its control"

    bed.click("Bedfont")
    bed.click("Menlo")
    bed.wait(5)
    assert bed.exists("Menlo"), "Menlo wasn't chosen"
    assert bed.exists("rotate-ccw"), "the font row has no reset button"
    bed.screenshot("settings-menlo")

    bed.click("rotate-ccw")
    assert bed.exists("Bedfont") and not bed.exists("rotate-ccw"), "reset didn't clear the font"

    # Toggles (Window blur on macOS) can be switched on.
    toggle = bed.find(role="CheckBox")
    assert toggle["toggled"] == "False"
    bed.click(toggle)
    assert bed.find(role="CheckBox")["toggled"] == "True", "the toggle didn't switch on"
    assert bed.exists("rotate-ccw"), "switching the toggle didn't set the setting"
