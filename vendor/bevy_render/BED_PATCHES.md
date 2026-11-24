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

## Unsupported composite alpha modes fall back to `Auto`

A window that asks for a `CompositeAlphaMode` its surface doesn't offer is
configured with `Auto` instead, with a warning. wgpu treats an unsupported
explicit mode as a configuration error, and `Auto` itself only ever chooses
`Opaque` or `Inherit`, so a transparent window has to name its mode. bEd asks for
`PreMultiplied` on Linux (2026-11-23): Wayland surfaces offer it, while X11
surfaces commonly offer only `Opaque`/`Inherit`, where the window then stays
opaque rather than failing to open. Not yet exercised on a Linux machine; check
both a Wayland and an X11 session when upgrading this fork.
