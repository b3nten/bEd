"""Structure (outline polish): an editor's outline beside it, with kind icons and details; it
follows the caret, filters, and its hovered entry is shaded in the editor.
The editor shows breadcrumbs, and Go to Symbol in File finds an entry."""
import tempfile
from pathlib import Path
from bed import Bed

CODE = """struct Cat {
    age: u32,
    name: String,
}

impl Cat {
    fn meow(&self, times: usize) -> bool {
        times > 0
    }

    fn rename(&mut self, name: &str) {
        self.name = name.into();
    }
}

enum Mood {
    Happy,
    Grumpy,
}

const LIVES: u32 = 9;

fn dog(name: &str) -> Option<String> {
    None
}
"""


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The leftmost widget named `name` (Files' row, not a tab)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def center(widget):
    x0, y0, x1, y1 = widget["rect"]
    return ((x0 + x1) / 2, (y0 + y1) / 2)


def structure_rows(bed, name):
    """Structure's rows named `name` (the rightmost, past the editor)."""
    return max(bed.find_all(name), key=lambda widget: widget["rect"][0])


with tempfile.TemporaryDirectory() as folder:
    (Path(folder) / "cats.rs").write_text(CODE)

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("cats.rs"), "the root is listed")
        x, y = center(row(bed, "cats.rs"))
        at = {"move": {"x": x, "y": y}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists(contains="struct Cat"), "cats.rs opens")

        bed.invoke("code.show_structure")
        until(bed, lambda: bed.exists("meow"), "Structure lists the outline")
        # Beside the editor: its tab dragged to the window's right edge.
        tab = [w for w in bed.find_all("Structure", role="Button") if w["rect"][1] < 130][0]
        window = bed.status()["window"]
        bed.drag(center(tab), (window["width"] - 30, window["height"] / 2), steps=20)
        bed.wait(20)
        bed.screenshot("structure-outline")

        # Hovering an entry shades it in the editor.
        bed.move(*center(structure_rows(bed, "rename")))
        bed.wait(30)
        bed.screenshot("structure-hover")

        # Filtering.
        bed.click(bed.find("Filter", role="TextInput"))
        bed.type("mw")
        bed.wait(10)
        bed.screenshot("structure-filter")
        bed.key("Escape")
        until(bed, lambda: bed.find("Filter", role="TextInput")["value"] == "", "Escape clears the filter")

        # Go to Symbol in the editor.
        bed.click(center(bed.find(contains="struct Cat")))
        bed.invoke("code.go_to_symbol")
        until(bed, lambda: bed.exists("Go to symbol", role="TextInput"), "the picker opens")
        bed.type("rn")
        bed.wait(10)
        bed.screenshot("go-to-symbol")
        bed.key("Enter")
        bed.wait(20)
        bed.screenshot("structure-after-go-to")
        # The caret went to `rename`; the breadcrumbs name it.
        assert not bed.exists("Go to symbol", role="TextInput")
        assert any(w["role"] == "Button" for w in bed.find_all("rename")), "a breadcrumb"
        assert not bed.status()["errors"], bed.status()["errors"]
