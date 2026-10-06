#!/usr/bin/env bash
# The analyzer's 4:2:0 handling of the HLG Profile 8.4 decode against the spec composer's
# structure, on non-flat patterns and on real HLG cuts. ROADMAP P8, design and results in
# docs/HLG_COMPOSER.md section 9.
#
# Decodes (tools/fit_hlg_composer chroma-siting computes all of them per pixel):
# - analyzer (the tool's model of the pre-P8 analyzer): each chroma sample shared by its 2x2 quad,
#   MMR with the pixel's own luma.
# - spec: ETSI GS CCM 001 v1.1.1 5.4.2.3.3, MMR at chroma resolution on luma down-sampled to the
#   chroma positions, then the composed chroma upsampled (nearest, or bilinear for chroma location
#   left / top-left). spec-float keeps the code / 1023 f32 arithmetic the bt2100 composer was
#   fitted in and libplacebo uses; spec-fixed is the spec's integer arithmetic (code / 1024).
# - renderer: chroma upsampled first, then the MMR per pixel (libplacebo's order).
#
# The analyzer's decode target is spec-float at the stream's own chroma location (owner decision
# 2026-10-06, docs/ROADMAP_LOG.md).
#
# Two modes:
#
# Measurement (default): one composer (--composer), one backend (--hwaccel). Synthetic patterns
# (colour edges at even and odd offsets, 1- and 2-px lines, 1x1 to 3x3 highlights, a colour ramp,
# flat controls; 256x256, lossless, chroma location left and top-left) and, with --cuts, every
# HLG cut. Prints how every variant differs from the analyzer, per frame and per scene. Exit
# status: anchor 2 only (below).
#
# Gate (--gate --cuts DIR): the P8 acceptance gate. Both composers, CPU and CUDA (--cpu-only skips
# CUDA and says so), every frame. Fails unless:
# - per frame, on the synthetic clips and every HLG cut: |tool spec-float-<own> max - analyzer
#   raw_max_pq| <= 0.5 code and |tool avg - analyzer avg_max_rgb_pq| <= 0.5 code (both from
#   --dump-frame-stats, unrounded), on exactly the same frames;
# - per pixel, on the synthetic clips: the analyzer's max-RGB (HDR_ANALYZER_DUMP_MAX_RGB) equals
#   the tool's spec-float-<own> bit for bit, with --no-crop and, on a letterboxed clip, with crop
#   detection on (inside the detected crop);
# - CPU and CUDA: identical .bin, identical sidecar frames/scenes/light_level, identical frame
#   statistics (the unrounded average within 1e-9), the CUDA sidecar says gpu: true, and the
#   CUDA per-pixel dump equals the tool too;
# - the sidecar names the composer's current luminance_mapping (`fit_hlg_composer mapping`), the
#   analyzer reports the chroma location ffprobe reports, and each cut matches its manifest
#   (chroma_location, frames = sidecar source.stream_frames);
# - every HLG manifest in DIR has its input.mkv and the count equals --expect-cuts (default 5);
# - anchor 2 holds.
#
# Anchor 2 (both modes): the tool's renderer variant for the clip's siting matches libplacebo
# (upscaler=bilinear) within TOLERANCE codes: per pixel at p99, at every pixel of the
# highlight/edge regions, and in frame max. It ties the tool's MMR, LUT and matrix to an
# independent implementation. Anchor 1 (the tool's analyzer variant against the analyzer) is
# printed as a diagnostic only.
#
# Needs: ffmpeg with libx265, libplacebo and a working Vulkan device; ffprobe; dovi_tool; python3;
# cargo; a built analyzer (for the gate with CUDA: built with --features cuda). Usage:
#   scripts/validate_hlg_chroma_siting.sh [path/to/hdr_analyzer_mvp] [--composer bt2100|preset]
#       [--hwaccel cuda] [--cuts DIR] [--every N] [--keep DIR]
#   scripts/validate_hlg_chroma_siting.sh [path/to/hdr_analyzer_mvp] --gate --cuts DIR
#       [--expect-cuts N] [--cpu-only] [--keep DIR]
set -euo pipefail
cd "$(dirname "$0")/.."

ANALYZER=target/release/hdr_analyzer_mvp
HWACCEL=none
COMPOSER=""
CUTS=""
EVERY=1
KEEP=""
GATE=0
CPU_ONLY=0
EXPECT_CUTS=5
while [ $# -gt 0 ]; do
    case "$1" in
        --hwaccel) HWACCEL="${2:?--hwaccel needs a value}"; shift 2 ;;
        --composer) COMPOSER="${2:?--composer needs a value}"; shift 2 ;;
        --cuts) CUTS="${2:?--cuts needs a directory}"; shift 2 ;;
        --every) EVERY="${2:?--every needs a value}"; shift 2 ;;
        --keep) KEEP="${2:?--keep needs a directory}"; shift 2 ;;
        --gate) GATE=1; shift ;;
        --cpu-only) CPU_ONLY=1; shift ;;
        --expect-cuts) EXPECT_CUTS="${2:?--expect-cuts needs a value}"; shift 2 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) ANALYZER="$1"; shift ;;
    esac
done
TOLERANCE="${TOLERANCE:-4}"

if [ "$GATE" = 1 ]; then
    [ -n "$CUTS" ] || { echo "--gate needs --cuts" >&2; exit 2; }
    [ "$EVERY" = 1 ] || { echo "--gate checks every frame; --every is refused" >&2; exit 2; }
    [ -z "$COMPOSER" ] || { echo "--gate runs both composers; --composer is refused" >&2; exit 2; }
    [ "$HWACCEL" = none ] || { echo "--gate runs CPU and CUDA itself; --hwaccel is refused" >&2; exit 2; }
    COMPOSERS=(bt2100 preset)
    if [ "$CPU_ONLY" = 1 ]; then BACKENDS=(none); else BACKENDS=(none cuda); fi
else
    [ "$CPU_ONLY" = 0 ] || { echo "--cpu-only belongs to --gate" >&2; exit 2; }
    COMPOSERS=("${COMPOSER:-bt2100}")
    BACKENDS=("$HWACCEL")
fi
for composer in "${COMPOSERS[@]}"; do
    case "$composer" in
        preset | bt2100) ;;
        *) echo "unknown composer: $composer (preset or bt2100)" >&2; exit 2 ;;
    esac
done

for tool in ffmpeg ffprobe dovi_tool python3 cargo; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
[ -x "$ANALYZER" ] || { echo "analyzer not found: $ANALYZER (build it first)" >&2; exit 2; }
[ -z "$CUTS" ] || [ -d "$CUTS" ] || { echo "cuts directory not found: $CUTS" >&2; exit 2; }
if [[ " ${BACKENDS[*]} " == *" cuda "* ]] && ! "$ANALYZER" --version | grep -q '+cuda'; then
    echo "the analyzer was built without CUDA ($ANALYZER --version); build it with --features cuda or pass --cpu-only" >&2
    exit 2
fi

if [ -n "$KEEP" ]; then
    mkdir -p "$KEEP"
    WORK="$(cd "$KEEP" && pwd)"
else
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi
rm -f "$WORK/refused-cuts" "$WORK/measured-cuts" "$WORK/missing-cuts" "$WORK/runs"

cargo build --release -q --manifest-path tools/fit_hlg_composer/Cargo.toml
FIT=tools/fit_hlg_composer/target/release/fit_hlg_composer

# analyze <input> <output stem> <composer> <backend> [dump] [extra analyzer args...]
# Records "<stem> <composer> <backend> <dump>" in $WORK/runs.
analyze() {
    local input="$1" stem="$2" composer="$3" backend="$4" dump="$5"
    shift 5
    local env=()
    if [ "$dump" = dump ]; then
        rm -rf "$stem.pixels"
        env=(HDR_ANALYZER_DUMP_MAX_RGB="$stem.pixels")
    fi
    mkdir -p "$(dirname "$stem")"
    env "${env[@]}" "$ANALYZER" -i "$input" -o "$stem.bin" --downscale 1 --sample-rate 1 \
        --peak-domain max-rgb --peak-estimator max --hlg-composer "$composer" \
        --hwaccel "$backend" --dump-frame-stats "$stem.frames.csv" "$@" \
        >"$stem.analyzer.log" 2>&1 || { cat "$stem.analyzer.log" >&2; exit 1; }
    echo "$stem $composer $backend $dump" >>"$WORK/runs"
}

W=256
H=256
BAR=32
for siting in left topleft; do
    case "$siting" in left) CHROMALOC=0 ;; topleft) CHROMALOC=2 ;; esac
    dir="$WORK/synthetic-$siting"
    mkdir -p "$dir"
    for clip in patterns letterbox; do
        bars=0
        [ "$clip" = letterbox ] && bars=$BAR
        FRAMES=$(python3 - "$dir" "$W" "$H" "$siting" "$clip" "$bars" <<'PY'
import json, struct, sys
out, w, h, siting, clip, bars = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5], int(sys.argv[6])
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

# Letterbox: black bars over the same patterns, so crop detection commits a crop whose edges cut
# through colour structure (the decode must not depend on the crop).
if bars:
    for _, rgb, mask in frames:
        for y in list(range(bars)) + list(range(h - bars, h)):
            rgb[y] = [(0.0, 0.0, 0.0)] * w
            mask[y] = [0] * w

with open(f"{out}/{clip}.yuv", "wb") as yuv, open(f"{out}/{clip}.mask.u8", "wb") as m:
    for name, rgb, mask in frames:
        luma, cb, cr = to_420(rgb)
        yuv.write(struct.pack(f"<{len(luma)}H", *luma) + struct.pack(f"<{len(cb)}H", *cb)
                  + struct.pack(f"<{len(cr)}H", *cr))
        m.write(bytes(v for row in mask for v in row))
json.dump([name for name, _, _ in frames], open(f"{out}/{clip}.json", "w"))
print(len(frames))
PY
)
        ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt yuv420p10le -s "${W}x${H}" -r 24 -i "$dir/$clip.yuv" \
            -c:v libx265 \
            -x265-params "lossless=1:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited:chromaloc=$CHROMALOC:log-level=error" \
            -pix_fmt yuv420p10le "$dir/$clip.mkv"
        ffmpeg -hide_banner -loglevel error -y -i "$dir/$clip.mkv" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$dir/$clip.hevc"
        ffmpeg -hide_banner -loglevel error -y -i "$dir/$clip.hevc" -f rawvideo -pix_fmt yuv420p10le "$dir/$clip.decoded.yuv"
        cmp -s "$dir/$clip.decoded.yuv" "$dir/$clip.yuv" || { echo "encode was not lossless" >&2; exit 1; }
    done

    for composer in "${COMPOSERS[@]}"; do
        cdir="$dir/$composer"
        mkdir -p "$cdir"
        printf '{"profile":"8.4","length":%d,"level6":{"max_display_mastering_luminance":1000,"min_display_mastering_luminance":50,"max_content_light_level":1000,"max_frame_average_light_level":400},"default_metadata_blocks":[{"Level1":{"min_pq":0,"max_pq":4095,"avg_pq":2000}}]}' \
            "$FRAMES" >"$cdir/gen.json"
        dovi_tool generate -j "$cdir/gen.json" -o "$cdir/rpu.bin" >/dev/null
        [ "$composer" = preset ] || "$FIT" rewrite-rpu "$composer" "$cdir/rpu.bin" >/dev/null
        dovi_tool inject-rpu -i "$dir/patterns.hevc" --rpu-in "$cdir/rpu.bin" -o "$cdir/patterns84.hevc" >/dev/null
        ffmpeg -hide_banner -loglevel error -y -init_hw_device vulkan -i "$cdir/patterns84.hevc" \
            -vf "libplacebo=apply_dolbyvision=1:upscaler=bilinear:color_primaries=bt2020:color_trc=smpte2084:colorspace=gbr:range=pc:tonemapping=clip:gamut_mode=clip:peak_detect=0:disable_fbos=1:format=rgba64le" \
            -f rawvideo -pix_fmt rgba64le "$cdir/render.raw"
        "$FIT" chroma-siting "$composer" "$W" "$H" --render "$cdir/render.raw" --mask "$dir/patterns.mask.u8" \
            --anchor-out "$cdir/anchor.csv" --dump-pixels "$cdir/patterns.tool-pixels" \
            <"$dir/patterns.decoded.yuv" >"$cdir/patterns.tool.csv" 2>/dev/null
        if [ "$GATE" = 1 ]; then
            "$FIT" chroma-siting "$composer" "$W" "$H" --dump-pixels "$cdir/letterbox.tool-pixels" \
                <"$dir/letterbox.decoded.yuv" >"$cdir/letterbox.tool.csv" 2>/dev/null
        fi
        for backend in "${BACKENDS[@]}"; do
            analyze "$dir/patterns.mkv" "$cdir/$backend/patterns" "$composer" "$backend" dump --no-crop
            if [ "$GATE" = 1 ]; then
                analyze "$dir/letterbox.mkv" "$cdir/$backend/letterbox" "$composer" "$backend" dump
            fi
        done
    done
done

if [ -n "$CUTS" ]; then
    mapfile -t HLG_CUTS < <(python3 - "$CUTS" <<'PY'
import glob, json, os, sys
for manifest in sorted(glob.glob(os.path.join(sys.argv[1], "*", "manifest.json"))):
    if json.load(open(manifest)).get("transfer") == "hlg":
        print(os.path.basename(os.path.dirname(manifest)))
PY
)
    [ "${#HLG_CUTS[@]}" -gt 0 ] || { echo "no HLG cuts (manifest.json transfer hlg) in $CUTS" >&2; exit 2; }
    for cut in "${HLG_CUTS[@]}"; do
        input="$CUTS/$cut/input.mkv"
        if [ ! -f "$input" ]; then
            echo "skipping $cut: no input.mkv" >&2
            echo "$cut" >>"$WORK/missing-cuts"
            continue
        fi
        dir="$WORK/cut-$cut"
        mkdir -p "$dir"
        IFS=, read -r width height siting < <(ffprobe -v error -select_streams v:0 \
            -show_entries stream=width,height,chroma_location -of csv=p=0 "$input")
        # The tool models chroma location left (type 0, the H.265 default) and top-left (type 2).
        case "$siting" in
            left | topleft) ;;
            unspecified | "") siting=left ;;
            *)
                echo "refusing $cut: chroma location $siting is not modelled" >&2
                echo "$cut ($siting)" >>"$WORK/refused-cuts"
                continue
                ;;
        esac
        echo "$siting" >"$dir/siting"
        cp "$CUTS/$cut/manifest.json" "$dir/manifest.json"
        for composer in "${COMPOSERS[@]}"; do
            echo "analyzing $cut (${width}x${height}, chroma location $siting, $composer)" >&2
            for backend in "${BACKENDS[@]}"; do
                analyze "$input" "$dir/$composer/$backend/cut" "$composer" "$backend" nodump --no-crop
            done
            ffmpeg -hide_banner -loglevel error -i "$input" -map 0:v:0 -fps_mode passthrough \
                -f rawvideo -pix_fmt yuv420p10le - |
                "$FIT" chroma-siting "$composer" "$width" "$height" --every "$EVERY" >"$dir/$composer/tool.csv"
        done
        echo "$dir" >>"$WORK/measured-cuts"
    done
fi

MAPPINGS=""
for composer in "${COMPOSERS[@]}"; do
    MAPPINGS="$MAPPINGS $composer=$("$FIT" mapping "$composer")"
done

python3 - "$WORK" "$TOLERANCE" "$GATE" "$EXPECT_CUTS" "$CPU_ONLY" "$MAPPINGS" <<'PY'
import csv, glob, json, os, re, struct, sys
work, tol, gate, expect_cuts, cpu_only = sys.argv[1], float(sys.argv[2]), sys.argv[3] == "1", int(sys.argv[4]), sys.argv[5] == "1"
mappings = dict(item.split("=") for item in sys.argv[6].split())
failures = []
CODES = 4095.0
SITING_NAMES = {"left": "left", "topleft": "top-left"}

def fail(message):
    failures.append(message)
    print(f"  FAIL {message}")

def tool_rows(path):
    rows = {}
    for r in csv.DictReader(open(path)):
        rows.setdefault(int(r["frame"]), {})[r["variant"]] = r
    return rows

runs = {}
for line in open(f"{work}/runs"):
    stem, composer, backend, dump = line.split()
    runs[stem] = (composer, backend, dump == "dump")

def load_run(stem):
    side = json.load(open(f"{stem}.bin.l1.json"))
    stats = list(csv.DictReader(open(f"{stem}.frames.csv")))
    return side, stats

def check_run(stem, rows, siting, label):
    """Per-frame P8 gate and metadata checks of one analyzer run against the tool."""
    composer, backend, _ = runs[stem]
    side, stats = load_run(stem)
    mapping = side["analysis"].get("luminance_mapping")
    if mapping != mappings[composer]:
        fail(f"{label}: luminance_mapping {mapping!r}, expected {mappings[composer]!r}")
    gpu = side["analysis"].get("gpu")
    if gpu != (backend == "cuda"):
        fail(f"{label}: sidecar analysis.gpu is {gpu} for --hwaccel {backend}")
    log = open(f"{stem}.analyzer.log").read()
    reported = re.search(r"HLG chroma location: (\S+)", log)
    if reported is None:
        fail(f"{label}: the analyzer does not report the HLG chroma location")
    elif reported.group(1) != SITING_NAMES[siting]:
        fail(f"{label}: the analyzer used chroma location {reported.group(1)}, ffprobe says {siting}")
    if stats and "avg_max_rgb_pq" not in stats[0]:
        fail(f"{label}: --dump-frame-stats has no avg_max_rgb_pq column (analyzer too old)")
        return side
    target = f"spec-float-{siting}"
    n = len(stats)
    if sorted(rows) != list(range(n)) or len(side["frames"]["avg_max_rgb_pq_12bit"]) != n:
        fail(f"{label}: frames differ: tool {len(rows)}, frame statistics {n}, sidecar "
             f"{len(side['frames']['avg_max_rgb_pq_12bit'])}")
        return side
    worst_max = worst_avg = 0.0
    for frame, row in enumerate(stats):
        t = rows[frame][target]
        worst_max = max(worst_max, abs(float(t["max"]) - float(row["raw_max_pq"]) * CODES))
        worst_avg = max(worst_avg, abs(float(t["avg"]) - float(row["avg_max_rgb_pq"]) * CODES))
    ok = worst_max <= 0.5 and worst_avg <= 0.5
    print(f"  {label}: analyzer vs {target}: worst |max| {worst_max:.3f}, |avg| {worst_avg:.3f} "
          f"codes over {n} frames {'ok' if ok else 'FAIL'}")
    if not ok:
        failures.append(f"{label}: analyzer vs {target} max {worst_max:.3f} avg {worst_avg:.3f}")
    # Diagnostic: the tool's model of the pre-P8 analyzer.
    old_max = max(abs(float(rows[f]["analyzer"]["max"]) - float(stats[f]["raw_max_pq"]) * CODES) for f in rows)
    print(f"  {label}: (diagnostic) analyzer vs the tool's pre-P8 model: worst |max| {old_max:.3f}")
    return side

def check_pixels(stem, tool_dir, siting, width, label, expect_crop):
    """Bit equality of the analyzer's per-pixel max-RGB with the tool's spec-float-<own>."""
    files = sorted(glob.glob(f"{stem}.pixels/frame_*.f32"))
    tool_files = sorted(glob.glob(f"{tool_dir}/frame_*_spec-float-{siting}.f32"))
    if not files or len(files) != len(tool_files):
        fail(f"{label}: per-pixel dumps: analyzer {len(files)} frames, tool {len(tool_files)}")
        return
    mismatched = checked = 0
    worst = 0.0
    cropped = False
    for path in files:
        m = re.search(r"frame_(\d+)_x(\d+)_y(\d+)_w(\d+)_h(\d+)\.f32$", path)
        frame, x0, y0, w, h = map(int, m.groups())
        cropped |= (w, h) != (width, width)
        ours = open(path, "rb").read()
        tool = open(f"{tool_dir}/frame_{frame:06d}_spec-float-{siting}.f32", "rb").read()
        checked += w * h
        for y in range(h):
            a_row = ours[4 * y * w:4 * (y + 1) * w]
            start = 4 * ((y0 + y) * width + x0)
            b_row = tool[start:start + 4 * w]
            if a_row == b_row:
                continue
            for a, b in zip(struct.unpack(f"<{w}f", a_row), struct.unpack(f"<{w}f", b_row)):
                if struct.pack("<f", a) != struct.pack("<f", b):
                    mismatched += 1
                    worst = max(worst, abs(a - b) * CODES) if a == a else float("inf")
    note = ""
    if expect_crop and not cropped:
        fail(f"{label}: crop detection did not crop the letterboxed clip")
    elif expect_crop:
        note = f" inside the detected crop ({w}x{h} at {x0},{y0})"
    ok = mismatched == 0
    print(f"  {label}: per pixel vs tool spec-float-{siting}{note}: {mismatched} of {checked} differ "
          f"(worst {worst:.3f} codes) {'ok' if ok else 'FAIL'}")
    if not ok:
        failures.append(f"{label}: {mismatched} pixels differ from spec-float-{siting}")

def check_backends(cpu_stem, cuda_stem, label):
    """CPU and CUDA must agree: .bin bytes, sidecar frames/scenes/light_level, frame statistics."""
    problems = []
    if open(f"{cpu_stem}.bin", "rb").read() != open(f"{cuda_stem}.bin", "rb").read():
        problems.append(".bin differs")
    cpu, cuda = json.load(open(f"{cpu_stem}.bin.l1.json")), json.load(open(f"{cuda_stem}.bin.l1.json"))
    for key in ("frames", "scenes", "light_level"):
        if cpu.get(key) != cuda.get(key):
            problems.append(f"sidecar {key} differs")
    a, b = list(csv.DictReader(open(f"{cpu_stem}.frames.csv"))), list(csv.DictReader(open(f"{cuda_stem}.frames.csv")))
    if len(a) != len(b):
        problems.append(f"frame statistics rows {len(a)} vs {len(b)}")
    else:
        for i, (ra, rb) in enumerate(zip(a, b)):
            head = [k for k in ra if k != "avg_max_rgb_pq"]
            if any(ra[k] != rb[k] for k in head) or abs(float(ra.get("avg_max_rgb_pq", 0)) - float(rb.get("avg_max_rgb_pq", 0))) > 1e-9:
                problems.append(f"frame statistics differ at frame {i}")
                break
    print(f"  {label}: CPU vs CUDA {'identical' if not problems else 'FAIL: ' + ', '.join(problems)}")
    for p in problems:
        failures.append(f"{label}: CPU vs CUDA: {p}")

backends = sorted({backend for _, backend, _ in runs.values()})
if gate and cpu_only:
    print("CUDA not checked (--cpu-only)")

for siting in ("left", "topleft"):
    d = f"{work}/synthetic-{siting}"
    names = json.load(open(f"{d}/patterns.json"))
    for composer in mappings:
        cdir = f"{d}/{composer}"
        print(f"\n== synthetic, chroma location {siting}, composer {composer} ({mappings[composer]})")
        rows = tool_rows(f"{cdir}/patterns.tool.csv")
        renderer = f"renderer-{siting}"
        worst = [0.0, 0.0, 0.0]
        checked = 0
        for r in csv.DictReader(open(f"{cdir}/anchor.csv")):
            if r["variant"] == renderer:
                checked += 1
                for k, key in enumerate(("p99", "roi_max", "frame_max_diff")):
                    worst[k] = max(worst[k], float(r[key]))
        ok = max(worst) <= tol and checked == len(names)
        if not ok:
            failures.append(f"synthetic-{siting} {composer}: anchor 2 {worst}, {checked} of {len(names)} frames")
        print(f"  anchor 2 ({renderer} vs libplacebo bilinear): worst p99 {worst[0]:.2f}, region max "
              f"{worst[1]:.2f}, frame-max diff {worst[2]:.2f} codes (tolerance {tol}) {'ok' if ok else 'FAIL'}")
        for backend in backends:
            label = f"synthetic-{siting} {composer} {backend}"
            stem = f"{cdir}/{backend}/patterns"
            if gate:
                check_run(stem, rows, siting, label)
                check_pixels(stem, f"{cdir}/patterns.tool-pixels", siting, 256, label, False)
                check_pixels(f"{cdir}/{backend}/letterbox", f"{cdir}/letterbox.tool-pixels", siting, 256,
                             f"letterbox-{siting} {composer} {backend}", True)
        if gate and "cuda" in backends:
            check_backends(f"{cdir}/none/patterns", f"{cdir}/cuda/patterns", f"synthetic-{siting} {composer}")
            check_backends(f"{cdir}/none/letterbox", f"{cdir}/cuda/letterbox", f"letterbox-{siting} {composer}")
        # Measurement table against the analyzer (first backend).
        side, stats = load_run(f"{cdir}/{backends[0]}/patterns")
        spec = [f"spec-float-{siting}", f"spec-fixed-{siting}", "spec-fixed-nearest", renderer]
        print(f"  {'pattern':28s} {'analyzer':>9s} | max, avg of (variant - analyzer), from frame statistics")
        print(f"  {'':28s} {'max':>9s} | " + " | ".join(f"{v:>16s}" for v in spec))
        for frame, name in enumerate(names):
            a_max = float(stats[frame]["raw_max_pq"]) * CODES
            a_avg = float(stats[frame].get("avg_max_rgb_pq", "nan")) * CODES
            cells = [f"{float(rows[frame][s]['max']) - a_max:+7.2f} {float(rows[frame][s]['avg']) - a_avg:+7.2f}" for s in spec]
            print(f"  {name:28s} {a_max:9.1f} | " + " | ".join(f"{c:>16s}" for c in cells))

if os.path.exists(f"{work}/refused-cuts"):
    for line in open(f"{work}/refused-cuts"):
        failures.append(f"chroma location not modelled: {line.strip()}")

# Only the cuts this run measured: a reused --keep directory can hold older ones.
measured = []
if os.path.exists(f"{work}/measured-cuts"):
    measured = [line.strip() for line in open(f"{work}/measured-cuts") if line.strip()]
if gate:
    if os.path.exists(f"{work}/missing-cuts"):
        for line in open(f"{work}/missing-cuts"):
            failures.append(f"HLG cut without input.mkv: {line.strip()}")
    if len(measured) != expect_cuts:
        failures.append(f"{len(measured)} HLG cuts measured, expected {expect_cuts}")

for d in sorted(measured):
    cut = os.path.basename(d)[4:]
    siting = open(f"{d}/siting").read().strip()
    manifest = json.load(open(f"{d}/manifest.json"))
    print(f"\n== {cut} (chroma location {siting})")
    if gate and manifest.get("chroma_location") != siting:
        fail(f"{cut}: manifest chroma_location {manifest.get('chroma_location')!r}, ffprobe {siting!r}")
    for composer in mappings:
        rows = tool_rows(f"{d}/{composer}/tool.csv")
        for backend in backends:
            stem = f"{d}/{composer}/{backend}/cut"
            label = f"{cut} {composer} {backend}"
            if gate:
                side = check_run(stem, rows, siting, label)
                stream_frames = side.get("source", {}).get("stream_frames")
                if stream_frames != manifest.get("frames"):
                    fail(f"{label}: sidecar stream_frames {stream_frames}, manifest frames {manifest.get('frames')}")
        if gate and "cuda" in backends:
            check_backends(f"{d}/{composer}/none/cut", f"{d}/{composer}/cuda/cut", f"{cut} {composer}")
        # Per-scene measurement against the analyzer (first backend).
        side, stats = load_run(f"{d}/{composer}/{backends[0]}/cut")
        variants = [f"spec-float-{siting}", f"spec-fixed-{siting}", "spec-float-nearest", f"renderer-{siting}", "analyzer"]
        per_scene = {v: {"max": [], "avg": []} for v in variants}
        has_avg = bool(stats) and "avg_max_rgb_pq" in stats[0]
        for scene in side["scenes"]:
            frames = [f for f in range(scene["start"], scene["end"] + 1) if f in rows and f < len(stats)]
            if not frames:
                continue
            a_max = max(float(stats[f]["raw_max_pq"]) for f in frames) * CODES
            a_avg = (sum(float(stats[f]["avg_max_rgb_pq"]) for f in frames) / len(frames) * CODES) if has_avg else float("nan")
            for v in variants:
                per_scene[v]["max"].append(max(float(rows[f][v]["max"]) for f in frames) - a_max)
                per_scene[v]["avg"].append(sum(float(rows[f][v]["avg"]) for f in frames) / len(frames) - a_avg)
        def summary(values):
            if not values:
                return "no scenes"
            s = sorted(abs(x) for x in values)
            return (f"mean {sum(values) / len(values):+6.2f}, |p95| {s[min(len(s) - 1, len(s) * 95 // 100)]:5.2f}, "
                    f"|max| {s[-1]:5.2f}")
        print(f"  {composer}: {len(per_scene[variants[0]]['max'])} scenes; per-scene (variant - analyzer) in 12-bit codes:")
        for v in variants:
            print(f"    {v:20s} L1 max: {summary(per_scene[v]['max'])}   L1 avg: {summary(per_scene[v]['avg'])}")

if failures:
    print("\nFAILED: " + "; ".join(failures))
    sys.exit(1)
print("\nP8 gate ok" if gate else "\nanchors ok")
PY
