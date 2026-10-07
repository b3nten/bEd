#!/usr/bin/env bash
# Xvfb needs an actual window manager to test minimize/restore and focus.
set -euo pipefail
openbox > target/native-window-manager.log 2>&1 &
bed_window_manager=$!
trap 'kill "$bed_window_manager" 2>/dev/null || true' EXIT
sleep 1
# Dear ImGui's pinned winit viewport backend requires the X11 desktop space.
unset WAYLAND_DISPLAY
target/release/bed --lifecycle-smoke --capture-after-frames 30 \
  --capture-frame target/native-window.ppm --config-dir target/native-config src/main.rs
test -s target/native-window.ppm
target/release/examples/embed_host --smoke-test --capture-frame target/native-embedded.ppm
test -s target/native-embedded.ppm
mkdir -p target/native-project
cp src/main.rs target/native-project/main.rs
target/release/bed target/native-project target/native-project/main.rs --viewports-smoke \
  --capture-after-frames 30 --capture-frame target/native-viewports.ppm \
  --config-dir target/native-viewport-config
test -s target/native-viewports-secondary-1.ppm
target/release/bed --main-only-smoke --capture-after-frames 30 \
  --capture-frame target/native-internal.ppm --config-dir target/native-internal-config
test -s target/native-internal.ppm
