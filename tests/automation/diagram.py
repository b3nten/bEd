"""Diagrams: a .diagram file opens on the canvas, fitted to the panel; Zoom
In, Zoom Out and Reset Zoom zoom the canvas; scrolling pans it; New Diagram
makes an untitled one. The tools (by key or button) draw shapes, which are
selected, moved and deleted; the Text tool writes text where it's clicked, and
a double click in a box writes its label; an arrow drawn from one shape's side
to another's is bound to both, pinned to those sides, and follows them; the
style bar's buttons open the styles to pick for what's selected; a line takes
heads, and bends where its middle is dragged; the pen draws a stroke by hand; what's selected is duplicated, copied and
pasted, grouped and lined up; a double click goes into a group; the other
button's menu ungroups; Undo takes each step back; Save
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
    """The saved arrow's bends (its bound ends aren't saved: they're worked
    out), and whether it's bound to the rectangle and the ellipse."""
    line = next(line for line in text.splitlines() if "shape: Arrow(" in line)
    via = re.search(r"via: \[(.*?)\]", line)
    points = [(float(x), float(y)) for x, y in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", via.group(1))] if via else []
    return points, 'from: "rect-1", to: "ellipse-1"' in line


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
        assert points == [], points

        # An outline is picked up by its edge: the rectangle goes down 80.
        bed.drag((cx - 150, cy - 20), (cx - 150, cy + 60))
        until(bed, lambda: shows(bed, "1 selected"), "the rectangle selected")
        bed.screenshot("diagram_drawn")
        bed.invoke("document.save")
        until(bed, lambda: "Rectangle(x: -150, y: 30, w: 100, h: 60)" in drawing.read_text(), "the moved rectangle saved")
        saved = drawing.read_text()
        assert "Ellipse(x: 50, y: -50, w: 100, h: 60)" in saved, saved
        # The arrow went with the rectangle (where it meets it is worked out,
        # not saved), still bound to both.
        points, bound = arrow(saved)
        assert bound and points == [], saved

        # The rectangle is selected: the style bar's picks are its style.
        # A button of the bar opens its choices; the corners' turns them round.
        for opener, pick in (("Fill", "Fill Blue"), ("Stroke width", "Bold stroke"), ("Dashes", "Dashed stroke")):
            bed.click(opener)
            until(bed, lambda: bed.exists(pick), f"the choices of {opener}")
            if opener == "Fill":
                bed.screenshot("diagram_style_choices")
            bed.click(pick)
            until(bed, lambda: not bed.exists(pick), f"the choices of {opener} closed")
        assert not bed.exists("Arrow at end"), "a rectangle has no heads"
        bed.click("Round corners")
        bed.wait(2)
        bed.screenshot("diagram_styled")
        bed.invoke("document.save")
        until(
            bed,
            lambda: "fill: Blue, width: 4, dash: Dashed, rounded: true" in drawing.read_text(),
            "the rectangle's style saved",
        )

        # The arrow: a head for its start from the
        # bar; its middle, dragged, is a point it bends at; twice on that
        # point straightens it again.
        bed.click((cx - 10, cy + 29))
        until(bed, lambda: shows(bed, "1 selected") and bed.exists("Arrow at start"), "the arrow selected")
        bed.click("Arrow at start")
        # (Its label is over its middle: the grip is a quarter along.)
        bed.drag((cx - 22.5, cy + 40), (cx, cy + 90))
        bed.screenshot("diagram_bent")
        bed.invoke("document.save")
        until(bed, lambda: "start: Arrow" in drawing.read_text(), "both heads saved")
        points, bound = arrow(drawing.read_text())
        assert bound and len(points) == 1 and about(points[0], 0, 90), points
        at = {"move": {"x": cx, "y": cy + 90}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        bed.invoke("document.save")
        until(bed, lambda: len(arrow(drawing.read_text())[0]) == 0, "the bend taken out")
        for _ in range(3):
            bed.invoke("document.undo")
        bed.invoke("document.save")
        until(bed, lambda: "start: Arrow" not in drawing.read_text(), "the arrow as it was")
        bed.key("Escape")

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
        # Grouped, they're one thing: a click on any selects all (and the
        # group, an element itself), and there's nothing to line up.
        bed.click((cx - 100, cy + 150))
        until(bed, lambda: shows(bed, "7 elements", "7 selected"), "the whole group, by a click on the text")
        assert not bed.exists("Align left")
        bed.invoke("document.save")
        until(bed, lambda: 'shape: Group, children: [' in drawing.read_text(), "the group saved")
        assert drawing.read_text().count("\n            (id: ") == 6, drawing.read_text()
        lefts = set(re.findall(r"(?:Rectangle|Ellipse)\(x: (-?[\d.]+)", drawing.read_text()))
        assert lefts == {"-150"}, f"not lined up on the left: {lefts}"
        # Twice on one of a group goes into it: that one alone, to move on its
        # own. Escape leaves the group (or first, if a third click came,
        # the writing), then lets go.
        at = {"move": {"x": cx - 100, "y": cy + 60}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: shows(bed, "1 selected"), "one of the group, by a double click")
        bed.screenshot("diagram_within_group")
        for _ in range(3):
            if not shows(bed, "0 selected"):
                bed.key("Escape")
                bed.wait(2)
        until(bed, lambda: shows(bed, "0 selected", "Select"), "out of the group")
        # The other button's menu is for what it's on: the group, to ungroup.
        bed.click((cx - 100, cy + 60), button="Right")
        menu = lambda name: bed.find_all(contains=name, role="Button")
        until(bed, lambda: shows(bed, "7 selected") and menu("Duplicate"), "the group's menu")
        assert menu("Ungroup") and menu("Order") and menu("Export Selection")
        bed.screenshot("diagram_menu")
        bed.click(menu("Ungroup")[0])
        until(bed, lambda: not menu("Duplicate"), "the menu closed")
        bed.invoke("document.save")
        until(bed, lambda: "Group" not in drawing.read_text(), "the group taken apart")
        bed.key("Escape")
        # On nothing, it's the canvas's menu.
        bed.click((cx + 250, cy - 150), button="Right")
        until(bed, lambda: menu("Select All") and menu("Zoom to Fit"), "the canvas's menu")
        assert not menu("Ungroup")
        bed.screenshot("diagram_canvas_menu")
        bed.click(menu("Select All")[0])
        until(bed, lambda: shows(bed, "6 selected"), "everything selected from the menu")
        bed.invoke("document.undo")
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
        until(bed, lambda: "Rectangle(x: -150, y: -50, w: 100, h: 60)" in drawing.read_text(), "the move undone")
        points, bound = arrow(drawing.read_text())
        assert bound and points == [], points
        bed.key("Escape")

        # A file that isn't a diagram says so, rather than showing a canvas.
        open_file(
            bed,
            "broken.diagram",
            lambda: bed.exists(contains="can't be opened"),
            "why it can't be opened",
        )
