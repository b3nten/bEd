"""The hello example (src/hello): an app of bedtime's plugins
and one of its own, without bEd. It starts with a counter; a click counts;
the status bar shows the total; its setting is in Settings without being
laid out; its action opens another; Quit saves; and the next run has what
was saved."""
import shutil
import tempfile
from bed import Bed


def until(bed, check, what):
    for _ in range(200):
        if check():
            return
        bed.wait(5)
    bed.screenshot("hello-failed")
    raise AssertionError(f"never: {what}")


config = tempfile.mkdtemp(prefix="hello-automation-")
try:
    with Bed(app="hello", config=config) as bed:
        until(bed, lambda: bed.exists("Add 1"), "a counter on the first run")
        bed.click("Add 1")
        bed.click("Add 1")
        until(bed, lambda: bed.exists("2"), "the count goes up")
        until(bed, lambda: bed.exists("Total 2"), "the status bar's total")
        bed.screenshot("hello")

        # The plugin's setting, in the section it declared.
        bed.invoke("settings.show")
        until(bed, lambda: bed.exists("Counter") and bed.exists("Step"), "Step shows in Settings")
        bed.screenshot("hello-settings")
        # It has no documents, so nothing about them.
        assert not bed.exists("Documents") and not bed.exists("Autosave"), "a documents setting shows"

        bed.invoke("counter.show")
        until(bed, lambda: bed.exists("Add 1") and bed.exists("0"), "another counter opens")

        # Quit, as its key would: the app saves and exits.
        # (The app goes while the script waits on it: only ask.)
        bed.call("bedtime.invoke", {"command": "app.quit"})
        assert bed.process.wait(10) == 0, "Quit didn't exit cleanly"

    with Bed(app="hello", config=config) as bed:
        until(bed, lambda: bed.exists("Total 2"), "the next run has the count")
finally:
    shutil.rmtree(config, ignore_errors=True)
