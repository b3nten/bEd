"""The cat: it sits on the window's bottom edge, plays with the pointer once
clicked (bats at it in reach, runs after it far off), and its setting (kitty!)
sends it away and brings it back."""
import os
from bed import Bed

# Start it sitting at a known place, and have it say what it does.
os.environ["BED_CAT_ACT"] = "sit"
os.environ["BED_CAT_LOG"] = "1"


def until(bed, check, what):
    for _ in range(300):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def acts(bed):
    """What the cat has done, in order."""
    lines = open(bed.log_path).read().splitlines()
    return [line.split("cat: ")[1].split()[0] for line in lines if "cat: " in line]


def toggle_beside(bed, label):
    """The toggle on the same row as `label`."""
    y = sum(bed.find(label)["rect"][1::2]) / 2
    toggles = bed.find_all(role="CheckBox")
    return min(toggles, key=lambda t: abs(sum(t["rect"][1::2]) / 2 - y))


with Bed(settings={"appearance.cat": True}) as bed:
    until(bed, lambda: bed.exists("The cat"), "the cat shows")
    bed.wait(30)
    x0, y0, x1, y1 = bed.find("The cat")["rect"]
    height = bed.status()["window"]["height"]
    assert abs(y1 - height) < 2, f"it stands on the window's bottom edge: {y1} of {height}"
    assert 40 < y1 - y0 < 160 and 30 < x1 - x0 < 220, "it is cat-sized"
    bed.screenshot("cat-sitting")

    bed.click(bed.find("The cat"))
    bed.move(x1 + 28, y0 - 4)
    until(bed, lambda: "Bat" in acts(bed), "it bats at a pointer in reach")
    bed.wait(12)
    bed.screenshot("cat-batting")
    bed.move(x1 + 420, y0)
    until(bed, lambda: "Walk" in acts(bed), "it runs after a pointer far off")
    until(bed, lambda: bed.find("The cat")["rect"][0] > x0 + 150, "it gets there")
    bed.screenshot("cat-chasing")

    # The setting sends it away, and brings it back.
    bed.key("Primary+2")
    bed.invoke("settings.open")
    for _ in range(30):
        if bed.exists("kitty!"):
            break
        bed.scroll(-150, at={"rect": [480, 320, 480, 320]})
    bed.click(toggle_beside(bed, "kitty!"))
    until(bed, lambda: not bed.exists("The cat"), "the setting sends it away")
    bed.click(toggle_beside(bed, "kitty!"))
    until(bed, lambda: bed.exists("The cat"), "the setting brings it back")
    bed.wait(20)
    bed.screenshot("cat-back")
    assert not bed.status()["errors"], bed.status()["errors"]
    log = open(bed.log_path).read()
    assert "ERROR" not in log and "panicked" not in log, log[-2000:]

with Bed() as bed:
    bed.wait(30)
    assert not bed.exists("The cat"), "scripts run without it unless they ask"
