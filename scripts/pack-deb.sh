#!/usr/bin/env bash
# Bed Linux packaging translated from pinned ned scripts/pack-deb.sh.
set -euo pipefail
source "$(dirname "$0")/package-common.sh"
cd "$BED_PACKAGE_ROOT"
test "$(uname -s)" = Linux
binary=${BED_BINARY:-target/release/bed}
test -f "$binary" || { echo 'Build with cargo build --locked --release --bin bed first.' >&2; exit 1; }
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
Name=Bed
Comment=Rust desktop text editor
Exec=bed %F
Icon=bed
Terminal=false
Categories=Development;TextEditor;
MimeType=text/plain;
EOF
mkdir -p "$stage/usr/share/icons/hicolor/256x256/apps"
cp resources/icons/bed.png "$stage/usr/share/icons/hicolor/256x256/apps/bed.png"
arch=$(uname -m)
tar -czf "$dist/Bed-$BED_PACKAGE_VERSION-$arch-linux.tar.gz" -C "$stage" usr
if command -v dpkg-deb >/dev/null 2>&1; then
    arch=$(dpkg --print-architecture)
    mkdir -p "$stage/DEBIAN"
    cat > "$stage/DEBIAN/control" <<EOF
Package: bed
Version: $BED_PACKAGE_VERSION
Architecture: $arch
Maintainer: Bed contributors
Section: editors
Priority: optional
Depends: libc6, libgcc-s1, libstdc++6, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libwayland-client0, libvulkan1
Description: Bed Text Editor with embeddable document views
 Desktop text editor with syntax highlighting, Git, language servers,
 terminals and SSH workspaces. See the bundled PORTING.md for details.
EOF
    dpkg-deb --root-owner-group --build "$stage" "$dist/bed_${BED_PACKAGE_VERSION}_${arch}.deb"
    dpkg-deb --info "$dist/bed_${BED_PACKAGE_VERSION}_${arch}.deb"
fi
echo "$dist"
