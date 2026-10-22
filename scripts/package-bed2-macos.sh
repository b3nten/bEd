#!/usr/bin/env bash
# Build and package bEd2.app, optionally install it into /Applications.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: scripts/package-bed2-macos.sh [--skip-build] [--install]

Build the release bed2 application, then produce target/dist/bed2/bEd2.app and
its ZIP archive. --install also installs /Applications/bEd2.app alongside bed1.
--skip-build packages an existing executable.

Requires macOS, Rust, Xcode Command Line Tools and Python 3. BED2_BINARY and
BED2_DIST_DIR override the executable and output directory. BED2_INSTALL_DIR
changes the --install destination (default: /Applications). Runtime fonts, icons
and grammars are embedded; no Docker or SSH helper builds are needed.
EOF
}
bed2_skip_build=false
bed2_install=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) bed2_skip_build=true ;;
        --install) bed2_install=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
source "$(dirname "$0")/lib/package-bed2-common.sh"
cd "$BED2_PACKAGE_ROOT"
test "$(uname -s)" = Darwin || { echo 'Package the macOS application on macOS.' >&2; exit 1; }
binary=${BED2_BINARY:-target/release/bed2}
prepare_bed2_package
dist=${BED2_DIST_DIR:-target/dist/bed2}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/bed-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/bEd2.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/bed2"
copy_bed2_notices "$app/Contents/Resources"
iconset="$stage/bed.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips --resampleHeightWidth "$size" "$size" "$BED2_PACKAGE_ICON_SOURCE" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    retina_size=$((size * 2))
    sips --resampleHeightWidth "$retina_size" "$retina_size" "$BED2_PACKAGE_ICON_SOURCE" \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/bed.icns" "$iconset"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>bed2</string>
<key>CFBundleIconFile</key><string>bed.icns</string>
<key>CFBundleIdentifier</key><string>org.bed-editor.bed2</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundleName</key><string>bEd2</string>
<key>CFBundleDisplayName</key><string>bEd2</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$BED2_PACKAGE_VERSION</string>
<key>CFBundleVersion</key><string>$BED2_PACKAGE_VERSION</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
plutil -lint "$app/Contents/Info.plist"
# Reject external libraries that would make the application depend on this machine.
while IFS= read -r dependency; do
    case "$dependency" in /System/*|/usr/lib/*) ;; *) echo "Unbundled dependency: $dependency" >&2; exit 1 ;; esac
done < <(otool -L "$app/Contents/MacOS/bed2" | awk 'NR > 1 {print $1}')
verify_bed2_notices "$app/Contents/Resources"
codesign --force --sign - "$app/Contents/MacOS/bed2"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
output="$dist/bEd2-$BED2_PACKAGE_VERSION-$(uname -m).zip"
# Finish the replacement before removing the previously published archive.
staged_output="$stage/${output##*/}"
ditto -c -k --sequesterRsrc --keepParent "$app" "$staged_output"
rm -f "$output"
mv -f "$staged_output" "$output"
rm -rf "$dist/bEd2.app"
mv "$app" "$dist/bEd2.app"
echo "$output"
rm -rf "$stage"
trap - EXIT
install_bed2_app() {
    install_dir=${BED2_INSTALL_DIR:-/Applications}
    mkdir -p "$install_dir"
    install_stage=$(mktemp -d "$install_dir/.bed2-install.XXXXXX")
    previous_app="$install_stage/Previous.app"
    install_complete=false
    cleanup_install() {
        if [ "$install_complete" = true ] || { [ ! -e "$previous_app" ] && [ ! -L "$previous_app" ]; }; then
            rm -rf "$install_stage"
        else
            printf 'Previous application retained at %s\n' "$previous_app" >&2
        fi
    }
    trap cleanup_install EXIT
    ditto "$1" "$install_stage/bEd2.app"
    codesign --verify --strict "$install_stage/bEd2.app"
    if [ -e "$install_dir/bEd2.app" ] || [ -L "$install_dir/bEd2.app" ]; then
        mv "$install_dir/bEd2.app" "$previous_app"
    fi
    if ! mv "$install_stage/bEd2.app" "$install_dir/bEd2.app"; then
        if [ -e "$previous_app" ] || [ -L "$previous_app" ]; then
            if ! mv "$previous_app" "$install_dir/bEd2.app"; then
                echo 'Installation failed and the previous app could not be restored.' >&2
                exit 1
            fi
        fi
        echo 'Installation failed; the previous app was restored if present.' >&2
        exit 1
    fi
    install_complete=true
    printf 'Installed %s/bEd2.app\n' "$install_dir"
}

if [ "$bed2_install" = true ]; then
    install_bed2_app "$dist/bEd2.app"
fi
