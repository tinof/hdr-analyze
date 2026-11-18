#!/usr/bin/env bash
# CPU vs CUDA parity gate for the analyzer. Run it on the GPU host before any PR that touches
# hdr_analyzer_mvp/src/analysis/, kernels.cu or ffmpeg_io.rs.
#
# Encodes the synthetic L1 corpus (tools/l1_diff/corpus/make_corpus.py, 640x360, 242 frames) as
# HEVC Main10 twice, tagged PQ and HLG, then runs hdr_analyzer_mvp/tests/cuda_parity.rs against a
# debug build with the cuda feature. The test compares --hwaccel none with --hwaccel cuda:
# byte-identical .bin, equal crop/scenes/frames in the L1 sidecar, and analysis.gpu == true for
# the CUDA run. The HLG clip is checked once per Profile 8.4 composer (--hlg-composer preset and
# bt2100, each with its sidecar luminance_mapping), and the PQ clip also with
# --hlg-composer bt2100, which must leave PQ output unchanged. It fails closed: a missing tool,
# a missing GPU or a CPU fallback is an error. MKV clips also get --au-table, and the CPU and
# CUDA AU tables must be equal apart from the sidecar digest (P15.1). Every run also writes
# --dump-pq-hist, and the per-frame 4096-bin PQ histograms must be byte-identical (WS7.1), in
# this pass and in the NVDEC pass below. Before the clips it runs the in-crate GPU test of
# --peak-estimator st2094-10 on crops crop detection never produces (odd anchor and size, planar
# and P010, consecutive frames; ROADMAP WS1.5), which also compares the 4096 histogram bins.
#
# A second pass builds the analyzer without the cuda feature and runs
# nvdec_cpu_analysis_matches_software_decode (ROADMAP E12.2): NVDEC decode with CPU analysis
# against software decode, byte-identical .bin and sidecar, on both clips, both composers,
# median3, st2094-10, --no-crop and a letterboxed PQ clip with --crop-probes 0. The first pass
# runs the same test on the +cuda build for --pre-denoise median3, its one CPU analysis of NVDEC
# frames. The letterboxed clip is always generated, also with --pq / --hlg.
#
# Usage:
#   scripts/cuda-parity.sh [--pq <file>] [--hlg <file>]
# --pq / --hlg replace the generated clip for that transfer with a real sample (HEVC 10-bit).
set -euo pipefail
cd "$(dirname "$0")/.."

PQ_CLIP=""
HLG_CLIP=""
while [ $# -gt 0 ]; do
    case "$1" in
        --pq) PQ_CLIP="${2:?--pq needs a file}"; shift 2 ;;
        --hlg) HLG_CLIP="${2:?--hlg needs a file}"; shift 2 ;;
        -h|--help) echo "usage: $0 [--pq <file>] [--hlg <file>]"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
for clip in "$PQ_CLIP" "$HLG_CLIP"; do
    [ -z "$clip" ] || [ -f "$clip" ] || { echo "clip not found: $clip" >&2; exit 2; }
done

if ! command -v nvidia-smi >/dev/null 2>&1 && [ ! -x /usr/lib/wsl/lib/nvidia-smi ]; then
    echo "nvidia-smi not found: the CUDA parity gate needs an NVIDIA GPU host" >&2
    exit 2
fi
for tool in ffmpeg ffprobe python3 cargo; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 2; }
done
# Captured first: with pipefail, `ffmpeg | grep -q` fails when grep closes the pipe early.
ENCODERS="$(ffmpeg -hide_banner -encoders 2>/dev/null)"
grep -q libx265 <<<"$ENCODERS" || { echo "ffmpeg has no libx265 encoder" >&2; exit 2; }

CORPUS=tools/l1_diff/corpus/make_corpus.py
WIDTH=640
HEIGHT=360
LETTERBOX_HEIGHT=264

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Encode the corpus with the given transfer (x265 name, FFmpeg name). The tags go into the HEVC
# VUI through -x265-params: the analyzer's FFmpeg can drop tags held only in the MKV container.
# Main10 yuv420p10le, so NVDEC produces P010 surfaces and frames are analyzed in place.
# With a fourth argument `letterbox`, the corpus is drawn LETTERBOX_HEIGHT high and padded with
# black bars to the full height; the input is tagged too, so the pad filter converts nothing.
make_clip() {
    local x265_transfer="$1" ffmpeg_transfer="$2" out="$3" probe
    local height="$HEIGHT" input_tags=() filter=()
    if [ "${4:-}" = letterbox ]; then
        height="$LETTERBOX_HEIGHT"
        input_tags=(-color_primaries bt2020 -color_trc "$ffmpeg_transfer" -colorspace bt2020nc -color_range tv)
        filter=(-vf "pad=${WIDTH}:${HEIGHT}:0:$(( (HEIGHT - LETTERBOX_HEIGHT) / 2 )):black")
    fi
    [ -f "$CORPUS" ] || { echo "corpus generator not found: $CORPUS" >&2; exit 2; }
    python3 "$CORPUS" --width "$WIDTH" --height "$height" \
        | ffmpeg -hide_banner -loglevel error -y \
            -f rawvideo -pix_fmt yuv420p10le -video_size "${WIDTH}x${height}" -framerate 24 \
            ${input_tags[@]+"${input_tags[@]}"} -i - ${filter[@]+"${filter[@]}"} \
            -c:v libx265 -preset fast -crf 14 -profile:v main10 -pix_fmt yuv420p10le \
            -x265-params "colorprim=bt2020:transfer=${x265_transfer}:colormatrix=bt2020nc:range=limited:log-level=error" \
            -color_primaries bt2020 -color_trc "$ffmpeg_transfer" -colorspace bt2020nc -color_range tv \
            "$out"
    # Check the elementary stream, not the container: copy to raw Annex B and probe that.
    ffmpeg -hide_banner -loglevel error -y -i "$out" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$out.hevc"
    probe="$(ffprobe -v error -select_streams v:0 -count_frames \
        -show_entries stream=profile,pix_fmt,color_transfer,nb_read_frames \
        -of default=noprint_wrappers=1 "$out.hevc")"
    rm -f "$out.hevc"
    for expected in "profile=Main 10" "pix_fmt=yuv420p10le" "color_transfer=${ffmpeg_transfer}" "nb_read_frames=242"; do
        grep -qxF "$expected" <<<"$probe" \
            || { echo "generated clip $out is not as expected (want $expected):" >&2; echo "$probe" >&2; exit 1; }
    done
}

if [ -z "$PQ_CLIP" ]; then
    PQ_CLIP="$WORK/parity_pq.mkv"
    make_clip smpte2084 smpte2084 "$PQ_CLIP"
fi
if [ -z "$HLG_CLIP" ]; then
    HLG_CLIP="$WORK/parity_hlg.mkv"
    make_clip arib-std-b67 arib-std-b67 "$HLG_CLIP"
fi
LETTERBOX_CLIP="$WORK/parity_letterbox_pq.mkv"
make_clip smpte2084 smpte2084 "$LETTERBOX_CLIP" letterbox
# The test runs in the crate directory, so pass absolute paths.
PQ_CLIP="$(realpath "$PQ_CLIP")"
HLG_CLIP="$(realpath "$HLG_CLIP")"
echo "PQ clip:  $PQ_CLIP"
echo "HLG clip: $HLG_CLIP"
echo "Letterboxed PQ clip: $LETTERBOX_CLIP"

# Debug profile on purpose: target/release holds the installed binaries.
# libtest exits 0 when the filter matches nothing, so require exactly one passed test.
RS_LOG="$WORK/reduced_set_odd_crops.log"
HDR_ANALYZE_CUDA_PARITY_REQUIRED=1 \
    cargo test -p hdr_analyzer_mvp --features cuda --bin hdr_analyzer_mvp \
    analysis::gpu::tests::reduced_set_on_the_gpu_matches_the_cpu_on_odd_crops -- --exact --nocapture \
    > "$RS_LOG" 2>&1 || { command cat "$RS_LOG"; exit 1; }
command cat "$RS_LOG"
if ! grep -q 'test result: ok\. 1 passed' "$RS_LOG"; then
    echo "CUDA parity: the odd-crop reduced-set test did not run exactly once" >&2
    exit 1
fi
export HDR_ANALYZE_CUDA_PARITY_REQUIRED=1
export HDR_ANALYZE_CUDA_PARITY_PQ="$PQ_CLIP"
export HDR_ANALYZE_CUDA_PARITY_HLG="$HLG_CLIP"
export HDR_ANALYZE_CUDA_PARITY_LETTERBOX="$LETTERBOX_CLIP"
export HDR_ANALYZE_CUDA_PARITY_HLG_COMPOSERS=preset,bt2100
# One test at a time: each runs NVDEC and CUDA processes.
CUDA_LOG="$WORK/cuda_parity.log"
cargo test -p hdr_analyzer_mvp --features cuda --test cuda_parity -- --test-threads=1 --nocapture \
    > "$CUDA_LOG" 2>&1 || { command cat "$CUDA_LOG"; exit 1; }
command cat "$CUDA_LOG"
if ! grep -q 'test result: ok\. 2 passed' "$CUDA_LOG"; then
    echo "CUDA parity: the +cuda pass did not run both tests" >&2
    exit 1
fi

# The same NVDEC decode with CPU analysis from a build without the cuda feature (ROADMAP E12.2).
NVDEC_LOG="$WORK/nvdec_cpu_analysis.log"
cargo test -p hdr_analyzer_mvp --test cuda_parity -- \
    --exact nvdec_cpu_analysis_matches_software_decode --nocapture \
    > "$NVDEC_LOG" 2>&1 || { command cat "$NVDEC_LOG"; exit 1; }
command cat "$NVDEC_LOG"
if ! grep -q 'test result: ok\. 1 passed' "$NVDEC_LOG"; then
    echo "CUDA parity: the NVDEC CPU-analysis test without the cuda feature did not run exactly once" >&2
    exit 1
fi

echo "CUDA parity: PASS"
