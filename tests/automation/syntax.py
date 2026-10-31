"""Syntax: a Rust file opens coloured, with fold arrows where blocks start;
folding hides the block's lines, and editing keeps the colours."""
import tempfile
from pathlib import Path
from bed import Bed

CODE = """fn main() {
    let cat = 1;
    println!("{}", cat);
}

// A second function.
fn two() -> u32 {
    2
}
"""


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}; text: {text(bed)!r}")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def text(bed):
    editors = [w for w in bed.widgets() if w["role"] == "TextInput" or (w.get("value") or "").startswith("fn main")]
    return editors[0].get("value") or "" if editors else ""


with tempfile.TemporaryDirectory() as folder:
    (Path(folder) / "main.rs").write_text(CODE)

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("main.rs"), "the root is listed")
        x0, y0, x1, y1 = row(bed, "main.rs")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists("Fold line 1") and bed.exists("Fold line 7"), "fold arrows")
        bed.screenshot("syntax")

        bed.click("Fold line 1")
        until(bed, lambda: bed.exists("Unfold line 1"), "line 1 folds")
        until(bed, lambda: "let cat" not in text(bed), "the block is hidden")
        bed.screenshot("syntax-folded")
        bed.click("Unfold line 1")
        until(bed, lambda: "let cat" in text(bed), "the block shows again")

        # Typing keeps colours and the new comment is analysed.
        bed.type("// top")
        bed.key("Enter")
        until(bed, lambda: text(bed).startswith("// top\nfn main"), "typing shows")
        bed.wait(20)
        bed.screenshot("syntax-edited")
        assert not bed.status()["errors"], bed.status()["errors"]
