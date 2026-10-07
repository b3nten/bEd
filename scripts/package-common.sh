#!/usr/bin/env bash
# Runtime resource layout used by all Bed packages. See NOTICE for attribution.
set -euo pipefail
BED_PACKAGE_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BED_PACKAGE_VERSION=$(awk '$1 == "version" {gsub(/"/, "", $3); print $3; exit}' "$BED_PACKAGE_ROOT/Cargo.toml")

copy_bed_resources() {
    local destination=$1
    mkdir -p "$destination"
    cp -R "$BED_PACKAGE_ROOT/resources" "$destination/resources"
    cp -R "$BED_PACKAGE_ROOT/resources/queries" "$destination/queries"
    cp -R "$BED_PACKAGE_ROOT/LICENSES" "$destination/LICENSES"
    cp "$BED_PACKAGE_ROOT/LICENSE" "$BED_PACKAGE_ROOT/NOTICE" "$BED_PACKAGE_ROOT/PORTING.md" "$destination/"
    python3 "$BED_PACKAGE_ROOT/scripts/collect-licenses.py" "$destination"
    python3 "$BED_PACKAGE_ROOT/scripts/package-remote-helpers.py" copy --destination "$destination"
}
verify_bed_resources() {
    local destination=$1
    test -f "$destination/resources/config/bed.json"
    test -f "$destination/resources/fonts/SourceCodePro-Regular.ttf"
    test -f "$destination/resources/fonts/Emoji.ttf"
    test -f "$destination/resources/icons/bed.png"
    test -f "$destination/queries/rs.scm"
    test -f "$destination/LICENSES/terminal-adapter-BSL-1.1.txt"
    test -f "$destination/LICENSES/dependencies/vendor/freetype-sys/freetype2/docs/FTL.TXT"
    test -f "$destination/LICENSES/dependencies/index.json"
    python3 "$BED_PACKAGE_ROOT/scripts/package-remote-helpers.py" validate --source "$destination/remote-helpers"
}
