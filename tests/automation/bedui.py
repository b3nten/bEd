"""The bedtime-ui playground (src/bedui): its three panels open on the first
run, each story draws when it's chosen (a screenshot of each, in the dark
and the light theme, in target/automation), and a control changes the story."""
import shutil
import tempfile
from bed import Bed

STORIES = ["Tokens", "Button", "Text", "Checkbox and Toggle", "Text Field", "Select", "Segmented",
           "Slider", "Text Area", "List Row and Menu", "Table", "Spinner",
           "Section and Separator", "Row and Column"]


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

        # A select opens, and a click on an option chooses it.
        bed.click("Select")
        until(bed, lambda: bed.exists("Chosen: GET"), "the Select story draws")
        bed.click("GET", role="ComboBox")
        until(bed, lambda: bed.exists("DELETE"), "the options show")
        bed.wait(150)  # the list fades in
        bed.screenshot("bedui-select-open")
        bed.click("DELETE")
        until(bed, lambda: bed.exists("Chosen: DELETE"), "the option is chosen")

        # A searchable one narrows as you type, and Enter chooses the best match.
        bed.click("Rust", role="ComboBox")
        until(bed, lambda: bed.exists("Search"), "the search field shows")
        bed.type("type")
        until(bed, lambda: bed.exists("TypeScript") and not bed.exists("Haskell"), "the list narrows")
        bed.wait(150)
        bed.screenshot("bedui-select-search")
        bed.key("Enter")
        until(bed, lambda: bed.exists("Chosen: TypeScript"), "Enter chooses the match")

        # Widgets bound to the world: a toggle that is a setting (both of
        # its forms follow it), and a button that is an action.
        bed.click("Actions and Settings")
        until(bed, lambda: bed.exists("Rainbow caret"), "the bound story draws")
        both = lambda on: all(w["toggled"] == str(on) for w in bed.find_all(role="CheckBox")
                              if w["label"] in ("Rainbow caret", "The same setting, as a checkbox"))
        assert both(True), "the setting starts on"
        bed.click("Rainbow caret")
        until(bed, lambda: both(False), "flipping one changes the setting, and so the other")
        bed.screenshot("bedui-bound")
        bed.click("Settings")
        until(bed, lambda: bed.exists("Appearance"), "the action button opens Settings")
        bed.click("Canvas")

        # A checkbox ticks.
        bed.click("Checkbox and Toggle")
        until(bed, lambda: bed.exists("Show hidden files"), "the Checkbox story draws")
        bed.click("Show hidden files")
        bed.wait(20)
        bed.screenshot("bedui-checkbox-ticked")

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
