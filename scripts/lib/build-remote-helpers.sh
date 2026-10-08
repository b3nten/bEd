#!/usr/bin/env bash
# Build the bundled Linux helpers locally with the official Rust/musl C toolchain.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/lib/build-remote-helpers.sh [RUST_TARGET ...]

Build and stage both Linux helpers by default. To repair one bundle, select:
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl

Requires a running Docker engine (OrbStack or Docker Desktop), host Python 3,
and Cargo for license collection. Compilation uses ordinary GCC in the local
container; connecting to an SSH workspace never installs or runs a compiler.

BED_HELPER_BUILD_IMAGE overrides the official rust:1.99.0-alpine3.23 image.
An override must provide Cargo, native musl Rust targets, and GCC for each
selected container platform. Docker caches persist separately per architecture.
EOF
}

if [[ $# -eq 0 ]]; then
    targets=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl)
else
    targets=("$@")
fi
for target in "${targets[@]}"; do
    case "$target" in
        x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unsupported helper target: %s\n' "$target" >&2; usage >&2; exit 2 ;;
    esac
done

if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker is required. Install OrbStack or Docker Desktop, start it, and retry.\n' >&2
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    printf 'Cannot reach the Docker engine. Start OrbStack or Docker Desktop and retry.\n' >&2
    exit 1
fi
for program in python3 cargo; do
    if ! command -v "$program" >/dev/null 2>&1; then
        printf '%s is required on the host to stage helper bundles and license notices.\n' "$program" >&2
        exit 1
    fi
done

bed_root=$(cd "$(dirname "$0")/../.." && pwd)
image=${BED_HELPER_BUILD_IMAGE:-rust:1.99.0-alpine3.23}
host_uid=$(id -u)
host_gid=$(id -g)
validation_args=(validate)

for target in "${targets[@]}"; do
    # Fixed mappings also work with macOS's system Bash 3.2.
    case "$target" in
        x86_64-unknown-linux-musl)
            architecture=amd64
            cc_key=CC_x86_64_unknown_linux_musl
            linker_key=CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER
            ;;
        aarch64-unknown-linux-musl)
            architecture=arm64
            cc_key=CC_aarch64_unknown_linux_musl
            linker_key=CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER
            ;;
    esac
    output="$bed_root/target/remote-helper-builds/$target"
    mkdir -p "$output"
    printf 'Building %s using %s on linux/%s\n' "$target" "$image" "$architecture"
    # Keep Linux build products and registry caches away from macOS Cargo state.
    # Only the exported executable is written to the small host output mount.
    docker run --rm --platform "linux/$architecture" --user 0:0 \
        --mount "type=bind,source=$bed_root,target=/workspace,readonly" \
        --mount "type=volume,source=bed-helper-target-$architecture,target=/target" \
        --mount "type=volume,source=bed-helper-registry-$architecture,target=/usr/local/cargo/registry" \
        --mount "type=bind,source=$output,target=/out" \
        --workdir /workspace \
        --env CARGO_TARGET_DIR=/target \
        --env LIBZ_SYS_STATIC=1 \
        --env "$cc_key=gcc" \
        --env "$linker_key=gcc" \
        --env "BED_BUILD_TRIPLE=$target" \
        --env "BED_BUILD_UID=$host_uid" \
        --env "BED_BUILD_GID=$host_gid" \
        "$image" sh -eu -c '
            cargo build --locked --release --target "$BED_BUILD_TRIPLE" -p bed-headless
            binary="/target/$BED_BUILD_TRIPLE/release/bed-headless"
            printf "Helper protocol version: "
            "$binary" --protocol-version
            cp "$binary" /out/bed-headless
            chmod 0755 /out/bed-headless
            chown "$BED_BUILD_UID:$BED_BUILD_GID" /out/bed-headless
        '
    python3 "$bed_root/scripts/lib/remote-helpers.py" stage \
        --target "$target" --binary "$output/bed-headless"
    validation_args+=(--target "$target")
done

python3 "$bed_root/scripts/lib/remote-helpers.py" "${validation_args[@]}"
