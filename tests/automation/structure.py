"""Structure: lists the last focused editor's declarations; clicking one
brings that editor back, with the name selected."""
import tempfile
from pathlib import Path
from bed import Bed

CODE = "fn cat() {\n    let age = 42;\n}\n\nstruct Dog {\n    name: String,\n}\n"


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def editor(bed):
    return next(w for w in bed.widgets() if w["role"] == "TextInput")


with tempfile.TemporaryDirectory() as folder:
    (Path(folder) / "pets.rs").write_text(CODE)

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("pets.rs"), "the root is listed")
        x0, y0, x1, y1 = row(bed, "pets.rs")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: bed.exists("Fold line 1"), "pets.rs is analysed")

        bed.invoke("code.show_structure")
        until(bed, lambda: bed.exists(contains="Dog"), "Structure lists the declarations")
        assert bed.exists(contains="cat"), [w.get("label") for w in bed.widgets()]
        bed.screenshot("structure")

        # Its chevron collapses an entry, and expands it again.
        bed.click("Collapse Dog")
        until(bed, lambda: not bed.exists(contains="name"), "Dog collapses")
        bed.click("Expand Dog")
        until(bed, lambda: bed.exists(contains="name"), "Dog expands")

        bed.click(bed.find(contains="Dog"))
        until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "the editor is back")
        until(bed, lambda: editor(bed)["focused"], "the editor has focus")
        bed.type("Big")
        until(bed, lambda: "struct Big {" in (editor(bed).get("value") or ""), "the name was selected")
        bed.screenshot("structure-revealed")
        assert not bed.status()["errors"], bed.status()["errors"]
