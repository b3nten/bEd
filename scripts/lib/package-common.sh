#!/usr/bin/env bash
# Shared desktop packaging steps. See NOTICE for attribution.
set -euo pipefail
BED_PACKAGE_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BED_PACKAGE_VERSION=$(awk '$1 == "version" {gsub(/"/, "", $3); print $3; exit}' "$BED_PACKAGE_ROOT/Cargo.toml")
BED_PACKAGE_ICON_SOURCE="$BED_PACKAGE_ROOT/assets/bEd-iOS-Default-1024@1x.png"

prepare_bed_package() {
    test -f "$BED_PACKAGE_ICON_SOURCE" || {
        printf 'Missing application icon export: %s\n' "$BED_PACKAGE_ICON_SOURCE" >&2
        return 1
    }
    if [ "$bed_skip_build" = false ]; then
        if ! python3 "$BED_PACKAGE_ROOT/scripts/lib/remote-helpers.py" validate >/dev/null 2>&1; then
            bash "$BED_PACKAGE_ROOT/scripts/lib/build-remote-helpers.sh"
        fi
        cargo build --locked --release --bin bed
    fi
    test -f "$binary" || {
        printf 'Missing desktop executable: %s. Run this script without --skip-build.\n' "$binary" >&2
        exit 1
    }
    python3 "$BED_PACKAGE_ROOT/scripts/lib/remote-helpers.py" validate
}

copy_bed_resources() {
    local destination=$1
    mkdir -p "$destination"
    cp -R "$BED_PACKAGE_ROOT/resources" "$destination/resources"
    cp "$BED_PACKAGE_ICON_SOURCE" "$destination/resources/icons/bed.png"
    cp "$BED_PACKAGE_ROOT/LICENSE" "$BED_PACKAGE_ROOT/NOTICE" "$destination/"
    python3 "$BED_PACKAGE_ROOT/scripts/lib/collect-licenses.py" "$destination"
    python3 "$BED_PACKAGE_ROOT/scripts/lib/remote-helpers.py" copy --destination "$destination"
}

verify_bed_resources() {
    local destination=$1
    test -f "$destination/resources/config/bed.json"
    test -f "$destination/resources/fonts/SourceCodePro-Regular.ttf"
    test -f "$destination/resources/fonts/Emoji.ttf"
    test -f "$destination/resources/icons/bed.png"
    test -f "$destination/resources/queries/rs.scm"
    test -f "$destination/LICENSES/terminal-adapter-BSL-1.1.txt"
    test -f "$destination/LICENSES/dependencies/vendor/freetype-sys/freetype2/docs/FTL.TXT"
    test -f "$destination/LICENSES/dependencies/index.json"
    local binding license notice_path
    for binding in dear-imgui-sys dear-imgui-winit dear-imgui-wgpu; do
        for license in LICENSE-MIT LICENSE-APACHE; do
            notice_path="$destination/LICENSES/dependencies/vendor/$binding/$license"
            test -f "$notice_path" || {
                printf 'Missing Rust binding notice: %s\n' "$notice_path" >&2
                return 1
            }
        done
    done
    python3 "$BED_PACKAGE_ROOT/scripts/lib/remote-helpers.py" validate --source "$destination/remote-helpers"
}
