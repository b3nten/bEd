#!/usr/bin/env bash
# Bed.app packaging translated from the pinned ned scripts/pack-mac.sh.
set -euo pipefail
source "$(dirname "$0")/package-common.sh"
cd "$BED_PACKAGE_ROOT"
test "$(uname -s)" = Darwin
binary=${BED_BINARY:-target/release/bed}
test -f "$binary" || { echo 'Build with cargo build --locked --release --bin bed first.' >&2; exit 1; }
dist=${BED_DIST_DIR:-target/dist}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/bed-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/Bed.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/bed"
copy_bed_resources "$app/Contents/Resources"
cp resources/icons/bed.icns "$app/Contents/Resources/bed.icns"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>bed</string>
<key>CFBundleIconFile</key><string>bed.icns</string>
<key>CFBundleIdentifier</key><string>org.bed-editor.bed</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundleName</key><string>Bed</string>
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
output="$dist/Bed-$BED_PACKAGE_VERSION-$(uname -m).zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$output"
rm -rf "$dist/Bed.app"
mv "$app" "$dist/Bed.app"
echo "$output"
