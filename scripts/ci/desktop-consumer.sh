#!/usr/bin/env bash
# Launch the independent app without repository files in its working directory.
set -euo pipefail
bed_consumer_root=$(cd "$(dirname "$0")/../.." && pwd)
bed_consumer_directory=$(mktemp -d)
trap 'rm -rf "$bed_consumer_directory"' EXIT
export CARGO_TARGET_DIR="$bed_consumer_root/target"
cargo build --locked --manifest-path "$bed_consumer_root/bedtime/tests/fixtures/desktop-app/Cargo.toml"
(cd "$bed_consumer_directory" && "$CARGO_TARGET_DIR/debug/desktop-consumer" --smoke)
mkdir -p "$CARGO_TARGET_DIR/desktop-consumer-smokes/base"
cp "$bed_consumer_directory/target/"desktop-consumer-*.png "$CARGO_TARGET_DIR/desktop-consumer-smokes/base/"
cargo build --locked --manifest-path "$bed_consumer_root/bedtime/tests/fixtures/desktop-app/Cargo.toml" --features layout,effects
(cd "$bed_consumer_directory" && "$CARGO_TARGET_DIR/debug/desktop-consumer" --smoke)
mkdir -p "$CARGO_TARGET_DIR/desktop-consumer-smokes/layout-effects"
cp "$bed_consumer_directory/target/"desktop-consumer-*.png "$CARGO_TARGET_DIR/desktop-consumer-smokes/layout-effects/"
