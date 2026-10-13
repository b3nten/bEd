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
cargo run --locked -p bed-desktop --example lifecycle_smoke
test -s target/native-window.ppm
cargo run --locked -p bed-example-terminal --example terminal_smoke
test -s target/native-terminal.ppm
mkdir -p target/native-project
cp src/main.rs target/native-project/main.rs
target/release/bed --main-only-smoke --capture-after-frames 30 \
  --capture-frame target/native-internal.ppm --config-dir target/native-internal-config
test -s target/native-internal.ppm

cargo run --locked -p bed-desktop --example native_smoke
cargo run --locked -p bed-desktop --example canvas_smoke
cargo run --locked -p bed-desktop --example automation_smoke
cargo run --locked -p bed-desktop --example window_config_smoke
cargo run --locked -p bed-launch --example workbench_smoke
cargo run --locked -p bed-launch --example effects_smoke
cargo run --locked -p bed-launch --features language-services --example git_smoke
cargo run --locked -p bed-launch --features language-services --example viewers_smoke
cargo run --locked -p bed-example-layouts -- --smoke-test --capture-frame target/native-layout.ppm
test -s target/native-layout.ppm
cargo run --locked -p bed-example-editor -- --smoke-test
cargo run --locked -p bed-example-model -- --smoke-test
bash scripts/ci/desktop-consumer.sh
