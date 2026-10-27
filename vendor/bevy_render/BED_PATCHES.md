# bEd patches to bevy_render 0.20.0

Upstream: https://github.com/bevyengine/bevy (published crate 0.20.0).
Original MIT and Apache 2.0 licenses are retained alongside this file.

Window screenshots always copy the rendered capture texture into the readback
buffer, including frames without a swapchain drawable. Only the subsequent
capture-to-window blit requires a drawable. Copy dimensions and format come from
the prepared capture texture, matching the readback allocation.

On macOS/Metal (2026-10-24), a native Hex smoke intermittently returned completely
transparent captures. GPU probes showed valid pixels after both alpha conversion
and the final output blit, with `ExtractedWindow.swap_chain_texture_view == None`
on the failed capture frames. Upstream skipped the readback copy but still mapped
the unwritten buffer. No alpha-conversion or screenshot-content retry is needed.

The native `bedtime-automation2` example `screenshot_without_drawable` deliberately
removes the drawable after rendering and verifies colored and fully transparent
captures. Run it alongside the strict Hex/Filetree smokes when upgrading this fork;
remove the fork when the published renderer includes equivalent handling.
