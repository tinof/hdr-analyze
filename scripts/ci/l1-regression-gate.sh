#!/usr/bin/env bash
# L1 regression gate: analyze the synthetic corpus clip (PQ and HLG) and score the result
# against the committed references in tools/l1_diff/corpus with tools/l1_diff.
#
#   scripts/ci/l1-regression-gate.sh            check, exit 1 on any breach
#   scripts/ci/l1-regression-gate.sh --update   rewrite the references from this build
#
# The references are the analyzer's own output, so this detects change, not error against
# ground truth (hdr_analyzer_mvp/tests/synthetic_accuracy.rs covers that). When a change is
# meant to move L1, run --update and review the reference diff in the pull request.
#
# ANALYZER and L1_DIFF select prebuilt binaries; by default both are built in the dev profile.
set -euo pipefail

cd "$(dirname "$0")/../.."
corpus=tools/l1_diff/corpus

update=0
case "${1:-}" in
    "") ;;
    --update) update=1 ;;
    *) echo "usage: $0 [--update]" >&2; exit 2 ;;
esac

# A missing tool must fail the gate, never skip it.
for tool in ffmpeg python3 jq; do
    command -v "$tool" >/dev/null || { echo "error: $tool not found" >&2; exit 2; }
done
# Not a pipe into `grep -q`: it closes the pipe early and pipefail reports ffmpeg's SIGPIPE.
encoders=$(ffmpeg -hide_banner -encoders 2>/dev/null)
grep -qw ffv1 <<< "$encoders" || { echo "error: ffmpeg has no ffv1 encoder" >&2; exit 2; }

if [[ -z "${ANALYZER:-}" ]]; then
    cargo build -p hdr_analyzer_mvp
    ANALYZER=target/debug/hdr_analyzer_mvp
fi
if [[ -z "${L1_DIFF:-}" ]]; then
    cargo build --manifest-path tools/l1_diff/Cargo.toml
    L1_DIFF=tools/l1_diff/target/debug/l1_diff
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Limits in 12-bit PQ codes. An unchanged analyzer scores 0. The error limits leave room for
# one rounding flip of a whole-code value on another CPU; the bias limits catch a shift of
# one code over a single shot (24 of 144 frames: bias 0.17).
limits=(
    --max-peak-bias 0.1 --max-peak-error 0.5
    --max-min-bias 0.1 --max-min-error 1
    --max-avg-bias 0.1 --max-avg-error 1
    --max-scene-mismatches 0
)

status=0
for transfer in pq hlg; do
    case $transfer in
        pq) trc=smpte2084 ;;
        hlg) trc=arib-std-b67 ;;
    esac
    clip=$work/$transfer.mkv
    bin=$work/$transfer.bin
    python3 "$corpus/make_corpus.py" | ffmpeg -hide_banner -loglevel error -y \
        -f rawvideo -pix_fmt yuv420p10le -video_size 320x180 -framerate 24 -i - \
        -c:v ffv1 -color_primaries bt2020 -color_trc "$trc" -colorspace bt2020nc \
        -color_range tv "$clip"

    "$ANALYZER" "$clip" -o "$bin" --transfer "$transfer" --downscale 1 --disable-optimizer \
        > "$work/$transfer.log" 2>&1 \
        || { cat "$work/$transfer.log" >&2; echo "error: analyzer failed on $transfer" >&2; exit 2; }

    if (( update )); then
        "$L1_DIFF" --ours "$bin" --export-reference "$corpus/$transfer.reference.csv"
        jq -r '.scenes[].start' "$bin.l1.json" > "$corpus/$transfer.scenes.txt"
        continue
    fi

    echo "::group::l1_diff $transfer"
    "$L1_DIFF" --ours "$bin" --reference "$corpus/$transfer.reference.csv" \
        --scenes "$corpus/$transfer.scenes.txt" "${limits[@]}" || status=1
    echo "::endgroup::"
done

if (( update )); then
    echo "References rewritten. Review: git diff -- $corpus"
elif (( status )); then
    echo "L1 regression gate FAILED. If the change is intended, run $0 --update and commit the references." >&2
else
    echo "L1 regression gate passed."
fi
exit "$status"
