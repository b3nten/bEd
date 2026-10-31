"""Code actions, rename, find references and formatting with rust-analyzer:
Find References lists uses in a References panel; F2 renames across files
(the open one stays unsaved, the other is written); formatting tidies the
file; Cmd+. offers code actions, and choosing one applies it. Needs
rust-analyzer installed."""
import shutil
import tempfile
import time
from pathlib import Path
from bed import Bed

if not shutil.which("rust-analyzer") and not (Path.home() / ".cargo/bin/rust-analyzer").exists():
    print("skipped: rust-analyzer isn't installed")
    raise SystemExit(0)

MAIN = """mod other;

fn main() {
    other::helper();
    let count:u32=7;
    println!("{}", count);
}
"""


def until(bed, check, what, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def settled(bed, seconds=2.0):
    """rust-analyzer is ready and idle (just its name in the status bar), and
    stays so for `seconds`: it reports loading only once it has started."""
    deadline = time.monotonic() + 180
    since = None
    while time.monotonic() < deadline:
        if bed.exists("rust-analyzer"):
            since = since or time.monotonic()
            if time.monotonic() - since >= seconds:
                return
        else:
            since = None
        bed.wait(10)
    raise AssertionError("never: rust-analyzer settles")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def tab(bed, name):
    return max(bed.find_all(name), key=lambda widget: widget["rect"][0])


def editor(bed):
    return next(w for w in bed.widgets() if w["role"] == "TextInput" and w["rect"][0] > 200)


def text(bed):
    return editor(bed).get("value") or ""


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "Cargo.toml").write_text('[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n')
    (root / "src").mkdir()
    (root / "src" / "main.rs").write_text(MAIN)
    (root / "src" / "other.rs").write_text("pub fn helper() {}\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("src"), "the root is listed")
        bed.click(row(bed, "src"))
        until(bed, lambda: bed.exists("main.rs"), "src opens")
        x0, y0, x1, y1 = row(bed, "main.rs")["rect"]
        at = {"move": {"x": (x0 + x1) / 2, "y": (y0 + y1) / 2}}
        bed.input(at, {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        # Ready and not loading: the status shows just the server's name.
        settled(bed)

        # The caret onto `helper` (line 4, column 13).
        for _ in range(3):
            bed.key("ArrowDown")
        for _ in range(12):
            bed.key("ArrowRight")
        bed.key("Shift+F12")
        until(bed, lambda: bed.exists(contains="references to helper"), "references are listed")
        assert bed.exists(contains="pub fn helper"), "the definition's line is listed"
        bed.screenshot("code-actions-references")

        # Back to main.rs; F2 renames `helper` to `assist` everywhere.
        bed.click(tab(bed, "main.rs"))
        until(bed, lambda: editor(bed)["focused"], "main.rs is focused")
        bed.key("F2")
        until(bed, lambda: any(w.get("value") == "helper" for w in bed.widgets()), "the rename box")
        bed.key("Primary+A")
        bed.type("assist")
        bed.screenshot("code-actions-rename")
        bed.key("Enter")
        until(bed, lambda: "other::assist();" in text(bed), "main.rs is renamed")
        until(bed, lambda: (root / "src" / "other.rs").read_text() == "pub fn assist() {}\n",
              "other.rs is renamed on disk")
        assert "helper" in (root / "src" / "main.rs").read_text(), "main.rs was saved"

        # Formatting.
        bed.key("Primary+Shift+I")
        until(bed, lambda: "let count: u32 = 7;" in text(bed), "formatted")

        # Cmd+. offers code actions; Escape closes them.
        before = {w.get("label") for w in bed.widgets()}
        bed.key("Primary+Period")
        until(bed, lambda: {w.get("label") for w in bed.widgets()} - before - {None},
              "code actions are offered")
        bed.screenshot("code-actions-menu")
        offered = {w.get("label") for w in bed.widgets()} - before - {None}
        bed.key("Escape")
        until(bed, lambda: not ({w.get("label") for w in bed.widgets()} & offered), "the menu closes")
        # Again, and Enter takes the first: rust-analyzer fills in its edit only
        # when it's chosen (resolving it).
        bed.key("Primary+Period")
        until(bed, lambda: bed.exists("Replace qualified path with use"), "code actions again")
        first = next(w for w in bed.widgets() if w.get("label") == "Replace qualified path with use")
        bed.click(first)
        until(bed, lambda: "use crate::other::assist;" in text(bed), "the code action is applied")
        assert not bed.status()["errors"], bed.status()["errors"]
