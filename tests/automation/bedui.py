"""The bedtime-ui playground (src/bedui): its three panels open on the first
run, each story draws when it's chosen (a screenshot of each, in the dark
and the light theme, in target/automation), and a control changes the story."""
import shutil
import tempfile
from bed import Bed

STORIES = ["Tokens", "Button", "Text", "Row and Column"]


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    bed.screenshot("bedui-failed")
    raise AssertionError(f"never: {what}")


config = tempfile.mkdtemp(prefix="bedui-automation-")
try:
    with Bed(app="bedui", config=config) as bed:
        until(bed, lambda: all(bed.exists(story) for story in STORIES), "the stories are listed")
        until(bed, lambda: bed.exists("Space"), "the first story draws")
        for story in STORIES:
            bed.click(story)
            bed.wait(20)
            bed.screenshot("bedui-" + story.lower().replace(" ", "-"))

        # A control changes the story: the Button story's variant.
        bed.click("Button")
        until(bed, lambda: bed.exists("Save changes"), "the Button story draws")
        until(bed, lambda: bed.exists("Disabled"), "its controls draw")
        bed.click("Lg")
        bed.click("Icon")
        bed.wait(20)
        bed.screenshot("bedui-button-large-icon")
        bed.click("Disabled")
        bed.wait(20)
        bed.screenshot("bedui-button-disabled")

    # bedtime-ui's own light theme, chosen as any app without a themes folder can.
    with Bed(app="bedui", settings={"appearance.theme": '"everforest-light-medium"'}) as bed:
        until(bed, lambda: bed.exists("Button"), "the stories are listed")
        for story in STORIES:
            bed.click(story)
            bed.wait(20)
            bed.screenshot("bedui-light-" + story.lower().replace(" ", "-"))
        bed.invoke("settings.show")
        until(bed, lambda: bed.exists("Everforest Light Medium"), "Settings shows the chosen theme")
finally:
    shutil.rmtree(config, ignore_errors=True)
