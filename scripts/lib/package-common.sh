#!/usr/bin/env bash
# Shared desktop packaging steps. See NOTICE for attribution.
set -euo pipefail
# Keep macOS AppleDouble metadata out of portable runtime archives.
export COPYFILE_DISABLE=1
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
    test -f "$destination/resources/config/settings.json"
    test -f "$destination/resources/fonts/PaperMono-Regular.ttf"
    test -f "$destination/resources/fonts/PaperMono-Bold.ttf"
    test -f "$destination/resources/fonts/PaperMono-OFL.txt"
    local theme
    for theme in tokyo solarized-light carbon catppuccin-latte catppuccin-frappe catppuccin-macchiato catppuccin-mocha rose-pine rose-pine-moon rose-pine-dawn synthwave-84 everforest-dark-hard everforest-dark-medium everforest-dark-soft everforest-light-hard everforest-light-medium everforest-light-soft oxocarbon-dark oxocarbon-light; do
        test -f "$destination/resources/themes/$theme.json"
    done
    for theme in tokyo solarized carbon catppuccin rose-pine synthwave everforest oxocarbon; do
        test -f "$destination/resources/themes/$theme-LICENSE.txt"
    done
    python3 - "$destination" <<'PYFONT'
import sys
from pathlib import Path
fonts = Path(sys.argv[1]) / "resources/fonts"
actual = {p.name for p in fonts.rglob("*") if p.suffix.lower() in {".ttf", ".otf", ".ttc", ".otc", ".woff", ".woff2"}}
assert actual == {"PaperMono-Regular.ttf", "PaperMono-Bold.ttf"}, actual
PYFONT
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
