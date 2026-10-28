"""Themes are files in <config>/themes: bed writes its own there when missing,
and picks up new, edited and broken files while it runs."""
import time
from pathlib import Path
from bed import Bed

MINE = Path(__file__).with_name("theme_fixture.ron").read_text()


def settle(bed):
    """The folder is looked at about once a second."""
    time.sleep(1.5)
    bed.wait(5)


with Bed(settings={"appearance.theme": '"tokyo"'}) as bed:
    folder = bed.config / "themes"
    assert len(list(folder.glob("*.ron"))) == 20, "bed's themes were written"
    bed.key("Primary+2")
    bed.invoke("app.open_settings")
    assert bed.exists("Tokyo Night"), "the chosen theme is shown"
    bed.screenshot("theme-tokyo")

    # Editing a shipped theme's file changes it live.
    tokyo = folder / "tokyo.ron"
    tokyo.write_text(tokyo.read_text().replace('"Tokyo Night"', '"Tokyo Edited"'))
    settle(bed)
    assert bed.exists("Tokyo Edited"), "the edit was picked up"

    # A new file shows up, and can be chosen (here through the settings file,
    # which bed reloads; choosing from the list is covered by settings.py).
    (folder / "mine.ron").write_text(MINE)
    settle(bed)
    bed.click("Tokyo Edited")
    assert bed.exists("Mine", visible=False), "the new theme is listed"
    bed.key("Escape")
    settings = bed.config / "global" / "settings.ron"
    settings.write_text(settings.read_text().replace('"tokyo"', '"mine"'))
    settle(bed)
    assert bed.exists("Mine") and not bed.exists("Tokyo Edited"), "the theme is chosen"
    bed.screenshot("theme-mine")

    # A broken file is reported, and the theme falls back.
    (folder / "mine.ron").write_text("(")
    settle(bed)
    assert bed.exists(contains="have errors", visible=False), "the broken file is reported"
    assert bed.exists("Not found"), "the chosen theme is gone"
    assert not bed.status()["errors"]
