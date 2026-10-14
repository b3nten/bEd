#!/usr/bin/env bash
# Xvfb needs an actual window manager to test minimize/restore and focus.
set -euo pipefail
cd "$(dirname "$0")/../.."
openbox > target/native-window-manager.log 2>&1 &
bed_window_manager=$!
trap 'kill "$bed_window_manager" 2>/dev/null || true' EXIT
sleep 1
# Exercise Bevy's X11 desktop backend under the Xvfb window manager.
unset WAYLAND_DISPLAY
cargo build --locked -p bedside-table
python3 tools/automation/run_smokes.py --bed-binary target/release/bed
python3 scripts/test-native.py
bash scripts/ci/desktop-consumer.sh
