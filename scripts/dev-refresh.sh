#!/usr/bin/env bash
# Rebuild the release binaries for local development use.
#
# Intended setup: ~/.local/bin/{mkvdovi,hdr_analyzer_mvp,verifier} are symlinks
# into this repo's target/release/, so a rebuild is all it takes for the
# system-wide commands to run the latest code:
#   ln -sf "$PWD"/target/release/{mkvdovi,hdr_analyzer_mvp,verifier} ~/.local/bin/
#
# Build order matters: a plain workspace release build produces an analyzer
# WITHOUT the CUDA backend, so the CUDA-featured analyzer must be built last
# or it gets overwritten and auto mode silently falls back to CPU analysis.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --workspace

if command -v nvidia-smi >/dev/null 2>&1 || [ -x /usr/lib/wsl/lib/nvidia-smi ]; then
    echo "NVIDIA GPU tooling detected: rebuilding hdr_analyzer_mvp with the CUDA backend..."
    cargo build --release -p hdr_analyzer_mvp --features cuda
fi

echo
echo "Binaries refreshed:"
for bin in mkvdovi hdr_analyzer_mvp verifier; do
    printf '  %-18s %s\n' "$bin" "$("target/release/$bin" --version)"
done
