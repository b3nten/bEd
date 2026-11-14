"""The Schedules panel: it opens from its command, reads bed's own schedules,
and each of its views draws."""
from bed import Bed

with Bed() as bed:
    bed.invoke("schedules.show")
    bed.settle(5)
    assert bed.exists("Schedules"), "the panel opened"
    assert bed.exists(contains="schedules ·"), "the schedules were read"
    for view in ["Graph", "Arcs", "Timeline", "Flame", "List", "Lanes"]:
        bed.click(view)
        bed.settle(5)
        bed.screenshot(f"schedules-{view.lower()}")
    # Graph shows one schedule: Update, with the systems bed's plugins added.
    bed.click("Graph")
    bed.settle(5)
    assert bed.exists(contains="ordering edges"), "a schedule's graph is shown"
