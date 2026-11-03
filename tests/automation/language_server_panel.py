"""The Language Servers panel: rust-analyzer shows as running for an open
Rust file; disabling it stops it, enabling starts it again; servers that
aren't installed are listed, collapsed. Needs rust-analyzer installed."""
import shutil
import tempfile
import time
from pathlib import Path
from bed import Bed

if not shutil.which("rust-analyzer") and not (Path.home() / ".cargo/bin/rust-analyzer").exists():
    print("skipped: rust-analyzer isn't installed")
    raise SystemExit(0)


def until(bed, check, what, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def card_status(bed):
    """rust-analyzer's card's status line."""
    for widget in bed.widgets():
        text = widget.get("label") or widget.get("value") or ""
        if text.startswith(("Running", "Starting", "Disabled", "Starts with", "Failed", "Stopped")):
            return text
    return None


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "Cargo.toml").write_text('[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n')
    (root / "src").mkdir()
    (root / "src" / "main.rs").write_text("fn main() {}\n")

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("src"), "the root is listed")
        bed.click(row(bed, "src"))
        until(bed, lambda: bed.exists("main.rs"), "src opens")
        bed.click(row(bed, "main.rs"))
        bed.invoke("app.open_language_servers")
        until(bed, lambda: bed.exists("rust-analyzer"), "the panel lists rust-analyzer")
        until(bed, lambda: (card_status(bed) or "").startswith("Running"), "rust-analyzer runs", 120)
        assert bed.exists(contains="Not installed ("), "the not-installed section"
        bed.screenshot("language-servers")

        # Off: it stops. On: it starts again.
        toggle = next(w for w in bed.widgets() if w["role"] == "CheckBox")
        bed.click(toggle)
        until(bed, lambda: card_status(bed) == "Disabled", "disabling stops it")
        bed.click(next(w for w in bed.widgets() if w["role"] == "CheckBox"))
        until(bed, lambda: (card_status(bed) or "").startswith(("Starting", "Running")), "enabling starts it", 120)
        until(bed, lambda: (card_status(bed) or "").startswith("Running"), "it runs again", 120)

        # Restart, from its button.
        bed.click(bed.find("rotate-ccw"))
        until(bed, lambda: (card_status(bed) or "").startswith("Running"), "it runs after restarting", 120)
        assert not bed.status()["errors"], bed.status()["errors"]
