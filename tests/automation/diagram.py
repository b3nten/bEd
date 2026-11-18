"""Diagrams: a .diagram file opens on the canvas, fitted to the panel; Zoom
In, Zoom Out and Reset Zoom zoom the canvas; scrolling pans it; New Diagram
makes an untitled one. The tools (by key or button) draw shapes, which are
selected, moved and deleted; the Text tool writes text where it's clicked, and
a double click in a box writes its label; an arrow drawn from one shape's side
to another's is bound to both, pinned to those sides, and follows them; the style bar restyles what's selected; the pen draws a stroke by hand; what's selected is duplicated, copied and
pasted, grouped and lined up; Undo takes each step back; Save
writes the file."""
import math
import re
import shutil
import tempfile
from pathlib import Path
from bed import Bed

FIXTURE = Path(__file__).resolve().parents[1] / "fixtures" / "diagram" / "sample.diagram"


def until(bed, check, what, tries=300):
    for _ in range(tries):
        if check():
            return
        bed.wait(5)
    bed.screenshot("diagram-failed")
    raise AssertionError(f"never: {what}")


def open_file(bed, name, check, what):
    """Double-click the file tree's row until `check` holds: now and then a
    click doesn't land."""
    for _ in range(4):
        row = min(bed.find_all(name), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = row["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        for _ in range(60):
            if check():
                return
            bed.wait(5)
    bed.screenshot("diagram-failed")
    raise AssertionError(f"never: {what}")


def canvas(bed):
    """The canvas: what it shows of the diagram is its value ("16 elements at 100%")."""
    return bed.find("Diagram canvas")


def zoom(bed):
    """The canvas's zoom, in percent."""
    return int(canvas(bed)["value"].split(" at ")[1].split("%")[0])


def arrow(text):
    """The saved arrow's points, and whether it's bound to the first two elements."""
    line = next(line for line in text.splitlines() if "end: Arrow" in line)
    points = [(float(x), float(y)) for x, y in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", line)]
    return points, "from: Some(1), to: Some(2)" in line


def about(point, x, y):
    return abs(point[0] - x) < 0.01 and abs(point[1] - y) < 0.01


def shows(bed, *parts):
    """Whether the canvas's value has each part ("3 elements", "1 selected", "Select")."""
    value = canvas(bed)["value"]
    return all(part in value.replace(" at ", "; ").split("; ") for part in parts)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    shutil.copy(FIXTURE, root / "sample.diagram")
    (root / "broken.diagram").write_text("not a diagram\n")
    drawing = root / "drawing.diagram"
    drawing.write_text("")

    with Bed(folder, settings={"effects.enabled": True}) as bed:
        # All of them, so the rows are where they'll stay.
        until(
            bed,
            lambda: all(bed.exists(name) for name in ("broken.diagram", "drawing.diagram", "sample.diagram")),
            "the files listed",
        )
        bed.wait(5)
        open_file(
            bed,
            "sample.diagram",
            lambda: bed.exists("Diagram canvas") and canvas(bed)["value"].startswith("10 elements at "),
            "the fixture on the canvas",
        )
        # Fitted: all of it shows, no closer than actual size.
        fitted = zoom(bed)
        assert 50 <= fitted <= 100, fitted
        bed.screenshot("diagram")

        # The canvas has the focus, so zooming is its own.
        bed.click("Diagram canvas")
        bed.invoke("panel.zoom_in")
        until(bed, lambda: zoom(bed) > fitted, "zoomed in")
        bed.invoke("panel.zoom_out")
        bed.invoke("panel.zoom_out")
        until(bed, lambda: zoom(bed) < fitted, "zoomed out")
        bed.scroll(-120, at="Diagram canvas")
        bed.screenshot("diagram_zoomed_out")
        bed.invoke("panel.reset_zoom")
        until(bed, lambda: zoom(bed) == fitted, "fitted again")

        bed.invoke("diagram.new")
        until(bed, lambda: bed.exists("Untitled"), "an untitled diagram")
        until(bed, lambda: shows(bed, "0 elements", "100%"), "an empty canvas")
        bed.screenshot("diagram_new")

        # Drawing, in an empty file: the canvas's middle is its origin.
        open_file(
            bed,
            "drawing.diagram",
            lambda: bed.exists("Diagram canvas") and shows(bed, "0 elements"),
            "the empty drawing",
        )
        x0, y0, x1, y1 = canvas(bed)["rect"]
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        bed.click((cx, cy + 150))
        bed.key("r")
        until(bed, lambda: shows(bed, "Rectangle"), "the rectangle tool, by its key")
        bed.drag((cx - 150, cy - 50), (cx - 50, cy + 10))
        until(bed, lambda: shows(bed, "1 element", "1 selected", "Select"), "a rectangle, selected")
        bed.click("Ellipse")
        until(bed, lambda: shows(bed, "Ellipse"), "the ellipse tool, by its button")
        bed.drag((cx + 50, cy - 50), (cx + 150, cy + 10))
        until(bed, lambda: shows(bed, "2 elements"), "an ellipse")
        bed.key("a")
        bed.drag((cx - 50, cy - 20), (cx + 50, cy - 20))
        until(bed, lambda: shows(bed, "3 elements", "1 selected"), "an arrow between them")
        # Enter writes on what's selected: the arrow's label.
        bed.key("Enter")
        until(bed, lambda: shows(bed, "Writing"), "writing on the arrow")
        bed.type("go")
        bed.key("Primary+Enter")
        until(bed, lambda: shows(bed, "3 elements", "Select"), "the arrow's label kept")
        bed.invoke("document.save")
        until(bed, lambda: 'label: "go"' in drawing.read_text(), "the arrow saved")
        # Drawn from one shape's right side to the other's left, it's bound to
        # both and pinned to those sides: in the middle of each, a little off.
        points, bound = arrow(drawing.read_text())
        assert bound and "from_side: Right, to_side: Left" in drawing.read_text(), drawing.read_text()
        assert about(points[0], -45, -20) and about(points[1], 45, -20), points

        # An outline is picked up by its edge: the rectangle goes down 80.
        bed.drag((cx - 150, cy - 20), (cx - 150, cy + 60))
        until(bed, lambda: shows(bed, "1 selected"), "the rectangle selected")
        bed.screenshot("diagram_drawn")
        bed.invoke("document.save")
        until(bed, lambda: "Rectangle(x: -150.0, y: 30.0, w: 100.0, h: 60.0)" in drawing.read_text(), "the moved rectangle saved")
        saved = drawing.read_text()
        assert "Ellipse(x: 50.0, y: -50.0, w: 100.0, h: 60.0)" in saved, saved
        # The arrow went with the rectangle's right side, and still ends at
        # the ellipse's left.
        points, bound = arrow(saved)
        assert bound, saved
        assert about(points[0], -45, 60) and about(points[1], 45, -20), points

        # The rectangle is selected: the style bar's picks are its style.
        for pick in ("Fill Blue", "Bold stroke", "Dashed stroke", "Round corners"):
            bed.click(pick)
            bed.wait(2)
        bed.screenshot("diagram_styled")
        bed.invoke("document.save")
        until(
            bed,
            lambda: "fill: Some(Blue), width: 4.0, dash: Dashed, rounded: true" in drawing.read_text(),
            "the rectangle's style saved",
        )

        # Text, where the Text tool clicks; Cmd/Ctrl+Enter keeps it.
        bed.key("t")
        until(bed, lambda: shows(bed, "Text"), "the text tool")
        bed.click((cx - 100, cy + 150))
        until(bed, lambda: shows(bed, "Writing") and bed.exists("Diagram text"), "a field to write in")
        bed.type("Hello")
        bed.key("Primary+Enter")
        until(bed, lambda: shows(bed, "4 elements", "1 selected", "Select"), "the text, selected")

        # A label, by a double click inside the rectangle; Escape keeps it too.
        inside = {"move": {"x": cx - 100, "y": cy + 60}}
        bed.input(inside, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: shows(bed, "Writing"), "writing in the box")
        bed.type("Box")
        bed.screenshot("diagram_writing")
        bed.key("Escape")
        until(bed, lambda: shows(bed, "4 elements", "Select"), "the label kept")
        bed.screenshot("diagram_text")
        bed.invoke("document.save")
        until(bed, lambda: 'label: "Box"' in drawing.read_text(), "the label saved")
        assert 'text: "Hello"' in drawing.read_text(), drawing.read_text()

        # The pen draws a stroke through where the pointer goes, and stays in
        # hand; Undo takes the stroke back.
        bed.key("p")
        until(bed, lambda: shows(bed, "Pen"), "the pen")
        wave = [(cx + 150 + 6 * i, cy + 150 + 30 * math.sin(i / 3)) for i in range(25)]
        bed.input(
            {"move": {"x": wave[0][0], "y": wave[0][1]}},
            {"press": "Left"},
            *({"move": {"x": x, "y": y}} for x, y in wave[1:]),
            {"release": "Left"},
        )
        until(bed, lambda: shows(bed, "5 elements", "0 selected", "Pen"), "a stroke, the pen still in hand")
        bed.screenshot("diagram_pen")
        bed.invoke("document.save")
        until(bed, lambda: "Freehand(points: [" in drawing.read_text(), "the stroke saved")
        stroke = next(line for line in drawing.read_text().splitlines() if "Freehand" in line)
        kept = len(re.findall(r"\(-?[\d.]+, -?[\d.]+\)", stroke))
        assert 4 <= kept < 25, f"{kept} points kept of 25: {stroke}"
        bed.invoke("document.undo")
        until(bed, lambda: shows(bed, "4 elements"), "the stroke undone")
        bed.key("v")

        # The rectangle, duplicated, then copied and pasted; everything grouped
        # is selected by a click on any of it, and lined up from the style bar.
        bed.click((cx - 100, cy + 60))
        until(bed, lambda: shows(bed, "1 selected"), "the rectangle selected")
        bed.key("Primary+d")
        until(bed, lambda: shows(bed, "5 elements", "1 selected"), "a duplicate, selected")
        bed.key("Primary+c")
        bed.key("Primary+v")
        until(bed, lambda: shows(bed, "6 elements", "1 selected"), "a pasted copy, selected")
        bed.key("Primary+a")
        until(bed, lambda: shows(bed, "6 selected"), "everything selected")
        assert bed.exists("Send to back") and bed.exists("Bring to front")
        bed.click("Align left")
        bed.wait(2)
        bed.screenshot("diagram_arranged")
        bed.key("Primary+g")
        bed.key("Escape")
        until(bed, lambda: shows(bed, "0 selected"), "nothing selected")
        # Grouped, they're one thing: a click on any selects all, and there's
        # nothing to line up.
        bed.click((cx - 100, cy + 150))
        until(bed, lambda: shows(bed, "6 selected"), "the whole group, by a click on the text")
        assert not bed.exists("Align left")
        bed.invoke("document.save")
        until(bed, lambda: drawing.read_text().count("group: Some(1)") == 6, "the group saved")
        lefts = set(re.findall(r"(?:Rectangle|Ellipse)\(x: (-?[\d.]+)", drawing.read_text()))
        assert lefts == {"-150.0"}, f"not lined up on the left: {lefts}"
        # Back to the four: the group, the alignment, the paste and the duplicate.
        for _ in range(4):
            bed.invoke("document.undo")
        until(bed, lambda: shows(bed, "4 elements"), "the arranging undone")
        bed.key("Escape")

        # Everything, deleted; Undo brings it back, then takes back the label,
        # the text and the move.
        bed.click((cx + 200, cy + 150))
        bed.key("Primary+a")
        until(bed, lambda: shows(bed, "4 selected"), "everything selected")
        bed.key("Delete")
        until(bed, lambda: shows(bed, "0 elements", "0 selected"), "everything deleted")
        bed.invoke("document.undo")
        until(bed, lambda: shows(bed, "4 elements"), "the delete undone")
        bed.invoke("document.undo")
        bed.invoke("document.undo")
        until(bed, lambda: shows(bed, "3 elements"), "the label and the text undone")
        # The four styles, and then the move.
        for _ in range(4):
            bed.invoke("document.undo")
        bed.invoke("document.save")
        until(bed, lambda: "fill" not in drawing.read_text(), "the styles undone")
        bed.invoke("document.undo")
        bed.invoke("document.save")
        until(bed, lambda: "Rectangle(x: -150.0, y: -50.0, w: 100.0, h: 60.0)" in drawing.read_text(), "the move undone")
        points, bound = arrow(drawing.read_text())
        assert bound and about(points[0], -45, -20) and about(points[1], 45, -20), points
        bed.key("Escape")

        # A file that isn't a diagram says so, rather than showing a canvas.
        open_file(
            bed,
            "broken.diagram",
            lambda: bed.exists(contains="can't be opened"),
            "why it can't be opened",
        )
