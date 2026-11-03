"""Screen effects: the toggle in Settings turns them on, showing their sliders,
and they render without errors. (Curvature stays off: it bends real pointer
input, which automation's positions, taken from the undistorted layout, would
then miss.)"""
from bed import Bed


def toggle_beside(bed, label):
    """The toggle on the same row as `label`."""
    y = sum(bed.find(label)["rect"][1::2]) / 2
    toggles = bed.find_all(role="CheckBox")
    return min(toggles, key=lambda t: abs(sum(t["rect"][1::2]) / 2 - y))


with Bed() as bed:
    bed.key("Primary+2")
    bed.invoke("app.open_settings")
    for _ in range(20):
        if bed.exists("Screen effects"):
            break
        # The middle of the settings page (the window's own panel).
        bed.scroll(-150, at={"rect": [480, 320, 480, 320]})
    assert bed.exists("Screen effects") and not bed.exists("Scanlines")

    bed.click(toggle_beside(bed, "Screen effects"))
    bed.wait(30)
    assert bed.exists("Scanlines"), "the sliders show once on"
    bed.screenshot("effects-on")
    # Showing a slider mustn't change its setting (egui rounds what it shows).
    assert len(bed.find_all("rotate-ccw")) == 1, "only the toggle was set"
    saved = (bed.config / "global" / "settings.ron")
    bed.wait(80)
    assert "bloom" not in saved.read_text(), saved.read_text()
    assert not bed.status()["errors"]

    bed.click(toggle_beside(bed, "Screen effects"))
    assert not bed.exists("Scanlines"), "the sliders hide again"
    log = open(bed.log_path).read()
    assert "ERROR" not in log and "panicked" not in log, log[-2000:]
