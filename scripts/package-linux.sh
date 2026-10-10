#!/usr/bin/env bash
# Build and package bEd as a Linux archive and, when available, a Debian package.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: scripts/package-linux.sh [--skip-build]

Build the release application and missing static SSH helpers, then produce a
target/dist/bEd-*-linux.tar.gz archive and a .deb when dpkg-deb is available.
--skip-build packages existing binaries and validated helper bundles (used in CI).

Requires Linux, Rust, the desktop development libraries, Python 3 and ImageMagick
(magick or convert). Building missing SSH helpers also requires Docker.
BED_BINARY, BED_DIST_DIR and BED_HELPERS_DIR override the executable, output and
prebuilt helper directories.
EOF
}
bed_skip_build=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) bed_skip_build=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
source "$(dirname "$0")/lib/package-common.sh"
cd "$BED_PACKAGE_ROOT"
test "$(uname -s)" = Linux || { echo 'Package the Linux application on Linux.' >&2; exit 1; }
if command -v magick >/dev/null 2>&1; then
    bed_image_resizer=magick
elif command -v convert >/dev/null 2>&1; then
    bed_image_resizer=convert
else
    echo 'ImageMagick is required to generate Linux icons (install imagemagick).' >&2
    exit 1
fi
binary=${BED_BINARY:-target/release/bed}
prepare_bed_package
dist=${BED_DIST_DIR:-target/dist}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/bed-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/usr/bin" "$stage/usr/share/Bed" "$stage/usr/share/applications"
install -m755 "$binary" "$stage/usr/bin/bed"
copy_bed_resources "$stage/usr/share/Bed"
verify_bed_resources "$stage/usr/share/Bed"
cat > "$stage/usr/share/applications/bed.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=bEd
Comment=Rust desktop text editor
Exec=bed %F
Icon=bed
Terminal=false
Categories=Development;TextEditor;
MimeType=text/plain;
EOF
for size in 16 24 32 48 64 128 256 512 1024; do
    icon_directory="$stage/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$icon_directory"
    "$bed_image_resizer" "$BED_PACKAGE_ICON_SOURCE" -resize "${size}x${size}" "$icon_directory/bed.png"
done
arch=$(uname -m)
archive="bEd-$BED_PACKAGE_VERSION-$arch-linux.tar.gz"
tar -czf "$stage/$archive" -C "$stage" usr
mv -f "$stage/$archive" "$dist/$archive"
if command -v dpkg-deb >/dev/null 2>&1; then
    arch=$(dpkg --print-architecture)
    mkdir -p "$stage/DEBIAN"
    cat > "$stage/DEBIAN/control" <<EOF
Package: bed
Version: $BED_PACKAGE_VERSION
Architecture: $arch
Maintainer: bEd contributors
Section: editors
Priority: optional
Depends: libc6, libgcc-s1, libstdc++6, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libwayland-client0, libvulkan1, libasound2t64 | libasound2
Description: bEd desktop editor with embeddable document views
 Desktop text editor with syntax highlighting, Git, language servers,
 terminals and SSH workspaces.
EOF
    dpkg-deb --root-owner-group --build "$stage" "$dist/bed_${BED_PACKAGE_VERSION}_${arch}.deb"
    dpkg-deb --info "$dist/bed_${BED_PACKAGE_VERSION}_${arch}.deb"
fi
echo "$dist"
