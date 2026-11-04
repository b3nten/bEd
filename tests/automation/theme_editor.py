"""The Theme Editor edits a theme's file: colors are saved shortly after a
change, a shipped theme can be reset, and a copy can be made, renamed and
deleted."""
import time
from bed import Bed


def saved(bed):
    """Edits are saved about half a second after the last one."""
    time.sleep(0.8)
    bed.wait(5)


with Bed(settings={"appearance.theme": '"tokyo"'}) as bed:
    folder = bed.config / "themes"
    tokyo = folder / "tokyo.ron"
    shipped = tokyo.read_text()
    bed.key("Primary+2")
    bed.invoke("app.open_theme_editor")
    assert bed.exists("Tokyo Night"), "the theme in use is edited"
    assert bed.find("Reset theme")["disabled"], "nothing to reset yet"
    bed.screenshot("theme-editor")

    # Typing a color saves it to the theme's file.
    while not bed.exists("#7aa2f7"):
        bed.scroll(-120, at="Interface")
    bed.click(bed.find("#7aa2f7"))
    bed.key("Primary+A")
    bed.type("#ff0000")
    saved(bed)
    assert 'accent: "#ff0000"' in tokyo.read_text(), "the accent was saved"
    bed.key("Escape")

    # A shipped theme resets to what bEd ships.
    bed.click("Reset theme")
    assert tokyo.read_text() == shipped, "the file is as shipped"
    assert bed.exists("#7aa2f7")

    # Duplicate, then rename: the file follows the name.
    bed.click("Duplicate theme")
    assert bed.exists("Tokyo Night Copy")
    bed.click("Create")
    copy = folder / "tokyo-night-copy.ron"
    assert copy.exists(), "the copy was written"
    assert not bed.exists("Reset theme") and bed.exists("Delete theme")
    bed.click(bed.find("Tokyo Night Copy", role="TextInput"))
    bed.key("Primary+A")
    bed.type("Midnight")
    bed.key("Enter")
    saved(bed)
    assert not copy.exists() and (folder / "midnight.ron").exists(), "renamed"
    assert '"Midnight"' in (folder / "midnight.ron").read_text()
    bed.screenshot("theme-editor-copy")

    # A custom theme can be deleted.
    bed.click("Delete theme")
    bed.click("Delete")
    assert not (folder / "midnight.ron").exists(), "deleted"
    assert bed.exists("Tokyo Night"), "back to the theme in use"
    assert not bed.status()["errors"]

    # Settings' Theme row opens the editor on the chosen theme.
    (folder / "solarized-light.ron").write_text(
        (folder / "solarized-light.ron").read_text())
    settings = bed.config / "global" / "settings.ron"
    settings.write_text(settings.read_text().replace('"tokyo"', '"solarized-light"'))
    time.sleep(1.5)
    bed.wait(5)
    bed.invoke("settings.open")
    bed.click("Edit theme")
    assert bed.exists("Solarized Light", role="TextInput"), "editing the chosen theme"
    assert not bed.status()["errors"]
