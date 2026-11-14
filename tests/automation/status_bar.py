"""Status bar: shows the workspace, and a setting that can't be saved is
reported there instead of in a dialog."""
import os
import stat

from bed import Bed

with Bed() as bed:

    # Settings are saved in the global store; make it read-only so saving fails.
    global_dir = bed.config / "global"
    mode = global_dir.stat().st_mode
    os.chmod(global_dir, stat.S_IRUSR | stat.S_IXUSR)
    try:
        bed.key("Primary+Comma")
        bed.click(bed.find(role="CheckBox"))
        for _ in range(100):
            if bed.exists(contains="Couldn't save settings"):
                break
            bed.wait(5)
        assert bed.exists(contains="Couldn't save settings"), "the failed save wasn't reported"
        assert not bed.exists(role="Dialog"), "it was a dialog"
        message = bed.find(contains="Couldn't save settings")
        assert message["rect"][1] > bed.find("Appearance")["rect"][1], "not along the bottom"
        bed.screenshot("status-bar-error")
    finally:
        os.chmod(global_dir, mode)
