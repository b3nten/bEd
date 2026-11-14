#!/usr/bin/env bash
# Build bEd.app and its ZIP; --install also puts it in /Applications.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: tools/package-macos.sh [--skip-build] [--install]

Builds the release `bed`, then makes target/dist/bEd.app and
target/dist/bEd-<version>-<arch>.zip. --install also installs it as
/Applications/bEd.app (the previous one is kept until the new one is in
place). --skip-build packages the existing target/release/bed.

Needs macOS, Rust, Xcode's command line tools and Python 3. BED_DIST_DIR
changes the output folder and BED_INSTALL_DIR the --install folder. Fonts,
icons, grammars and themes are embedded in the executable.
EOF
}
skip_build=false
install=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) skip_build=true ;;
        --install) install=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
test "$(uname -s)" = Darwin || { echo 'Package the macOS app on macOS.' >&2; exit 1; }
export COPYFILE_DISABLE=1
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(awk '$1 == "version" {gsub(/"/, "", $3); print $3; exit}' Cargo.toml)
icon=bed/assets/bEd-iOS-Default-1024@1x.png
binary=target/release/bed
test -f "$icon" || { echo "Missing app icon: $icon" >&2; exit 1; }
if [ "$skip_build" = false ]; then
    cargo build --locked --release -p bed --bin bed
fi
test -f "$binary" || { echo "Missing $binary: run without --skip-build." >&2; exit 1; }

dist=${BED_DIST_DIR:-target/dist}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/.package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/bEd.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/bed"

# Notices: bEd's license, then every dependency's and embedded asset's.
cp LICENSE "$app/Contents/Resources/"
python3 tools/collect-licenses.py "$app/Contents/Resources" \
    --target "$(rustc -vV | awk '$1 == "host:" {print $2}')"
test -s "$app/Contents/Resources/NOTICE"

iconset="$stage/bed.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips --resampleHeightWidth "$size" "$size" "$icon" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips --resampleHeightWidth $((size * 2)) $((size * 2)) "$icon" \
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
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Only the system's libraries: anything else ties the app to this machine.
while IFS= read -r library; do
    case "$library" in
        /System/*|/usr/lib/*) ;;
        *) echo "Unbundled library: $library" >&2; exit 1 ;;
    esac
done < <(otool -L "$app/Contents/MacOS/bed" | awk 'NR > 1 {print $1}')

codesign --force --sign - "$app/Contents/MacOS/bed"
codesign --force --sign - "$app"
codesign --verify --strict "$app"

zip="$dist/bEd-$version-$(uname -m).zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$stage/archive.zip"
mv -f "$stage/archive.zip" "$zip"
rm -rf "$dist/bEd.app"
mv "$app" "$dist/bEd.app"
rm -rf "$stage"
trap - EXIT
echo "$dist/bEd.app"
echo "$zip"

if [ "$install" = true ]; then
    destination=${BED_INSTALL_DIR:-/Applications}
    mkdir -p "$destination"
    incoming=$(mktemp -d "$destination/.bed-install.XXXXXX")
    previous="$incoming/Previous.app"
    trap '
        if [ -e "$previous" ]; then
            echo "The previous bEd is kept at $previous" >&2
        else
            rm -rf "$incoming"
        fi
    ' EXIT
    ditto "$dist/bEd.app" "$incoming/bEd.app"
    codesign --verify --strict "$incoming/bEd.app"
    if [ -e "$destination/bEd.app" ]; then
        mv "$destination/bEd.app" "$previous"
    fi
    if ! mv "$incoming/bEd.app" "$destination/bEd.app"; then
        if [ -e "$previous" ]; then
            mv "$previous" "$destination/bEd.app"
        fi
        echo 'Installing failed; the previous bEd is back.' >&2
        exit 1
    fi
    rm -rf "$incoming"
    echo "Installed $destination/bEd.app"
fi
