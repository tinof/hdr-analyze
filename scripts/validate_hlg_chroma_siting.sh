#!/usr/bin/env bash
# Measure how the analyzer's 4:2:0 chroma handling in the HLG Profile 8.4 decode differs from the
# spec composer, on non-flat patterns and (optionally) on real HLG cuts. ROADMAP P8, design and
# results in docs/HLG_COMPOSER.md section 9.
#
# Three decodes (tools/fit_hlg_composer chroma-siting computes all of them per pixel):
# - analyzer: each chroma sample shared by its 2x2 quad, MMR with the pixel's own luma
#   (hdr_analyzer_mvp analysis/frame.rs).
# - spec: ETSI GS CCM 001 v1.1.1 5.4.2.3.3, MMR at chroma resolution on luma down-sampled to the
#   chroma positions, then the composed chroma upsampled (nearest, or bilinear for chroma location
#   left / top-left). spec-float keeps the analyzer's arithmetic (code / 1023, f32); spec-fixed
#   is the spec's integer arithmetic (code / 1024, 16-bit output, inputs clamped to the pivots).
# - renderer: chroma upsampled first, then the MMR per pixel (libplacebo's order).
#
# Synthetic part (always): colour edges at even and odd offsets, 1- and 2-px colour lines,
# isolated 1x1 / 2x2 / 3x3 saturated highlights at odd offsets, a colour ramp over a luma ramp and
# flat controls, 256x256, encoded lossless twice (chroma location left and top-left). Two anchors
# tie the tool to independent decodes and decide the exit status:
# - anchor 1: the tool's analyzer variant reproduces the analyzer's per-frame max-RGB maximum
#   (--dump-frame-stats raw_max_pq) within 0.5 code and average (sidecar, integer codes) within 1.
# - anchor 2: the tool's renderer variant for the clip's siting matches libplacebo
#   (upscaler=bilinear) within TOLERANCE codes: per pixel at p99, at every pixel of the
#   highlight/edge regions, and in frame max.
# The analyzer-vs-spec numbers it prints are the measurement, not a pass/fail criterion.
#
# Real cuts (--cuts DIR, local only): every <DIR>/<cut>/input.mkv of the HLG development cuts is
# analyzed (anchor 1 applies) and decoded through the tool; per-scene differences between the
# analyzer and the spec decode for the cut's own chroma location are summarized. --every N decodes
# every Nth frame only (scene statistics then use those frames on both sides).
#
# Needs: ffmpeg with libx265, libplacebo and a working Vulkan device; dovi_tool; python3; cargo; a
# built analyzer. Usage:
#   scripts/validate_hlg_chroma_siting.sh [path/to/hdr_analyzer_mvp] [--composer bt2100|preset]
#       [--hwaccel cuda] [--cuts DIR] [--every N] [--keep DIR]
set -euo pipefail
cd "$(dirname "$0")/.."

ANALYZER=target/release/hdr_analyzer_mvp
HWACCEL_ARGS=()
COMPOSER=bt2100
CUTS=""
EVERY=1
KEEP=""
while [ $# -gt 0 ]; do
    case "$1" in
        --hwaccel) HWACCEL_ARGS=(--hwaccel "${2:?--hwaccel needs a value}"); shift 2 ;;
        --composer) COMPOSER="${2:?--composer needs a value}"; shift 2 ;;
        --cuts) CUTS="${2:?--cuts needs a directory}"; shift 2 ;;
        --every) EVERY="${2:?--every needs a value}"; shift 2 ;;
        --keep) KEEP="${2:?--keep needs a directory}"; shift 2 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) ANALYZER="$1"; shift ;;
    esac
done
case "$COMPOSER" in
    preset) MAPPING=dovi84-v2 ;;
    bt2100) MAPPING=dovi84-bt2100-v1 ;;
    *) echo "unknown composer: $COMPOSER (preset or bt2100)" >&2; exit 2 ;;
esac
TOLERANCE="${TOLERANCE:-4}"
HLG_CUTS=(g1-wimbledon-2024-final-rally g2-glastonbury-2025-supergrass-stage
    g3-the-green-planet-tropical-canopy g4-ucl-bayern-psg-match-action d11-bluelights-s01e02-hlg)

for tool in ffmpeg ffprobe dovi_tool python3 cargo; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
[ -x "$ANALYZER" ] || { echo "analyzer not found: $ANALYZER (build it first)" >&2; exit 2; }
[ -z "$CUTS" ] || [ -d "$CUTS" ] || { echo "cuts directory not found: $CUTS" >&2; exit 2; }

if [ -n "$KEEP" ]; then
    mkdir -p "$KEEP"
    WORK="$(cd "$KEEP" && pwd)"
else
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi

cargo build --release -q --manifest-path tools/fit_hlg_composer/Cargo.toml
FIT=tools/fit_hlg_composer/target/release/fit_hlg_composer

analyze() { # input output-stem
    "$ANALYZER" -i "$1" -o "$2.bin" --no-crop --downscale 1 --sample-rate 1 --peak-domain max-rgb \
        --peak-estimator max --hlg-composer "$COMPOSER" --dump-frame-stats "$2.frames.csv" \
        "${HWACCEL_ARGS[@]}" >"$2.analyzer.log" 2>&1 || { cat "$2.analyzer.log" >&2; exit 1; }
}

W=256
H=256
for siting in left topleft; do
    case "$siting" in left) CHROMALOC=0 ;; topleft) CHROMALOC=2 ;; esac
    dir="$WORK/synthetic-$siting"
    mkdir -p "$dir"
    FRAMES=$(python3 - "$dir" "$W" "$H" "$siting" <<'PY'
import json, struct, sys
out, w, h, siting = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
KR, KB = 0.2627, 0.0593
KG = 1 - KR - KB
COLOURS = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0), (0, 1, 1), (1, 0, 1)]

def ycc(rgb):
    r, g, b = rgb
    y = KR * r + KG * g + KB * b
    return y, (b - y) / (2 * (1 - KB)), (r - y) / (2 * (1 - KR))

def grey(level):
    return (level, level, level)

def colour(index, level, sat=1.0):
    return tuple(level * (sat * c + 1 - sat) for c in COLOURS[index % len(COLOURS)])

def clamp_code(v):
    return max(4, min(1019, round(v)))

def to_420(rgb):
    """Full-resolution R'G'B' rows to 10-bit Y and chroma down-sampled for the clip's siting."""
    yy = [[ycc(p) for p in row] for row in rgb]
    luma = [clamp_code(64 + 876 * p[0]) for row in yy for p in row]
    def chroma(k):
        plane = []
        for j in range(h // 2):
            for i in range(w // 2):
                def tap(x, y):
                    return yy[min(max(y, 0), h - 1)][min(max(x, 0), w - 1)][k]
                if siting == "left":   # co-sited horizontally, between the rows vertically
                    rows = [(2 * j, 0.5), (2 * j + 1, 0.5)]
                else:                  # co-sited both ways
                    rows = [(2 * j - 1, 0.25), (2 * j, 0.5), (2 * j + 1, 0.25)]
                v = sum(wy * (0.25 * tap(2 * i - 1, y) + 0.5 * tap(2 * i, y) + 0.25 * tap(2 * i + 1, y))
                        for y, wy in rows)
                plane.append(clamp_code(512 + 896 * v))
        return plane
    return luma, chroma(1), chroma(2)

frames = []  # (name, rgb rows, mask rows)

def new(bg):
    return [[bg] * w for _ in range(h)], [[0] * w for _ in range(h)]

def mark(mask, x0, y0, x1, y1):
    for y in range(max(y0, 0), min(y1, h)):
        for x in range(max(x0, 0), min(x1, w)):
            mask[y][x] = 1

# Flat controls (no region of interest).
for name, value in (("flat-grey", grey(0.6)), ("flat-red", colour(0, 0.75))):
    rgb, mask = new(value)
    frames.append((name, rgb, mask))

# Colour/grey stripes: vertical and horizontal boundaries at even and odd offsets.
for orientation in ("vertical", "horizontal"):
    for level in (0.75, 1.0):
        rgb, mask = new(grey(level * 0.5))
        bounds = [0] + [16 + 17 * k for k in range(14)] + [w]
        for k in range(len(bounds) - 1):
            if k % 2:
                continue
            value = colour(k // 2, level)
            for y in range(h):
                for x in range(w):
                    pos = x if orientation == "vertical" else y
                    if bounds[k] <= pos < bounds[k + 1]:
                        rgb[y][x] = value
        for b in bounds[1:-1]:
            if orientation == "vertical":
                mark(mask, b - 3, 0, b + 3, h)
            else:
                mark(mask, 0, b - 3, w, b + 3)
        frames.append((f"stripes-{orientation}-{level}", rgb, mask))

# 1- and 2-px colour lines on grey, at even and odd positions.
for background in (0.1, 0.5):
    rgb, mask = new(grey(background))
    x = 8
    for k in range(24):
        width = 1 + k % 2
        value = colour(k, 1.0 if k % 4 < 2 else 0.75)
        for y in range(h // 2):
            for dx in range(width):
                rgb[y][x + dx] = value
        mark(mask, x - 3, 0, x + width + 3, h // 2)
        yline = h // 2 + 4 + 5 * k
        for xx in range(w):
            for dy in range(width):
                if yline + dy < h:
                    rgb[yline + dy][xx] = value
        mark(mask, 0, yline - 3, w, yline + width + 3)
        x += 9 + (k % 3)
    frames.append((f"lines-bg{background}", rgb, mask))

# Isolated 1x1, 2x2, 3x3 saturated highlights at odd and even offsets.
for background in (0.1, 0.5):
    for level in (0.75, 1.0):
        rgb, mask = new(grey(background))
        k = 0
        for gy in range(12):
            for gx in range(12):
                size = 1 + (gx + gy) % 3
                x0 = 6 + 20 * gx + (gy % 2)
                y0 = 6 + 20 * gy + (gx % 2)
                value = colour(k, level, 1.0 if k % 2 == 0 else 0.75)
                for dy in range(size):
                    for dx in range(size):
                        rgb[y0 + dy][x0 + dx] = value
                mark(mask, x0 - 3, y0 - 3, x0 + size + 4, y0 + size + 4)
                k += 1
        frames.append((f"highlights-bg{background}-{level}", rgb, mask))

# Colour ramp: hue across x, signal level down y (smooth, whole frame).
import colorsys
rgb, mask = new(grey(0.5))
for y in range(h):
    level = 0.1 + 0.9 * y / (h - 1)
    for x in range(w):
        r, g, b = colorsys.hsv_to_rgb(x / w, 1.0, 1.0)
        rgb[y][x] = (r * level, g * level, b * level)
frames.append(("hue-ramp", rgb, [[1] * w for _ in range(h)]))

with open(f"{out}/patterns.yuv", "wb") as yuv, open(f"{out}/mask.u8", "wb") as m:
    for name, rgb, mask in frames:
        luma, cb, cr = to_420(rgb)
        yuv.write(struct.pack(f"<{len(luma)}H", *luma) + struct.pack(f"<{len(cb)}H", *cb)
                  + struct.pack(f"<{len(cr)}H", *cr))
        m.write(bytes(v for row in mask for v in row))
json.dump([name for name, _, _ in frames], open(f"{out}/patterns.json", "w"))
print(len(frames))
PY
)
    ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt yuv420p10le -s "${W}x${H}" -r 24 -i "$dir/patterns.yuv" \
        -c:v libx265 \
        -x265-params "lossless=1:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited:chromaloc=$CHROMALOC:log-level=error" \
        -pix_fmt yuv420p10le "$dir/patterns.mkv"
    ffmpeg -hide_banner -loglevel error -y -i "$dir/patterns.mkv" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$dir/patterns.hevc"
    ffmpeg -hide_banner -loglevel error -y -i "$dir/patterns.hevc" -f rawvideo -pix_fmt yuv420p10le "$dir/decoded.yuv"
    cmp -s "$dir/decoded.yuv" "$dir/patterns.yuv" || { echo "encode was not lossless" >&2; exit 1; }

    printf '{"profile":"8.4","length":%d,"level6":{"max_display_mastering_luminance":1000,"min_display_mastering_luminance":50,"max_content_light_level":1000,"max_frame_average_light_level":400},"default_metadata_blocks":[{"Level1":{"min_pq":0,"max_pq":4095,"avg_pq":2000}}]}' \
        "$FRAMES" >"$dir/gen.json"
    dovi_tool generate -j "$dir/gen.json" -o "$dir/rpu.bin" >/dev/null
    [ "$COMPOSER" = preset ] || "$FIT" rewrite-rpu "$COMPOSER" "$dir/rpu.bin" >/dev/null
    dovi_tool inject-rpu -i "$dir/patterns.hevc" --rpu-in "$dir/rpu.bin" -o "$dir/patterns84.hevc" >/dev/null

    ffmpeg -hide_banner -loglevel error -y -init_hw_device vulkan -i "$dir/patterns84.hevc" \
        -vf "libplacebo=apply_dolbyvision=1:upscaler=bilinear:color_primaries=bt2020:color_trc=smpte2084:colorspace=gbr:range=pc:tonemapping=clip:gamut_mode=clip:peak_detect=0:disable_fbos=1:format=rgba64le" \
        -f rawvideo -pix_fmt rgba64le "$dir/render.raw"

    analyze "$dir/patterns.mkv" "$dir/patterns"
    "$FIT" chroma-siting "$COMPOSER" "$W" "$H" --render "$dir/render.raw" --mask "$dir/mask.u8" \
        --anchor-out "$dir/anchor.csv" <"$dir/decoded.yuv" >"$dir/tool.csv" 2>/dev/null
done

if [ -n "$CUTS" ]; then
    for cut in "${HLG_CUTS[@]}"; do
        input="$CUTS/$cut/input.mkv"
        [ -f "$input" ] || { echo "skipping $cut: no input.mkv" >&2; continue; }
        dir="$WORK/cut-$cut"
        mkdir -p "$dir"
        IFS=, read -r width height siting < <(ffprobe -v error -select_streams v:0 \
            -show_entries stream=width,height,chroma_location -of csv=p=0 "$input")
        echo "$siting" >"$dir/siting"
        echo "analyzing $cut (${width}x${height}, chroma location $siting)" >&2
        analyze "$input" "$dir/cut"
        ffmpeg -hide_banner -loglevel error -i "$input" -map 0:v:0 -fps_mode passthrough \
            -f rawvideo -pix_fmt yuv420p10le - |
            "$FIT" chroma-siting "$COMPOSER" "$width" "$height" --every "$EVERY" >"$dir/tool.csv"
    done
fi

python3 - "$WORK" "$TOLERANCE" "$MAPPING" <<'PY'
import csv, glob, json, os, sys
work, tol, expected = sys.argv[1], float(sys.argv[2]), sys.argv[3]
failures = []

def tool_rows(path):
    rows = {}
    for r in csv.DictReader(open(path)):
        rows.setdefault(int(r["frame"]), {})[r["variant"]] = r
    return rows

def anchor1(stem, rows, label):
    side = json.load(open(f"{stem}.bin.l1.json"))
    mapping = side["analysis"].get("luminance_mapping")
    if mapping != expected:
        failures.append(f"{label}: luminance_mapping {mapping!r}, expected {expected!r}")
    peaks = [float(r["raw_max_pq"]) * 4095 for r in csv.DictReader(open(f"{stem}.frames.csv"))]
    avgs = side["frames"]["avg_max_rgb_pq_12bit"]
    worst_max = worst_avg = 0.0
    for frame, variants in rows.items():
        a = variants["analyzer"]
        worst_max = max(worst_max, abs(float(a["max"]) - peaks[frame]))
        worst_avg = max(worst_avg, abs(float(a["avg"]) - avgs[frame]))
    ok = worst_max <= 0.5 and worst_avg <= 1.0
    if not ok:
        failures.append(f"{label}: anchor 1 max {worst_max:.3f} avg {worst_avg:.3f}")
    print(f"{label}: anchor 1 (tool analyzer vs analyzer) worst |max| {worst_max:.3f}, "
          f"|avg| {worst_avg:.3f} codes over {len(rows)} frames {'ok' if ok else 'FAIL'}")
    return side

for siting in ("left", "topleft"):
    d = f"{work}/synthetic-{siting}"
    names = json.load(open(f"{d}/patterns.json"))
    rows = tool_rows(f"{d}/tool.csv")
    print(f"\n== synthetic, chroma location {siting}, composer {expected}")
    anchor1(f"{d}/patterns", rows, f"synthetic-{siting}")
    renderer = f"renderer-{siting}"
    worst = [0.0, 0.0, 0.0]
    for r in csv.DictReader(open(f"{d}/anchor.csv")):
        if r["variant"] == renderer:
            for k, key in enumerate(("p99", "roi_max", "frame_max_diff")):
                worst[k] = max(worst[k], float(r[key]))
    ok = max(worst) <= tol
    if not ok:
        failures.append(f"synthetic-{siting}: anchor 2 {worst}")
    print(f"anchor 2 ({renderer} vs libplacebo bilinear): worst p99 {worst[0]:.2f}, region max "
          f"{worst[1]:.2f}, frame-max diff {worst[2]:.2f} codes (tolerance {tol}) {'ok' if ok else 'FAIL'}")
    spec = [f"spec-fixed-{siting}", f"spec-float-{siting}", "spec-fixed-nearest", renderer]
    print(f"{'pattern':28s} {'analyzer':>9s} | max, avg, region max of (variant - analyzer)")
    print(f"{'':28s} {'max':>9s} | " + " | ".join(f"{v:>22s}" for v in spec))
    for frame, name in enumerate(names):
        v = rows[frame]
        a = v["analyzer"]
        cells = []
        for s in spec:
            r = v[s]
            cells.append(f"{float(r['max']) - float(a['max']):+6.2f} {float(r['avg']) - float(a['avg']):+6.2f} "
                         f"{float(r['roi_diff_max'] or 0):6.1f}")
        print(f"{name:28s} {float(a['max']):9.1f} | " + " | ".join(f"{c:>22s}" for c in cells))

for d in sorted(glob.glob(f"{work}/cut-*")):
    cut = os.path.basename(d)[4:]
    siting = open(f"{d}/siting").read().strip()
    own = "topleft" if siting == "topleft" else "left"
    rows = tool_rows(f"{d}/tool.csv")
    print(f"\n== {cut} (chroma location {siting}), composer {expected}")
    side = anchor1(f"{d}/cut", rows, cut)
    variants = ["spec-fixed-" + own, "spec-float-" + own, "spec-fixed-nearest", "spec-float-nearest",
                "renderer-" + own]
    per_scene = {v: {"max": [], "avg": []} for v in variants}
    for scene in side["scenes"]:
        frames = [f for f in range(scene["start"], scene["end"] + 1) if f in rows]
        if not frames:
            continue
        def agg(variant):
            return (max(float(rows[f][variant]["max"]) for f in frames),
                    sum(float(rows[f][variant]["avg"]) for f in frames) / len(frames))
        a_max, a_avg = agg("analyzer")
        for v in variants:
            m, av = agg(v)
            per_scene[v]["max"].append(m - a_max)
            per_scene[v]["avg"].append(av - a_avg)
    def summary(values):
        s = sorted(abs(x) for x in values)
        return (f"mean {sum(values) / len(values):+6.2f}, |p95| {s[min(len(s) - 1, len(s) * 95 // 100)]:5.2f}, "
                f"|max| {s[-1]:5.2f}")
    n = len(per_scene[variants[0]]["max"])
    print(f"{n} scenes; per-scene (variant - analyzer) in 12-bit codes:")
    for v in variants:
        print(f"  {v:20s} L1 max: {summary(per_scene[v]['max'])}   L1 avg: {summary(per_scene[v]['avg'])}")
    pixel = [float(rows[f][variants[0]]["diff_p99"]) for f in rows]
    pixel_max = [float(rows[f][variants[0]]["diff_max"]) for f in rows]
    print(f"  per pixel |{variants[0]} - analyzer|: frame p99 up to {max(pixel):.2f}, max {max(pixel_max):.1f}")

if failures:
    print("\nFAILED: " + "; ".join(failures))
    sys.exit(1)
print("\nanchors ok")
PY
