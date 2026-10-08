#!/usr/bin/env bash
# Build and package bEd.app, optionally install it into /Applications.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: scripts/package-macos.sh [--skip-build] [--install]

Build the release application and missing static SSH helpers, then produce
target/dist/bEd.app and its ZIP archive. --install also installs /Applications/bEd.app.
--skip-build packages existing binaries and validated helper bundles (used in CI).

Requires macOS, Rust, Xcode Command Line Tools and Python 3. Building missing
Linux SSH helpers also requires Docker. BED_BINARY, BED_DIST_DIR and
BED_HELPERS_DIR override the executable, output and prebuilt helper directories.
EOF
}
bed_skip_build=false
bed_install=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) bed_skip_build=true ;;
        --install) bed_install=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
source "$(dirname "$0")/lib/package-common.sh"
cd "$BED_PACKAGE_ROOT"
test "$(uname -s)" = Darwin || { echo 'Package the macOS application on macOS.' >&2; exit 1; }
binary=${BED_BINARY:-target/release/bed}
prepare_bed_package
dist=${BED_DIST_DIR:-target/dist}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/bed-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/bEd.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/bed"
copy_bed_resources "$app/Contents/Resources"
iconset="$stage/bed.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips --resampleHeightWidth "$size" "$size" "$BED_PACKAGE_ICON_SOURCE" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    retina_size=$((size * 2))
    sips --resampleHeightWidth "$retina_size" "$retina_size" "$BED_PACKAGE_ICON_SOURCE" \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/bed.icns" "$iconset"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>bed</string>
<key>CFBundleIconFile</key><string>bed.icns</string>
<key>CFBundleIdentifier</key><string>org.bed-editor.bed</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundleName</key><string>bEd</string>
<key>CFBundleDisplayName</key><string>bEd</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$BED_PACKAGE_VERSION</string>
<key>CFBundleVersion</key><string>$BED_PACKAGE_VERSION</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
plutil -lint "$app/Contents/Info.plist"
# Bed bundles FreeType, PlutoSVG, zlib and local libgit2 statically. Detect an
# accidental external dependency instead of silently shipping a broken bundle.
while IFS= read -r dependency; do
    case "$dependency" in /System/*|/usr/lib/*) ;; *) echo "Unbundled dependency: $dependency" >&2; exit 1 ;; esac
done < <(otool -L "$app/Contents/MacOS/bed" | awk 'NR > 1 {print $1}')
verify_bed_resources "$app/Contents/Resources"
codesign --force --sign - "$app/Contents/MacOS/bed"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
output="$dist/bEd-$BED_PACKAGE_VERSION-$(uname -m).zip"
# Finish the replacement before removing the previously published archive.
staged_output="$stage/${output##*/}"
ditto -c -k --sequesterRsrc --keepParent "$app" "$staged_output"
# Recreate the entry so a legacy Bed-*.zip also changes case on APFS.
rm -f "$output"
mv -f "$staged_output" "$output"
rm -rf "$dist/Bed.app" "$dist/bEd.app"
mv "$app" "$dist/bEd.app"
echo "$output"
rm -rf "$stage"
trap - EXIT
install_bed_app() {
    install_stage=$(mktemp -d /Applications/.bed-install.XXXXXX)
    previous_app="$install_stage/Previous.app"
    previous_path=''
    install_complete=false
    cleanup_install() {
        if [ "$install_complete" = true ] || { [ ! -e "$previous_app" ] && [ ! -L "$previous_app" ]; }; then
            rm -rf "$install_stage"
        else
            printf 'Previous application retained at %s\n' "$previous_app" >&2
        fi
    }
    trap cleanup_install EXIT
    ditto "$1" "$install_stage/bEd.app"
    codesign --verify --strict "$install_stage/bEd.app"
    # Enumerate the actual directory entries: existence checks alone cannot
    # distinguish Bed.app from bEd.app on a case-insensitive filesystem.
    legacy_app=''
    for candidate in /Applications/*.app; do
        case "${candidate##*/}" in
            bEd.app) previous_path="$candidate" ;;
            Bed.app) legacy_app="$candidate" ;;
        esac
    done
    if [ -z "$previous_path" ]; then
        previous_path="$legacy_app"
    fi
    if [ -n "$previous_path" ]; then
        mv "$previous_path" "$previous_app"
    fi
    if ! mv "$install_stage/bEd.app" /Applications/bEd.app; then
        if [ -e "$previous_app" ] || [ -L "$previous_app" ]; then
            if ! mv "$previous_app" "$previous_path"; then
                echo 'Installation failed and the previous app could not be restored.' >&2
                exit 1
            fi
        fi
        echo 'Installation failed; the previous app was restored if present.' >&2
        exit 1
    fi
    install_complete=true
    echo 'Installed /Applications/bEd.app'
}

if [ "$bed_install" = true ]; then
    install_bed_app "$dist/bEd.app"
fi
