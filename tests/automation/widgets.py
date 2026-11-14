"""`+widgets` from a terminal opens the widget showcase beside it (its area
is split, with no other); running it again opens another, in that other
area. The filter narrows the cards, and the dialog card opens a modal."""
import time
from bed import Bed


def until(bed, check, what, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def widget_tabs(bed):
    return [w for w in bed.widgets() if w.get("label") == "Widgets" and w["role"] != "Label"]


# Effects keep the window drawing while it is in the background.
with Bed(settings={"effects.enabled": True}) as bed:
    bed.invoke("terminal.new")
    until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "a terminal opens")
    bed.wait(30)
    bed.type("+widgets")
    bed.key("Enter")
    until(bed, lambda: bed.exists(contains="Everything bed can draw"), "the showcase opens")
    bed.wait(20)
    bed.screenshot("widgets")
    page = bed.find(contains="Everything bed can draw")
    for index in range(1, 6):
        bed.scroll(-900, at=page)
        bed.wait(20)
        bed.screenshot(f"widgets-{index}")
    tabs = len(widget_tabs(bed))

    # Again, from the same terminal: a second panel, with the first.
    terminal = next(w for w in bed.widgets() if w.get("label") in ("zsh", "bash", "sh", "Terminal"))
    bed.click(terminal)
    bed.wait(10)
    bed.type("+widgets")
    bed.key("Enter")
    until(bed, lambda: len(widget_tabs(bed)) == tabs + 1, "a second +widgets opens another panel")
    page = bed.find(contains="Everything bed can draw")

    # The filter narrows the page to matching cards.
    for _ in range(6):
        bed.scroll(900, at=page)
    bed.wait(20)
    terminal_input = next(w for w in bed.widgets() if w["role"] == "TextInput")
    fields = [w for w in bed.find_all(role="TextInput") if w["rect"][0] > terminal_input["rect"][0]]
    bed.click(min(fields, key=lambda w: w["rect"][1]))
    bed.type("dialog")
    until(bed, lambda: not bed.exists("Make something", visible=False) and bed.exists("Open dialog"), "the filter leaves only the dialog card")
    bed.click("Open dialog")
    until(bed, lambda: bed.exists("Discard changes?"), "the dialog opens")
    bed.screenshot("widgets-dialog")
    bed.click("Cancel")
    until(bed, lambda: not bed.exists("Discard changes?"), "the dialog closes")

    assert not bed.status()["errors"], bed.status()["errors"]
