"""Previews: Open Preview to the Side shows Markdown rendered (with its
images) and JSON/TOML as trees beside their editor, following its edits."""
import shutil
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(300):
        if check():
            return
        bed.wait(5)
    bed.screenshot("previews-failed")
    raise AssertionError(f"never: {what}")


def focus_editor(bed, name):
    """Click into the editor under `name`'s tab."""
    tab = max(bed.find_all(name), key=lambda widget: widget["rect"][0])
    x0, y0, x1, y1 = tab["rect"]
    bed.click((x0 + 40, y1 + 120))


def row(bed, name):
    """The tree's row (tabs with the same name are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


README = """# Previews

Some *emphasis*, **strong** text, `inline code` and [a link](#usage).

![A sample picture](images/sample.png)

- [x] done
- [ ] not yet
  - nested

> [!NOTE]
> A callout.

## Usage

```rust
fn main() {
    println!("hello");
}
```

| Name | Value |
|------|------:|
| one  |     1 |
"""

with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    (root / "README.md").write_text(README)
    (root / "images").mkdir()
    shutil.copy(Path(__file__).resolve().parents[1] / "fixtures" / "images" / "sample.png",
                root / "images" / "sample.png")
    (root / "data.json").write_text(
        '{"name": "bed", "version": 3, "tags": ["editor", "bevy"], "nested": {"deep": {"x": null}}}'
    )
    (root / "config.toml").write_text(
        'title = "bEd"\nwhen = 2026-11-02\n\n[server]\nport = 8080\n\n[[plugins]]\nname = "git"\n'
    )

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("README.md"), "the root is listed")

        # Markdown: the editor, then its preview beside it.
        bed.click(row(bed, "README.md"))
        until(bed, lambda: len(bed.find_all("README.md")) >= 2, "README.md opens in the editor")
        focus_editor(bed, "README.md")
        bed.key("Primary+Shift+M")
        until(bed, lambda: bed.exists(contains="Some emphasis, strong text"), "the preview renders")
        assert bed.exists(contains="println!"), "the code block shows"
        until(bed, lambda: bed.exists("A sample picture", role="Image"), "the image shows")
        bed.wait(30)
        bed.screenshot("previews-markdown")
        assert not bed.status()["errors"], bed.status()["errors"]

        # The editor keeps focus; typing in it shows in the preview.
        bed.type("Typed live. ")
        until(bed, lambda: bed.exists(contains="Typed live."), "the preview follows the edit")

        # JSON as a tree: top-level containers open, deeper ones closed.
        bed.click(row(bed, "data.json"))
        until(bed, lambda: len(bed.find_all("data.json")) >= 2, "data.json opens")
        focus_editor(bed, "data.json")
        bed.key("Primary+Shift+M")
        until(bed, lambda: bed.exists("tags: 2 items"), "the JSON tree shows")
        assert bed.exists("0: editor"), "open arrays list their items"
        assert bed.exists("deep: 1 key") and not bed.exists("x: null"), "deeper levels start closed"
        bed.click(bed.find("deep: 1 key"))
        until(bed, lambda: bed.exists("x: null"), "a click opens a table")
        bed.screenshot("previews-json")
        assert not bed.status()["errors"], bed.status()["errors"]

        # TOML, and an edit that doesn't parse keeps the last tree.
        bed.click(row(bed, "config.toml"))
        until(bed, lambda: len(bed.find_all("config.toml")) >= 2, "config.toml opens")
        focus_editor(bed, "config.toml")
        bed.key("Primary+Shift+M")
        until(bed, lambda: bed.exists("port: 8080"), "the TOML tree shows")
        assert bed.exists("when: 2026-11-02"), "dates show"
        assert bed.exists("plugins: 1 item"), "arrays of tables are arrays"
        bed.type("= broken")
        until(bed, lambda: bed.exists(contains="Showing the last version that parsed"),
              "a parse error is reported")
        assert bed.exists("port: 8080"), "the last tree stays"
        bed.screenshot("previews-toml")
        assert not bed.status()["errors"], bed.status()["errors"]
