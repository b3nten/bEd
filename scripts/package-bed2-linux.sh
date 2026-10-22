#!/usr/bin/env bash
# Build and package bEd2 as a Linux archive and, when available, a Debian package.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: scripts/package-bed2-linux.sh [--skip-build]

Build the release bed2 application, then produce a target/dist/bed2/bEd2-*-linux.tar.gz
archive and a .deb when dpkg-deb is available. --skip-build packages an existing
executable. Install the archive's usr/ tree under /usr/local, or install the Debian package.

Requires Linux, Rust, the desktop development libraries, Python 3 and ImageMagick
(magick or convert). BED2_BINARY and BED2_DIST_DIR override the executable and
output directory. Runtime assets are embedded; no SSH helpers or Docker required.
EOF
}
bed2_skip_build=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) bed2_skip_build=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done
source "$(dirname "$0")/lib/package-bed2-common.sh"
cd "$BED2_PACKAGE_ROOT"
test "$(uname -s)" = Linux || { echo 'Package the Linux application on Linux.' >&2; exit 1; }
if command -v magick >/dev/null 2>&1; then
    bed_image_resizer=magick
elif command -v convert >/dev/null 2>&1; then
    bed_image_resizer=convert
else
    echo 'ImageMagick is required to generate Linux icons (install imagemagick).' >&2
    exit 1
fi
binary=${BED2_BINARY:-target/release/bed2}
prepare_bed2_package
dist=${BED2_DIST_DIR:-target/dist/bed2}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
stage=$(mktemp -d "$dist/bed-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/usr/bin" "$stage/usr/share/Bed2" "$stage/usr/share/applications"
install -m755 "$binary" "$stage/usr/bin/bed2"
copy_bed2_notices "$stage/usr/share/Bed2"
verify_bed2_notices "$stage/usr/share/Bed2"
cat > "$stage/usr/share/applications/bed2.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=bEd2
Comment=Rust desktop text editor
Exec=bed2 %F
Icon=bed2
Terminal=false
Categories=Development;TextEditor;
MimeType=text/plain;
EOF
for size in 16 24 32 48 64 128 256 512 1024; do
    icon_directory="$stage/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$icon_directory"
    "$bed_image_resizer" "$BED2_PACKAGE_ICON_SOURCE" -resize "${size}x${size}" "$icon_directory/bed2.png"
done
arch=$(uname -m)
archive="bEd2-$BED2_PACKAGE_VERSION-$arch-linux.tar.gz"
tar -czf "$stage/$archive" -C "$stage" usr
mv -f "$stage/$archive" "$dist/$archive"
if command -v dpkg-deb >/dev/null 2>&1; then
    arch=$(dpkg --print-architecture)
    mkdir -p "$stage/DEBIAN"
    cat > "$stage/DEBIAN/control" <<EOF
Package: bed2
Version: $BED2_PACKAGE_VERSION
Architecture: $arch
Maintainer: bEd2 contributors
Section: editors
Priority: optional
Depends: libc6, libgcc-s1, libstdc++6, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libwayland-client0, libvulkan1, libasound2t64 | libasound2
Description: bEd2 desktop code editor
 Text editor with syntax highlighting, folding, code Structure,
 a file tree and local terminals.
EOF
    dpkg-deb --root-owner-group --build "$stage" "$dist/bed2_${BED2_PACKAGE_VERSION}_${arch}.deb"
    dpkg-deb --info "$dist/bed2_${BED2_PACKAGE_VERSION}_${arch}.deb"
fi
echo "$dist"
