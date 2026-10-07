#!/usr/bin/env bash
# Build the desktop and its precompiled SSH helpers, package, and install bEd.
set -euo pipefail
cd "$(dirname "$0")/.."

install=true
case "${1:-}" in
    --no-install) install=false ;;
    --help|-h)
        echo 'Usage: scripts/build-macos.sh [--no-install]'
        echo 'Requires Rust, Xcode Command Line Tools, Python 3, and Docker.'
        exit 0 ;;
    '') ;;
    *) echo 'Usage: scripts/build-macos.sh [--no-install]' >&2; exit 2 ;;
esac
if [ "$#" -gt 1 ]; then
    echo 'Usage: scripts/build-macos.sh [--no-install]' >&2
    exit 2
fi
if [ "$(uname -s)" != Darwin ]; then
    echo 'Build the macOS application on macOS.' >&2
    exit 1
fi

bash scripts/build-remote-helpers.sh
cargo build --locked --release --bin bed
bash scripts/pack-mac.sh
if [ "$install" = true ]; then
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
    ditto "${BED_DIST_DIR:-target/dist}/bEd.app" "$install_stage/bEd.app"
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
fi
