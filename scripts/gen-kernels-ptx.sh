#!/usr/bin/env bash
# Regenerate (or check) the committed PTX of the CUDA analysis kernel.
#
# The analyzer embeds hdr_analyzer_mvp/src/analysis/kernels.ptx and loads it through the
# driver's PTX JIT, so users need only the NVIDIA driver (ROADMAP E12). The PTX is NVRTC's
# output for kernels.cu, pinned to one NVRTC version and one option set below: another NVRTC
# version emits different float code (13.1 contracts more operations into FMAs than 12.6), and
# nvcc -ptx does not produce NVRTC's bytes. After any change to kernels.cu: run this script,
# then scripts/cuda-parity.sh, and commit both files.
#
# Usage:
#   scripts/gen-kernels-ptx.sh            # rewrite kernels.ptx
#   scripts/gen-kernels-ptx.sh --check    # exit 1 with a diff when kernels.ptx is stale
#
# NVRTC: $NVRTC_LIB (an absolute path to libnvrtc.so.12 of 12.6.77), else the CUDA 12.6
# toolkit's /usr/local/cuda-12.6/lib64/libnvrtc.so.12. CI installs the pinned pip wheel
# (scripts/ci/nvrtc-requirements.txt), whose library gives the same bytes as the toolkit's.
set -euo pipefail
cd "$(dirname "$0")/.."

NVRTC_VERSION="12.6"
NVRTC_BUILD="V12.6.77"
# compute_52 is NVRTC 12.6's default, written down; the PTX JIT-compiles on every later GPU.
OPTIONS=(--gpu-architecture=compute_52)
SRC=hdr_analyzer_mvp/src/analysis/kernels.cu
PTX=hdr_analyzer_mvp/src/analysis/kernels.ptx

check=0
case "${1:-}" in
    "") ;;
    --check) check=1 ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
esac

lib="${NVRTC_LIB:-/usr/local/cuda-12.6/lib64/libnvrtc.so.12}"
if [ ! -f "$lib" ]; then
    echo "NVRTC $NVRTC_VERSION not found at $lib; set NVRTC_LIB to an absolute path of libnvrtc.so.12 ($NVRTC_BUILD)" >&2
    exit 2
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 -I scripts/ci/nvrtc_ptx.py "$lib" "$NVRTC_VERSION" "$SRC" "$tmp/kernels.ptx" "${OPTIONS[@]}"
if ! grep -q "release $NVRTC_VERSION, $NVRTC_BUILD\$" "$tmp/kernels.ptx"; then
    echo "the PTX header does not name $NVRTC_BUILD:" >&2
    head -n 6 "$tmp/kernels.ptx" >&2
    exit 1
fi

if [ "$check" = 1 ]; then
    if cmp -s "$tmp/kernels.ptx" "$PTX"; then
        echo "$PTX is current (NVRTC $NVRTC_BUILD, ${OPTIONS[*]})"
        exit 0
    fi
    echo "::error::$PTX is stale: run scripts/gen-kernels-ptx.sh, then scripts/cuda-parity.sh, and commit both files" >&2
    diff -u "$PTX" "$tmp/kernels.ptx" | head -n 40 >&2 || true
    exit 1
fi
cp "$tmp/kernels.ptx" "$PTX"
echo "wrote $PTX ($(wc -c < "$PTX") bytes, NVRTC $NVRTC_BUILD, ${OPTIONS[*]})"
