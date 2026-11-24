"""The menus in the window (the "Menu in window" setting: always on where the
platform has no menu bar of its own, off by default on macOS). Folded into
one button ("Compact menu", on by default), the whole bar opens from bEd's
icon in the titlebar, and its menus still lead to their items. Not compact,
the menu bar is drawn instead."""
import sys

from bed import Bed

MENUS = ("bEd", "File", "Edit", "View")
SHOWN = {"appearance.window_menu": True}


def buttons(bed):
    return {w["label"] for w in bed.find_all(role="Button") if w.get("label")}


if sys.platform == "darwin":
    with Bed() as bed:
        bed.wait(2)
        labels = buttons(bed)
        assert "Menu" not in labels and "File" not in labels, (
            f"the menus were drawn beside the native bar; buttons: {sorted(labels)}"
        )

with Bed(settings=SHOWN) as bed:
    bed.wait(2)
    assert not any(label.startswith("File") for label in buttons(bed)), "the bar was drawn too"
    bed.screenshot("menu-button")
    bed.click("Menu")
    bed.wait(2)
    labels = buttons(bed)
    for menu in MENUS:
        assert any(label.startswith(menu) for label in labels), (
            f"no {menu} menu from the button; buttons: {sorted(labels)}"
        )

    file = next(label for label in labels if label.startswith("File"))
    bed.click(file)
    bed.wait(2)
    assert bed.exists(contains="Open Folder…"), "File had no Open Folder"
    bed.screenshot("menu-file")

with Bed(settings={**SHOWN, "appearance.compact_menu": False}) as bed:
    bed.wait(2)
    labels = buttons(bed)
    assert "Menu" not in labels, "the button was drawn beside the bar"
    for menu in MENUS:
        assert menu in labels, f"no {menu} in the menu bar; buttons: {sorted(labels)}"
    bed.screenshot("menu-bar")
