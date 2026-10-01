#!/usr/bin/env bash
# Capture and compare final-RPU baselines.
#
# "Final RPU" means the RPU extracted from the muxed .DV.mkv that mkvdovi writes, not an
# intermediate file. A baseline taken with one build is compared with a capture of the same input
# taken with a later build. Level 1 (min/avg/max PQ) may change and is scored. Everything else in
# the RPU must stay identical.
#
#   scripts/rpu-baseline.sh capture <out-dir> <input.mkv>... [-- <mkvdovi flags>]
#   scripts/rpu-baseline.sh compare [--require-identical-l1] <baseline-dir>/<stem> <candidate-dir>/<stem>
#
# capture writes <out-dir>/<stem>/{RPU.bin,rpu.json.gz,scenes.txt,summary.txt,mkvdovi.log,
# manifest.json} and the analyzer's <stem>_measurements.bin.l1.json when mkvdovi ran the analyzer.
# It never changes or deletes an input. Set MKVDOVI=path/to/mkvdovi to capture with another build
# (default: mkvdovi from PATH).
#
# compare exit status: 0 = identical outside L1 (and L1 identical with --require-identical-l1),
# 1 = difference, 2 = usage or tool error.
#
# Needs: capture: mkvdovi, hdr_analyzer_mvp, dovi_tool, mkvmerge, ffmpeg, jq, sha256sum, gzip
# (mediainfo is recorded when present). compare: jq, gzip, awk.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
HASH_FULL_LIMIT=$((2 * 1024 * 1024 * 1024)) # inputs above this get a partial hash
HASH_HEAD_BYTES=$((64 * 1024 * 1024))
STREAM_LIMIT=$((256 * 1024 * 1024))         # exports above this are split with jq --stream

usage() {
    sed -n '9,10p' "$0" | sed 's/^# *//' >&2
    exit 2
}

die() { # die <status> <message>
    echo "error: $2" >&2
    exit "$1"
}

need_tools() {
    local tool
    for tool in "$@"; do
        command -v "$tool" >/dev/null || die 2 "missing tool: $tool"
    done
}

# ---------------------------------------------------------------------------------------------
# capture
# ---------------------------------------------------------------------------------------------

# The analyzer mkvdovi will spawn: next to the mkvdovi executable, else PATH
# (pipeline::analyzer_executable; its cwd-relative target/release step cannot match here because
# mkvdovi runs inside the scratch directory).
analyzer_for() {
    local sibling
    sibling="$(dirname "$(readlink -f "$1")")/hdr_analyzer_mvp"
    if [ -x "$sibling" ]; then echo "$sibling"; else command -v hdr_analyzer_mvp; fi
}

capture_one() { # capture_one <out-dir> <input> [mkvdovi flags...]
    local out_dir="$1" input="$2"
    shift 2
    local name stem scratch stage final link dv sidecar size mtime hash method frames
    name="$(basename "$input")"
    stem="${name%.mkv}"
    final="$out_dir/$stem"
    scratch="$out_dir/.scratch/$stem"
    stage="$scratch/baseline"

    echo "== capture: $input"
    rm -rf "$scratch"
    mkdir -p "$stage"

    # mkvdovi writes <stem>.DV.mkv, mkvdovi_temp_<stem>/ and <stem>_measurements.bin next to its
    # input (pipeline::convert_file, run_hdr_analyzer), and reuses measurements it finds there.
    # Working on a link in an empty scratch directory keeps the user's directory untouched and
    # forces a fresh analysis with the build under test.
    link="$scratch/$name"
    ln "$input" "$link" 2>/dev/null || cp --reflink=auto "$input" "$link"

    size="$(stat -c %s "$input")"
    mtime="$(stat -c %Y "$input")"
    if [ "$size" -gt "$HASH_FULL_LIMIT" ]; then
        method="size+mtime+sha256-of-first-64MiB"
        hash="$(head -c "$HASH_HEAD_BYTES" "$input" | sha256sum | cut -d' ' -f1)"
    else
        method="sha256"
        hash="$(sha256sum "$input" | cut -d' ' -f1)"
    fi

    local cmd=("$MKVDOVI_BIN" --keep-source --verify "$@" "$link")
    echo "   ${cmd[*]}"
    if ! (cd "$scratch" && "${cmd[@]}") >"$stage/mkvdovi.log" 2>&1; then
        tail -n 25 "$stage/mkvdovi.log" >&2
        die 1 "mkvdovi failed for $input (conversion or --verify). Scratch kept: $scratch"
    fi

    dv="$scratch/$stem.DV.mkv"
    [ -s "$dv" ] || die 1 "mkvdovi exited 0 but wrote no $dv. Log: $stage/mkvdovi.log"
    [ -e "$link" ] || die 1 "mkvdovi removed its input link; --keep-source was not honoured"
    [ "$(stat -c '%s %Y' "$input")" = "$size $mtime" ] || die 1 "input changed during capture: $input"

    # dovi_tool 2.3.4+ reads Matroska directly. Older versions need Annex B HEVC on stdin.
    if ! dovi_tool extract-rpu "$dv" -o "$stage/RPU.bin" >"$scratch/extract.log" 2>&1 ||
        [ ! -s "$stage/RPU.bin" ]; then
        rm -f "$stage/RPU.bin"
        ffmpeg -hide_banner -loglevel error -i "$dv" -map 0:v:0 -c:v copy -bsf:v hevc_mp4toannexb -f hevc - |
            dovi_tool extract-rpu - -o "$stage/RPU.bin" >"$scratch/extract.log" 2>&1 ||
            die 1 "RPU extraction failed for $dv. Log: $scratch/extract.log"
    fi
    [ -s "$stage/RPU.bin" ] || die 1 "empty RPU extracted from $dv"

    dovi_tool export -i "$stage/RPU.bin" -d "all=$stage/rpu.json,scenes=$stage/scenes.txt" >/dev/null ||
        die 1 "dovi_tool export failed for $stage/RPU.bin"
    dovi_tool info -i "$stage/RPU.bin" -s >"$stage/summary.txt" ||
        die 1 "dovi_tool info failed for $stage/RPU.bin"
    frames="$(sed -n 's/^ *Frames: *\([0-9][0-9]*\).*/\1/p' "$stage/summary.txt" | head -n 1)"
    [ -n "$frames" ] && [ "$frames" -gt 0 ] || die 1 "RPU from $dv has no frames"
    [ -s "$stage/rpu.json" ] || die 1 "empty RPU export for $dv"
    gzip -n -9 "$stage/rpu.json"

    sidecar="$scratch/${stem}_measurements.bin.l1.json"
    local sidecar_name=""
    if [ -f "$sidecar" ]; then
        sidecar_name="$(basename "$sidecar")"
        cp "$sidecar" "$stage/$sidecar_name"
    fi

    local commit dirty mediainfo_version=""
    commit="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
    dirty=false
    [ -n "$(git -C "$REPO" status --porcelain 2>/dev/null)" ] && dirty=true
    if command -v mediainfo >/dev/null; then
        mediainfo_version="$(mediainfo --version | tr '\n' ' ' | sed 's/  */ /g; s/ $//')"
    fi

    jq -n \
        --arg input_name "$name" \
        --arg input_path "$(readlink -f "$input")" \
        --argjson input_size "$size" \
        --argjson input_mtime "$mtime" \
        --arg hash_method "$method" \
        --arg hash "$hash" \
        --arg captured "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        --arg commit "$commit" \
        --argjson dirty "$dirty" \
        --arg mkvdovi_path "$(readlink -f "$MKVDOVI_BIN")" \
        --arg mkvdovi "$("$MKVDOVI_BIN" --version)" \
        --arg analyzer_path "$(readlink -f "$ANALYZER_BIN")" \
        --arg analyzer "$("$ANALYZER_BIN" --version)" \
        --arg dovi_tool "$(dovi_tool --version)" \
        --arg mkvmerge "$(mkvmerge --version)" \
        --arg ffmpeg "$(ffmpeg -version | sed -n 1p)" \
        --arg mediainfo "$mediainfo_version" \
        --arg command "${cmd[*]}" \
        --arg sidecar "$sidecar_name" \
        --argjson frames "$frames" \
        '{
            input: {name: $input_name, path: $input_path, size: $input_size, mtime_unix: $input_mtime,
                    hash_method: $hash_method, hash: $hash},
            captured_utc: $captured,
            repo: {commit: $commit, dirty: $dirty},
            tools: {mkvdovi: $mkvdovi, mkvdovi_path: $mkvdovi_path,
                    hdr_analyzer_mvp: $analyzer, hdr_analyzer_mvp_path: $analyzer_path,
                    dovi_tool: $dovi_tool, mkvmerge: $mkvmerge, ffmpeg: $ffmpeg,
                    mediainfo: (if $mediainfo == "" then null else $mediainfo end)},
            mkvdovi_command: $command,
            l1_sidecar: (if $sidecar == "" then null else $sidecar end),
            rpu_frame_count: $frames
        }' >"$stage/manifest.json"

    # Publish the finished directory in one step, then drop the scratch copy of the .DV.mkv.
    mv "$stage" "$final"
    rm -rf "$scratch"
    echo "   ok: $frames RPU frames -> $final"
}

cmd_capture() {
    [ $# -ge 2 ] || usage
    local out_dir="$1"
    shift
    local inputs=() extra=() seen=" " input stem
    while [ $# -gt 0 ]; do
        if [ "$1" = "--" ]; then
            shift
            extra=("$@")
            break
        fi
        inputs+=("$1")
        shift
    done
    [ ${#inputs[@]} -gt 0 ] || usage

    need_tools dovi_tool mkvmerge ffmpeg jq sha256sum gzip git
    MKVDOVI_BIN="$(command -v "${MKVDOVI:-mkvdovi}")" || die 2 "missing tool: ${MKVDOVI:-mkvdovi}"
    ANALYZER_BIN="$(analyzer_for "$MKVDOVI_BIN")" || die 2 "missing tool: hdr_analyzer_mvp"

    # Check every input before any conversion starts.
    for input in "${inputs[@]}"; do
        [ -f "$input" ] || die 2 "input not found: $input"
        case "$input" in
            *.DV.mkv) die 2 "mkvdovi skips *.DV.mkv inputs, so no baseline can be captured: $input" ;;
            *.mkv) ;;
            *) die 2 "input is not an .mkv file: $input" ;;
        esac
        stem="$(basename "${input%.mkv}")"
        case "$seen" in *" $stem "*) die 2 "two inputs share the name $stem" ;; esac
        seen="$seen$stem "
        [ ! -e "$out_dir/$stem" ] || die 2 "$out_dir/$stem already exists. Remove it to capture again."
    done

    mkdir -p "$out_dir"
    out_dir="$(cd "$out_dir" && pwd)"
    for input in "${inputs[@]}"; do
        capture_one "$out_dir" "$input" "${extra[@]}"
    done
    rmdir "$out_dir/.scratch" 2>/dev/null || true
    echo "captured ${#inputs[@]} baseline(s) in $out_dir"
}

# ---------------------------------------------------------------------------------------------
# compare
# ---------------------------------------------------------------------------------------------

# One compact JSON object per frame. Large exports are split with the streaming parser so memory
# use does not grow with the frame count.
frames_jsonl() { # frames_jsonl <capture-dir> <out>
    local dir="$1" out="$2" src
    if [ -f "$dir/rpu.json.gz" ]; then
        src="$COMPARE_WORK/$(basename "$out").json"
        gzip -dc "$dir/rpu.json.gz" >"$src"
    elif [ -f "$dir/rpu.json" ]; then
        src="$dir/rpu.json"
    else
        die 2 "no rpu.json.gz or rpu.json in $dir"
    fi
    if [ "$(stat -c %s "$src")" -gt "$STREAM_LIMIT" ]; then
        jq -cn --stream 'fromstream(1 | truncate_stream(inputs))' "$src" >"$out"
    else
        jq -c '.[]' "$src" >"$out"
    fi
}

# The frame without anything that an L1 change is allowed to move: the Level1 payload (the block
# stays as a placeholder, so a missing or extra L1 block is still a difference), the per-frame
# CRC, which covers the L1 bytes, and the scene-cut flag, which is compared on its own.
STRUCT_FILTER='
def strip_l1:
    if type == "object" and (.ext_metadata_blocks | type) == "array" then
        .ext_metadata_blocks |= map(if type == "object" and has("Level1") then {Level1: null} else . end)
    else . end;
del(.rpu_data_crc32)
| if (.vdr_dm_data | type) == "object" then
    .vdr_dm_data |= (del(.scene_refresh_flag) | .cmv29_metadata |= strip_l1 | .cmv40_metadata |= strip_l1)
  else . end'

# "min avg max" per frame, or "- - -" when the frame has no L1 block (presence is checked by the
# structural comparison).
L1_FILTER='
[.vdr_dm_data.cmv29_metadata?.ext_metadata_blocks?[]?, .vdr_dm_data.cmv40_metadata?.ext_metadata_blocks?[]?
 | objects | select(has("Level1")) | .Level1] | first
| if . == null then "-\t-\t-" else "\(.min_pq)\t\(.avg_pq)\t\(.max_pq)" end'

SCENE_FILTER='if (.vdr_dm_data | type) == "object" then (.vdr_dm_data.scene_refresh_flag | tostring) else "-" end'

# Leaf values that differ between two frames, for the report.
LEAF_DIFF='
def leaves: [paths(scalars)];
(($a | leaves) + ($b | leaves) | unique)[]
| . as $p
| (try ($a | getpath($p) | tojson) catch "absent") as $x
| (try ($b | getpath($p) | tojson) catch "absent") as $y
| select($x != $y)
| "    \($p | map(tostring) | join(".")): \($x) -> \($y)"'

cmd_compare() {
    local require_l1=0 dirs=() arg
    for arg in "$@"; do
        case "$arg" in
            --require-identical-l1) require_l1=1 ;;
            -*) usage ;;
            *) dirs+=("$arg") ;;
        esac
    done
    [ ${#dirs[@]} -eq 2 ] || usage
    local base="${dirs[0]%/}" cand="${dirs[1]%/}"
    [ -d "$base" ] || die 2 "baseline directory not found: $base"
    [ -d "$cand" ] || die 2 "candidate directory not found: $cand"
    need_tools jq gzip awk cmp

    # Any command that fails from here on is a tool error, not a comparison result.
    set -E
    trap 'echo "error: command failed at line $LINENO" >&2; exit 2' ERR
    COMPARE_WORK="$(mktemp -d)"
    trap 'rm -rf "$COMPARE_WORK"' EXIT
    local w="$COMPARE_WORK" side

    echo "baseline:  $base"
    echo "candidate: $cand"
    if [ -f "$base/manifest.json" ] && [ -f "$cand/manifest.json" ]; then
        for side in "$base" "$cand"; do
            jq -r '"  \(.tools.mkvdovi) / \(.tools.hdr_analyzer_mvp), commit \(.repo.commit[0:12])\(if .repo.dirty then " (dirty)" else "" end), captured \(.captured_utc)"' \
                "$side/manifest.json"
        done
        # A baseline only says something about the same input bits.
        if [ "$(jq -c '[.input.size, .input.hash_method, .input.hash]' "$base/manifest.json")" != \
            "$(jq -c '[.input.size, .input.hash_method, .input.hash]' "$cand/manifest.json")" ]; then
            die 2 "the two captures were made from different inputs (size or hash differs in manifest.json)"
        fi
    else
        echo "  note: manifest.json missing on one side; input identity not checked"
    fi

    frames_jsonl "$base" "$w/a.jsonl"
    frames_jsonl "$cand" "$w/b.jsonl"

    local fail=0 na nb
    na="$(wc -l <"$w/a.jsonl")"
    nb="$(wc -l <"$w/b.jsonl")"
    [ "$na" -gt 0 ] || die 2 "baseline export has no frames: $base"
    if [ "$na" -ne "$nb" ]; then
        echo "FAIL frame count: baseline $na, candidate $nb"
        exit 1
    fi
    echo "ok   frame count: $na"

    for side in a b; do
        jq -r "$SCENE_FILTER" "$w/$side.jsonl" >"$w/$side.scene"
        jq -cS "$STRUCT_FILTER" "$w/$side.jsonl" >"$w/$side.struct"
        jq -r "$L1_FILTER" "$w/$side.jsonl" >"$w/$side.l1"
    done

    # Scene cuts, reported apart from the other fields.
    local cuts
    cuts="$(grep -c '^1$' "$w/a.scene" || true)"
    if cmp -s "$w/a.scene" "$w/b.scene"; then
        echo "ok   scene cuts: $cuts, same frames"
    else
        fail=1
        echo "FAIL scene cuts differ (baseline $cuts, candidate $(grep -c '^1$' "$w/b.scene" || true))"
        paste "$w/a.scene" "$w/b.scene" | awk -F'\t' '
            $1 != $2 { n++; if (n <= 10) printf "    frame %d: scene_refresh_flag %s -> %s\n", NR - 1, $1, $2 }
            END { if (n > 10) printf "    ... %d frames in total\n", n }'
    fi

    # Everything else outside L1.
    if cmp -s "$w/a.struct" "$w/b.struct"; then
        echo "ok   outside L1: identical (profile, header, mapping, L2, L5, L6, L9, L11, L254, other blocks)"
    else
        fail=1
        local first count
        read -r count first < <(awk -v other="$w/b.struct" '
            { getline line < other; if ($0 != line) { n++; if (!first) first = NR } }
            END { print n + 0, first + 0 }' "$w/a.struct")
        echo "FAIL outside L1: $count of $na frames differ; first is frame $((first - 1)):"
        jq -rn --argjson a "$(sed -n "${first}p" "$w/a.struct")" --argjson b "$(sed -n "${first}p" "$w/b.struct")" \
            "$LEAF_DIFF" | head -n 20
    fi

    # L1 score: candidate minus baseline, in 12-bit PQ codes.
    local changed
    changed="$(paste "$w/a.l1" "$w/b.l1" | awk -F'\t' '
        BEGIN { split("min avg max", name, " ") }
        $1 == "-" || $4 == "-" { next }
        {
            scored++
            any = 0
            for (i = 1; i <= 3; i++) {
                d = $(i + 3) - $i
                if (d != 0) { n[i]++; any = 1 }
                sum[i] += d
                if (d < 0) d = -d
                if (d > worst[i]) worst[i] = d
            }
            frames_changed += any
        }
        END {
            printf "L1 score over %d frames with an L1 block (candidate - baseline, 12-bit PQ codes):\n", scored > "/dev/stderr"
            for (i = 1; i <= 3; i++)
                printf "  %s_pq: %d frames changed, mean signed diff %+.3f, max abs diff %d\n", \
                    name[i], n[i], (scored ? sum[i] / scored : 0), worst[i] > "/dev/stderr"
            print frames_changed + 0
        }' 2>"$w/l1.report")"
    cat "$w/l1.report"

    if [ "$changed" -eq 0 ]; then
        echo "ok   L1: identical"
    elif [ "$require_l1" -eq 1 ]; then
        fail=1
        echo "FAIL L1: $changed frames changed and --require-identical-l1 is set"
    else
        echo "note L1: $changed frames changed (scored, not gated; review the numbers above)"
    fi

    if [ "$fail" -ne 0 ]; then
        echo "RESULT: FAIL"
        exit 1
    fi
    echo "RESULT: PASS"
}

[ $# -ge 1 ] || usage
sub="$1"
shift
case "$sub" in
    capture) cmd_capture "$@" ;;
    compare) cmd_compare "$@" ;;
    *) usage ;;
esac
