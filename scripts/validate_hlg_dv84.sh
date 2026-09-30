#!/usr/bin/env bash
# Check the analyzer's HLG -> PQ mapping against a Dolby Vision reference renderer.
#
# The analyzer measures HLG through the Profile 8.4 luma reshaping curve (analysis/hlg.rs).
# This script builds a lossless flat-grey HLG ramp (one 10-bit luma code per frame, 64..1008),
# injects a Profile 8.4 RPU with dovi_tool, renders it through ffmpeg's libplacebo filter with
# Dolby Vision applied, and compares each frame's reconstructed PQ luma (BT.2020 weights over the
# rendered R'G'B') with the per-frame minimum the analyzer writes to its L1 sidecar. The minimum
# is used because it is not temporally smoothed; on a flat frame it equals the mapped luma.
#
# Needs: ffmpeg with libx265, libplacebo and a working Vulkan device; dovi_tool; python3; a built
# analyzer. Usage:
#   scripts/validate_hlg_dv84.sh [path/to/hdr_analyzer_mvp] [--hwaccel cuda]
# Exit status is non-zero when any frame differs by more than TOLERANCE 12-bit PQ codes.
set -euo pipefail
cd "$(dirname "$0")/.."

ANALYZER="${1:-target/release/hdr_analyzer_mvp}"
HWACCEL_ARGS=()
[ "${2:-}" = "--hwaccel" ] && HWACCEL_ARGS=(--hwaccel "${3:?--hwaccel needs a value}")
TOLERANCE="${TOLERANCE:-4}"

for tool in ffmpeg dovi_tool python3; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
[ -x "$ANALYZER" ] || { echo "analyzer not found: $ANALYZER (build it first)" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FRAMES=60
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "color=black:s=64x64:r=24,format=yuv420p10le,geq=lum='min(64+N*16\,1023)':cb=512:cr=512" \
    -frames:v "$FRAMES" -c:v libx265 \
    -x265-params "lossless=1:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited:log-level=error" \
    -pix_fmt yuv420p10le "$WORK/ramp.mkv"
ffmpeg -hide_banner -loglevel error -y -i "$WORK/ramp.mkv" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$WORK/ramp.hevc"

# Decoded input codes (centre pixel), so the comparison uses what the encoder actually stored.
ffmpeg -hide_banner -loglevel error -i "$WORK/ramp.hevc" -f rawvideo -pix_fmt yuv420p10le "$WORK/in.raw"

printf '{"profile":"8.4","length":%d,"level6":{"max_display_mastering_luminance":1000,"min_display_mastering_luminance":50,"max_content_light_level":1000,"max_frame_average_light_level":400}}' \
    "$FRAMES" >"$WORK/gen.json"
dovi_tool generate -j "$WORK/gen.json" -o "$WORK/rpu.bin" >/dev/null
dovi_tool inject-rpu -i "$WORK/ramp.hevc" --rpu-in "$WORK/rpu.bin" -o "$WORK/ramp84.hevc" >/dev/null

ffmpeg -hide_banner -loglevel error -y -init_hw_device vulkan -i "$WORK/ramp84.hevc" \
    -vf "libplacebo=apply_dolbyvision=1:color_primaries=bt2020:color_trc=smpte2084:colorspace=gbr:range=pc:tonemapping=clip:gamut_mode=clip:peak_detect=0:format=gbrp16le" \
    -f rawvideo -pix_fmt gbrp16le "$WORK/ref.raw"

"$ANALYZER" -i "$WORK/ramp.mkv" -o "$WORK/ramp.bin" --no-crop --downscale 1 --sample-rate 1 \
    "${HWACCEL_ARGS[@]}" >"$WORK/analyzer.log" 2>&1 || { cat "$WORK/analyzer.log" >&2; exit 1; }

python3 - "$WORK" "$FRAMES" "$TOLERANCE" <<'PY'
import json, struct, sys
work, frames, tol = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
w = h = 64
centre = 32 * w + 32
inp = open(f"{work}/in.raw", "rb").read()
in_frame = w * h * 3 // 2 * 2
codes = [struct.unpack_from("<H", inp, i * in_frame + 2 * centre)[0] for i in range(frames)]
ref = open(f"{work}/ref.raw", "rb").read()
ref_frame = w * h * 2 * 3
side = json.load(open(f"{work}/ramp.bin.l1.json"))
assert side["version"] >= 3, "sidecar predates the 8.4 mapping"
mapping = side["analysis"].get("luminance_mapping")
assert mapping == "dovi84-v1", f"unexpected luminance_mapping {mapping!r}"
measured = side["frames"]["min_pq_12bit"]
assert len(measured) == frames, f"sidecar has {len(measured)} frames, expected {frames}"
worst = 0.0
for i, code in enumerate(codes):
    g, b, r = (struct.unpack_from("<H", ref, i * ref_frame + p * w * h * 2 + 2 * centre)[0] / 65535
               for p in range(3))
    ref_pq = (0.2627 * r + 0.6780 * g + 0.0593 * b) * 4095
    err = measured[i] - ref_pq
    worst = max(worst, abs(err))
    flag = "  <-- FAIL" if abs(err) > tol else ""
    print(f"code {code:4d}: analyzer {measured[i]:4d}  libplacebo {ref_pq:7.1f}  diff {err:+5.1f}{flag}")
print(f"worst |diff| = {worst:.2f} 12-bit PQ codes (tolerance {tol})")
sys.exit(1 if worst > tol else 0)
PY
