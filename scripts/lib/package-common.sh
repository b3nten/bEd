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
    cp "$BED_PACKAGE_ROOT/LICENSE" "$destination/"
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
    for theme in tokyo solarized-light carbon slash catppuccin-latte catppuccin-frappe catppuccin-macchiato catppuccin-mocha rose-pine rose-pine-moon rose-pine-dawn synthwave-84 everforest-dark-hard everforest-dark-medium everforest-dark-soft everforest-light-hard everforest-light-medium everforest-light-soft oxocarbon-dark oxocarbon-light; do
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
    python3 "$BED_PACKAGE_ROOT/scripts/lib/verify-icons.py" "$destination/resources/icons"
    test -f "$destination/resources/queries/rs.scm"
    test -f "$destination/resources/terminal/LICENSE"
    test -f "$destination/resources/terminal/ATTRIBUTION.txt"
    test -f "$destination/license-index.json"
    python3 - "$BED_PACKAGE_ROOT" "$destination" <<'PYNOTICE'
import importlib.util
import sys
from pathlib import Path
repository, destination = map(Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("collect_licenses", repository / "scripts/lib/collect-licenses.py")
licenses = importlib.util.module_from_spec(spec)
spec.loader.exec_module(licenses)
bundle = (destination / "NOTICE").read_bytes()
if licenses.original_notice(bundle, "ned") != licenses.original_notice((repository / "NOTICE").read_bytes(), "ned"):
    raise SystemExit("Changed upstream ned notice in packaged NOTICE")
required = ["vendor/freetype-sys/freetype2/docs/FTL.TXT"]
required += [f"vendor/{binding}/{license}"
             for binding in ("dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu")
             for license in ("LICENSE-MIT", "LICENSE-APACHE")]
for section in required:
    source = repository / section
    if not source.is_file():
        raise SystemExit(f"Missing dependency notice: {section}")
    if licenses.original_notice(bundle, section) != source.read_bytes():
        raise SystemExit(f"Changed dependency notice in packaged NOTICE: {section}")
PYNOTICE
    python3 "$BED_PACKAGE_ROOT/scripts/lib/remote-helpers.py" validate --source "$destination/remote-helpers"
}
