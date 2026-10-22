#!/usr/bin/env bash
# Bedtime 2 embeds its runtime assets; packages carry the executable and notices.
set -euo pipefail
export COPYFILE_DISABLE=1
BED2_PACKAGE_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BED2_PACKAGE_VERSION=$(awk '$1 == "version" {gsub(/"/, "", $3); print $3; exit}' "$BED2_PACKAGE_ROOT/Cargo.toml")
BED2_PACKAGE_ICON_SOURCE="$BED2_PACKAGE_ROOT/bed/assets/bEd-iOS-Default-1024@1x.png"

prepare_bed2_package() {
    test -f "$BED2_PACKAGE_ICON_SOURCE" || {
        printf 'Missing application icon: %s\n' "$BED2_PACKAGE_ICON_SOURCE" >&2
        return 1
    }
    if [ "$bed2_skip_build" = false ]; then
        cargo build --locked --release -p bed2 --bin bed2
    fi
    test -f "$binary" || {
        printf 'Missing desktop executable: %s. Run without --skip-build.\n' "$binary" >&2
        return 1
    }
}

copy_bed2_notices() {
    local destination=$1
    mkdir -p "$destination"
    cp "$BED2_PACKAGE_ROOT/LICENSE" "$destination/"
    local host_target
    host_target=$(rustc -vV | awk '$1 == "host:" {print $2}')
    python3 "$BED2_PACKAGE_ROOT/scripts/lib/collect-licenses.py" "$destination" --target "$host_target"
    # These workspace-owned embedded sources are omitted by the Cargo collector.
    python3 - "$BED2_PACKAGE_ROOT" "$destination" <<'PY'
import importlib.util
import sys
from pathlib import Path
repository, destination = map(Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("licenses", repository / "scripts/lib/collect-licenses.py")
licenses = importlib.util.module_from_spec(spec)
spec.loader.exec_module(licenses)
with (destination / "NOTICE").open("ab") as bundle:
    for root in ("bedtime2/crates/bedtime-ui2/assets", "bedtime2/crates/bedtime-grammars2/grammars",
                 "bedtime2/crates/bedtime-clipboard2"):
        for path in sorted(licenses.source_files(repository / root)):
            asset_notice = path.suffix.lower() in {".txt", ".md", ""} and any(
                word in path.name.upper() for word in ("LICENSE", "OFL"))
            if licenses.notice(path) or asset_notice or path.name in {"UPSTREAM.md", "sources.json"}:
                bundle.write(licenses.notice_section(path.relative_to(repository).as_posix(), path.read_bytes()))
    path = repository / "bedtime2/crates/bedtime-grammars2/sources.json"
    bundle.write(licenses.notice_section(path.relative_to(repository).as_posix(), path.read_bytes()))
PY
}

verify_bed2_notices() {
    test -s "$1/LICENSE"
    test -s "$1/NOTICE"
    test -s "$1/license-index.json"
}
