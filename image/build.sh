#!/bin/bash
# Build the LinuxAI ISO from this repo. On Arch, runs directly (as root); elsewhere uses Docker.
#   ./image/build.sh            → out/linuxai-<version>-x86_64.iso
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$ROOT/out"
if [ -f /etc/arch-release ]; then
    SRC="$ROOT" OUT="$ROOT/out" WORK="${WORK:-/tmp/linuxai-work}" exec sudo -E bash "$ROOT/image/build-in-arch.sh"
fi
CA_ARGS=()
if [ -n "${EXTRA_CA:-}" ]; then CA_ARGS=(-v "$EXTRA_CA:/extra-ca.crt:ro" -e EXTRA_CA=/extra-ca.crt); fi
docker run --rm --privileged "${CA_ARGS[@]}" -v "$ROOT:/src:ro" -v "$ROOT/out:/out" \
    archlinux:latest bash /src/image/build-in-arch.sh
