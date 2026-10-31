"""Language servers: a Rust file in a Cargo project gets rust-analyzer. The
status bar shows the server and the file's errors; Go to Definition opens the
definition in another file. Needs rust-analyzer installed."""
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
    let count: u32 = "seven";
}
"""


def until(bed, check, what, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


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

        until(bed, lambda: bed.exists(contains="rust-analyzer"), "the server shows in the status bar")
        until(bed, lambda: bed.exists(contains="1 error"), "the type error is reported")
        bed.screenshot("language-servers")

        # The caret onto `helper` (line 4, after `    other::`), then F12.
        for _ in range(3):
            bed.key("ArrowDown")
        for _ in range(12):
            bed.key("ArrowRight")
        until(bed, lambda: bed.exists("rust-analyzer"), "indexing finishes")
        bed.key("F12")
        until(bed, lambda: bed.exists("other.rs") and len(bed.find_all("other.rs")) == 2,
              "the definition opens", seconds=30)
        bed.screenshot("language-servers-definition")
        assert not bed.status()["errors"], bed.status()["errors"]
