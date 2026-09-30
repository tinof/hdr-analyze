#!/usr/bin/env bash
# Check the analyzer's HLG max-RGB (the full Dolby Vision 8.4 decode) against a DV reference renderer.
#
# Companion to validate_hlg_dv84.sh (luma). Builds lossless flat HLG colour patches (red, green,
# blue, yellow, cyan, magenta at 100% and 75% saturation, plus grey, at HLG signal levels 0.25,
# 0.5, 0.75 and 1.0), injects a Profile 8.4 RPU, renders it through ffmpeg's libplacebo filter with
# Dolby Vision applied, and compares each frame's max(R,G,B) * 4095 with the per-frame minimum the
# analyzer writes to its sidecar (not temporally smoothed; on a flat frame it equals the frame's
# max-RGB in the max-rgb peak domain).
#
# Reference render settings, and why:
# - disable_fbos=1 with packed rgba64le output: with FBOs libplacebo keeps intermediates in half
#   float, which adds up to ~5 12-bit codes of error on saturated colours.
# - The RPU carries L1 max_pq 4095: libplacebo takes the display tone-mapping source peak from L1
#   (or source_max_pq 3079 when L1 is absent) and would otherwise clip bright near-white to
#   ~1000 nits, which is renderer behaviour, not the DV decode.
# The analyzer clamps max-RGB to the RPU's declared source range [62, 3079] (as its luma table
# does), so the reference is compared with the same clamp; the unclamped value is printed too.
# 100% magenta at level 1.0 is outside BT.2020 (B' > 1, G' < 0) and is gamut-mapped by libplacebo;
# both sides clamp it to 3079.
#
# Needs: ffmpeg with libx265, libplacebo and a working Vulkan device; dovi_tool; python3; a built
# analyzer. Usage:
#   scripts/validate_hlg_dv84_color.sh [path/to/hdr_analyzer_mvp] [--hwaccel cuda]
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

# Flat patches as raw yuv420p10le (BT.2020 non-constant-luminance, limited range), one per frame.
FRAMES=$(python3 - "$WORK" <<'PY'
import json, struct, sys
work = sys.argv[1]
w = h = 64
KR, KB = 0.2627, 0.0593
COLOURS = {"red": (1, 0, 0), "green": (0, 1, 0), "blue": (0, 0, 1),
           "yellow": (1, 1, 0), "cyan": (0, 1, 1), "magenta": (1, 0, 1)}

def encode(r, g, b):
    y = KR * r + (1 - KR - KB) * g + KB * b
    return (round(64 + 876 * y), round(512 + 896 * (b - y) / (2 * (1 - KB))),
            round(512 + 896 * (r - y) / (2 * (1 - KR))))

patches = []
for level in (0.25, 0.5, 0.75, 1.0):
    patches.append(("grey", 100, level, encode(level, level, level)))
    for sat in (1.0, 0.75):
        for name, rgb in COLOURS.items():
            # 75% saturation mixes 25% white of the same signal level into the colour.
            patches.append((name, round(sat * 100), level,
                            encode(*(level * (sat * c + 1 - sat) for c in rgb))))
with open(f"{work}/patches.yuv", "wb") as f:
    for *_, (y, cb, cr) in patches:
        f.write(struct.pack("<H", y) * (w * h) + struct.pack("<H", cb) * (w * h // 4)
                + struct.pack("<H", cr) * (w * h // 4))
json.dump(patches, open(f"{work}/patches.json", "w"))
print(len(patches))
PY
)

ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt yuv420p10le -s 64x64 -r 24 -i "$WORK/patches.yuv" \
    -c:v libx265 \
    -x265-params "lossless=1:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited:log-level=error" \
    -pix_fmt yuv420p10le "$WORK/patches.mkv"
ffmpeg -hide_banner -loglevel error -y -i "$WORK/patches.mkv" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$WORK/patches.hevc"
ffmpeg -hide_banner -loglevel error -i "$WORK/patches.hevc" -f rawvideo -pix_fmt yuv420p10le "$WORK/decoded.yuv"
cmp -s "$WORK/decoded.yuv" "$WORK/patches.yuv" || { echo "encode was not lossless" >&2; exit 1; }

printf '{"profile":"8.4","length":%d,"level6":{"max_display_mastering_luminance":1000,"min_display_mastering_luminance":50,"max_content_light_level":1000,"max_frame_average_light_level":400},"default_metadata_blocks":[{"Level1":{"min_pq":0,"max_pq":4095,"avg_pq":2000}}]}' \
    "$FRAMES" >"$WORK/gen.json"
dovi_tool generate -j "$WORK/gen.json" -o "$WORK/rpu.bin" >/dev/null
dovi_tool inject-rpu -i "$WORK/patches.hevc" --rpu-in "$WORK/rpu.bin" -o "$WORK/patches84.hevc" >/dev/null

ffmpeg -hide_banner -loglevel error -y -init_hw_device vulkan -i "$WORK/patches84.hevc" \
    -vf "libplacebo=apply_dolbyvision=1:color_primaries=bt2020:color_trc=smpte2084:colorspace=gbr:range=pc:tonemapping=clip:gamut_mode=clip:peak_detect=0:disable_fbos=1:format=rgba64le" \
    -f rawvideo -pix_fmt rgba64le "$WORK/ref.raw"

"$ANALYZER" -i "$WORK/patches.mkv" -o "$WORK/patches.bin" --no-crop --downscale 1 --sample-rate 1 \
    --peak-domain max-rgb "${HWACCEL_ARGS[@]}" >"$WORK/analyzer.log" 2>&1 || { cat "$WORK/analyzer.log" >&2; exit 1; }

python3 - "$WORK" "$TOLERANCE" <<'PY'
import json, struct, sys
work, tol = sys.argv[1], float(sys.argv[2])
w = h = 64
centre = 32 * w + 32
SOURCE_MIN, SOURCE_MAX = 62, 3079
patches = json.load(open(f"{work}/patches.json"))
ref = open(f"{work}/ref.raw", "rb").read()
side = json.load(open(f"{work}/patches.bin.l1.json"))
mapping = side["analysis"].get("luminance_mapping")
assert mapping == "dovi84-v2", f"unexpected luminance_mapping {mapping!r}"
assert side["peak_domain"] == "max-rgb", side["peak_domain"]
measured = side["frames"]["min_pq_12bit"]
assert len(measured) == len(patches), f"sidecar has {len(measured)} frames, expected {len(patches)}"
worst, worst_by_colour = 0.0, {}
print("colour   sat level  Y'  Cb  Cr   analyzer  libplacebo(clamped)  unclamped   diff")
for i, (colour, sat, level, codes) in enumerate(patches):
    r, g, b, _ = struct.unpack_from("<4H", ref, i * w * h * 8 + 8 * centre)
    unclamped = max(r, g, b) / 65535 * 4095
    reference = min(max(unclamped, SOURCE_MIN), SOURCE_MAX)
    err = measured[i] - reference
    worst = max(worst, abs(err))
    worst_by_colour[colour] = max(worst_by_colour.get(colour, 0.0), abs(err))
    flag = "  <-- FAIL" if abs(err) > tol else ""
    print(f"{colour:8s} {sat:3d}% {level:4.2f} {codes[0]:4d} {codes[1]:3d} {codes[2]:3d}   {measured[i]:6d}"
          f"   {reference:10.1f}          {unclamped:7.1f}  {err:+6.2f}{flag}")
print("worst |diff| per colour: " + ", ".join(f"{c} {e:.2f}" for c, e in worst_by_colour.items()))
print(f"worst |diff| = {worst:.2f} 12-bit PQ codes (tolerance {tol})")
sys.exit(1 if worst > tol else 0)
PY
