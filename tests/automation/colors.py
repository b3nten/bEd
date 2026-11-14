"""Colors in text: each is underlined in its color; resting on one opens a
picker, and dragging in it rewrites the color as it was written (hex stays
hex). Moving away closes it, and the whole drag is one undo step."""
import tempfile
import time
from pathlib import Path
from bed import Bed

TEXT = "a { color: #ff0000; }\nb { background: rgb(0, 128, 255); }\n"


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def value(bed):
    return next(w for w in bed.widgets() if w["role"] == "TextInput").get("value") or ""


def picker(bed):
    return [w for w in bed.widgets() if w["role"] == "SpinButton"]


with tempfile.TemporaryDirectory() as folder:
    css = Path(folder) / "style.css"
    css.write_text(TEXT)
    with Bed(folder, settings={"editor.autosave": False}) as bed:
        until(bed, lambda: bed.exists("style.css"), "the root is listed")
        x0, y0, x1, y1 = min(bed.find_all("style.css"), key=lambda w: w["rect"][0])["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: "#ff0000" in value(bed), "style.css opens")
        bed.wait(10)
        bed.screenshot("colors")

        # Rest on "#ff0000" (after "a { color: ", eleven characters).
        ex, ey = next(w for w in bed.widgets() if w["role"] == "TextInput")["rect"][:2]
        spot = (ex + 8 + 14.5 * 9.3, ey + 6 + 10)
        bed.move(*spot)
        bed.wait(2)
        bed.move(spot[0] + 1, spot[1])
        time.sleep(0.5)
        until(bed, lambda: picker(bed), "the picker opens")
        assert [w["value"] for w in picker(bed)] == ["R 255", "G 0", "B 0"], picker(bed)
        bed.screenshot("colors-picker")

        # Drag across the color square: the text follows, still hex.
        numbers_bottom = max(w["rect"][3] for w in picker(bed))
        square = next(w for w in bed.widgets() if w["role"] == "Unknown"
                      and w["rect"][1] > numbers_bottom and w["rect"][2] - w["rect"][0] > 50
                      and abs((w["rect"][2] - w["rect"][0]) - (w["rect"][3] - w["rect"][1])) < 2)
        sx0, sy0, sx1, sy1 = square["rect"]
        start = {"move": {"x": sx1 - 4, "y": sy0 + 4}}
        steps = [{"move": {"x": sx1 - 4 - i * 8, "y": sy0 + 4 + i * 8}} for i in range(1, 8)]
        bed.input(start, {"press": "Left"}, *steps, {"release": "Left"})
        until(bed, lambda: "#ff0000" not in value(bed), "the color changes")
        line = value(bed).splitlines()[0]
        assert line.startswith("a { color: #") and line.endswith("; }") and len(line) == 21, line
        bed.screenshot("colors-dragged")

        # Away: it closes. One undo puts the color back.
        bed.move(ex + 600, ey + 480)
        until(bed, lambda: not picker(bed), "the picker closes")
        bed.key("Primary+Z")
        until(bed, lambda: "#ff0000" in value(bed), "one undo restores it")
        assert not bed.status()["errors"], bed.status()["errors"]
