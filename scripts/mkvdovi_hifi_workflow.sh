#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
High-fidelity mkvdovi workflow with post-run comparison.

Usage:
  mkvdovi_hifi_workflow.sh [options] <input.mkv>

Options:
  --repo-dir <path>           Repository root (default: script_dir/..)
  --optimizer-profile <name>  hdr_analyzer_mvp optimizer profile (default: aggressive)
  --force                     Remove existing <stem>.DV.mkv before running
  --keep-temp                 Keep temporary working directory
  -h, --help                  Show this help

Environment overrides (optional):
  HDR_ANALYZER_BIN            Path to hdr_analyzer_mvp binary
  MKVDOVI_BIN                 Path to mkvdovi binary
  MKVDOLBY_BIN                Deprecated alias for MKVDOVI_BIN (removed next release)
  VERIFIER_BIN                Path to verifier binary

What this workflow does:
  1) Extracts original Dolby Vision RPU summary from the input MKV
  2) Generates high-fidelity measurements:
       --downscale 1 --sample-rate 1 --optimizer-profile <profile>
  3) Runs mkvdovi with safe flags:
       --keep-source --verify --cm-version v40
  4) Extracts new Dolby Vision RPU summary from the output MKV
  5) Writes a comparison report next to the input file:
       <stem>.DV.hifi.compare.txt

EOF
}

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

require_cmd() {
    local cmd="$1"
    command -v "$cmd" >/dev/null 2>&1 || die "Required command not found: $cmd"
}

resolve_bin() {
    local env_var="$1"
    local repo_rel="$2"
    local fallback_name="$3"
    local env_val="${!env_var:-}"

    if [[ -n "$env_val" ]]; then
        [[ -x "$env_val" ]] || die "$env_var points to a non-executable path: $env_val"
        printf '%s\n' "$env_val"
        return
    fi

    if [[ -x "$REPO_DIR/$repo_rel" ]]; then
        printf '%s\n' "$REPO_DIR/$repo_rel"
        return
    fi

    if command -v "$fallback_name" >/dev/null 2>&1; then
        command -v "$fallback_name"
        return
    fi

    die "Could not resolve binary: $fallback_name (set $env_var or build it)"
}

extract_rpu_from_mkv() {
    local input_mkv="$1"
    local out_rpu="$2"

    ffmpeg -hide_banner -loglevel error \
        -i "$input_mkv" -map 0:v:0 -c copy -f hevc - \
        | dovi_tool extract-rpu -i - -o "$out_rpu" >/dev/null
}

collect_dv_probe() {
    local input_mkv="$1"
    local prefix="$2"
    local rpu="$WORK_DIR/${prefix}.rpu"
    local summary="$WORK_DIR/${prefix}.summary.txt"
    local frame0_json="$WORK_DIR/${prefix}.frame0.json"
    local l8_probe_log="$WORK_DIR/${prefix}.l8_probe.log"
    local l8_flag="$WORK_DIR/${prefix}.l8_supported"

    extract_rpu_from_mkv "$input_mkv" "$rpu"
    dovi_tool info --summary -i "$rpu" > "$summary"
    dovi_tool info -f 0 -i "$rpu" > "$frame0_json"

    if dovi_tool plot -i "$rpu" -p l8 -o "$WORK_DIR/${prefix}.l8.png" >"$l8_probe_log" 2>&1; then
        echo "yes" > "$l8_flag"
    else
        echo "no" > "$l8_flag"
    fi
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OPTIMIZER_PROFILE="aggressive"
FORCE=0
KEEP_TEMP=0

INPUT_FILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo-dir)
            [[ $# -ge 2 ]] || die "Missing value for --repo-dir"
            REPO_DIR="$2"
            shift 2
            ;;
        --optimizer-profile)
            [[ $# -ge 2 ]] || die "Missing value for --optimizer-profile"
            OPTIMIZER_PROFILE="$2"
            shift 2
            ;;
        --force)
            FORCE=1
            shift
            ;;
        --keep-temp)
            KEEP_TEMP=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        -*)
            die "Unknown option: $1"
            ;;
        *)
            if [[ -n "$INPUT_FILE" ]]; then
                die "Only one input file is supported"
            fi
            INPUT_FILE="$1"
            shift
            ;;
    esac
done

if [[ -z "$INPUT_FILE" && $# -gt 0 ]]; then
    INPUT_FILE="$1"
    shift
fi

[[ -n "$INPUT_FILE" ]] || {
    usage
    die "Input file is required"
}

[[ -f "$INPUT_FILE" ]] || die "Input file does not exist: $INPUT_FILE"
[[ "$INPUT_FILE" == *.mkv ]] || die "Input must be an .mkv file"

require_cmd ffmpeg
require_cmd dovi_tool
require_cmd mkvmerge
require_cmd python3

if ! command -v mediainfo >/dev/null 2>&1 && ! command -v ffprobe >/dev/null 2>&1; then
    die "Either mediainfo or ffprobe must be installed for mkvdovi"
fi

HDR_ANALYZER_BIN="$(resolve_bin HDR_ANALYZER_BIN target/release/hdr_analyzer_mvp hdr_analyzer_mvp)"
MKVDOVI_BIN="${MKVDOVI_BIN:-${MKVDOLBY_BIN:-}}"   # honor deprecated alias one release
MKVDOVI_BIN="$(resolve_bin MKVDOVI_BIN target/release/mkvdovi mkvdovi)"
VERIFIER_BIN="$(resolve_bin VERIFIER_BIN target/release/verifier verifier)"

INPUT_DIR="$(cd "$(dirname "$INPUT_FILE")" && pwd)"
INPUT_BASE="$(basename "$INPUT_FILE")"
INPUT_STEM="${INPUT_BASE%.mkv}"

INPUT_FILE="$INPUT_DIR/$INPUT_BASE"
MEASUREMENTS_FILE="$INPUT_DIR/${INPUT_STEM}_measurements.bin"
OUTPUT_FILE="$INPUT_DIR/${INPUT_STEM}.DV.mkv"
REPORT_FILE="$INPUT_DIR/${INPUT_STEM}.DV.hifi.compare.txt"
MKVDOVI_INPUT="$INPUT_FILE"

if [[ -e "$OUTPUT_FILE" ]]; then
    if [[ "$FORCE" -eq 1 ]]; then
        log "Removing existing output (requested by --force): $OUTPUT_FILE"
        rm -f "$OUTPUT_FILE"
    else
        die "Output already exists: $OUTPUT_FILE (use --force to overwrite)"
    fi
fi

WORK_DIR="$(mktemp -d -t mkvdovi-hifi-XXXXXX)"

cleanup() {
    if [[ "$KEEP_TEMP" -eq 0 && -d "$WORK_DIR" ]]; then
        rm -rf "$WORK_DIR"
    fi
}
trap cleanup EXIT

if [[ "$INPUT_BASE" == *.DV.mkv ]]; then
    die "Input filename ends with '.DV.mkv'; mkvdovi skips these paths. Copy/rename to a different .mkv filename and rerun."
fi

log "Input file:        $INPUT_FILE"
log "Output file:       $OUTPUT_FILE"
log "Measurements file: $MEASUREMENTS_FILE"
log "Report file:       $REPORT_FILE"
log "Working directory: $WORK_DIR"

if [[ -f "$MEASUREMENTS_FILE" ]]; then
    BACKUP_FILE="${MEASUREMENTS_FILE}.bak.$(date +%Y%m%d_%H%M%S)"
    log "Backing up existing measurements to: $BACKUP_FILE"
    cp -a "$MEASUREMENTS_FILE" "$BACKUP_FILE"
fi

log "Step 1/5: Probing original Dolby Vision metadata"
collect_dv_probe "$INPUT_FILE" "original"

log "Step 2/5: Generating high-fidelity measurements"
"$HDR_ANALYZER_BIN" "$INPUT_FILE" \
    -o "$MEASUREMENTS_FILE" \
    --downscale 1 \
    --sample-rate 1 \
    --optimizer-profile "$OPTIMIZER_PROFILE"

[[ -f "$MEASUREMENTS_FILE" ]] || die "High-fidelity measurements were not generated"

log "Step 3/5: Running verifier on high-fidelity measurements"
"$VERIFIER_BIN" "$MEASUREMENTS_FILE" > "$WORK_DIR/new_measurements.verifier.txt"

log "Step 4/5: Running mkvdovi with high-fidelity measurements"
"$MKVDOVI_BIN" \
    --keep-source \
    --verify \
    --cm-version v40 \
    "$MKVDOVI_INPUT"

[[ -f "$OUTPUT_FILE" ]] || die "mkvdovi did not produce expected output: $OUTPUT_FILE"

log "Step 5/5: Probing newly created Dolby Vision metadata"
collect_dv_probe "$OUTPUT_FILE" "new"

log "Building comparison report"
python3 - "$WORK_DIR/original.summary.txt" \
    "$WORK_DIR/new.summary.txt" \
    "$WORK_DIR/new_measurements.verifier.txt" \
    "$WORK_DIR/original.frame0.json" \
    "$WORK_DIR/new.frame0.json" \
    "$WORK_DIR/original.l8_supported" \
    "$WORK_DIR/new.l8_supported" \
    "$REPORT_FILE" \
    "$INPUT_FILE" \
    "$OUTPUT_FILE" \
    "$MEASUREMENTS_FILE" <<'PY'
import json
import re
import sys
from pathlib import Path


(
    orig_summary_path,
    new_summary_path,
    verifier_path,
    orig_frame0_path,
    new_frame0_path,
    orig_l8_flag_path,
    new_l8_flag_path,
    report_path,
    input_file,
    output_file,
    measurements_file,
) = sys.argv[1:]


def read_text(path: str) -> str:
    return Path(path).read_text(encoding="utf-8", errors="replace")


def find(pattern: str, text: str, cast=None):
    m = re.search(pattern, text, flags=re.MULTILINE)
    if not m:
        return None
    v = m.group(1).strip()
    return cast(v) if cast else v


def parse_dv_summary(text: str) -> dict:
    return {
        "frames": find(r"Frames:\s*(\d+)", text, int),
        "profile": find(r"Profile:\s*(\d+)", text, int),
        "dm_version": find(r"DM version:\s*([^\n]+)", text),
        "scene_count": find(r"Scene/shot count:\s*(\d+)", text, int),
        "rpu_mastering": find(r"RPU mastering display:\s*([^\n]+)", text),
        "l1_maxcll": find(r"RPU content light level \(L1\):\s*MaxCLL:\s*([0-9.]+)\s*nits", text, float),
        "l1_maxfall": find(r"RPU content light level \(L1\):.*?MaxFALL:\s*([0-9.]+)\s*nits", text, float),
        "l6": find(r"L6 metadata:\s*([^\n]+)", text),
        "l2_trims": find(r"L2 trims:\s*([^\n]+)", text),
        "l9_mdp": find(r"L9 MDP:\s*([^\n]+)", text),
    }


def parse_verifier(text: str) -> dict:
    return {
        "scenes": find(r"Scenes:\s*(\d+)", text, int),
        "frames": find(r"Frames:\s*(\d+)", text, int),
        "max_peak_pq": find(r"Max Peak PQ:\s*([0-9.]+)", text, float),
        "max_peak_nits": find(r"Max Peak PQ:\s*[0-9.]+\s*\(([0-9.]+)\s*nits\)", text, float),
        "avg_peak_pq": find(r"Avg Peak PQ:\s*([0-9.]+)", text, float),
        "avg_peak_nits": find(r"Avg Peak PQ:\s*[0-9.]+\s*\(([0-9.]+)\s*nits\)", text, float),
        "max_avg_pq": find(r"Max Avg PQ:\s*([0-9.]+)", text, float),
        "max_avg_nits": find(r"Max Avg PQ:\s*[0-9.]+\s*\(([0-9.]+)\s*nits\)", text, float),
        "avg_avg_pq": find(r"Avg Avg PQ:\s*([0-9.]+)", text, float),
        "avg_avg_nits": find(r"Avg Avg PQ:\s*[0-9.]+\s*\(([0-9.]+)\s*nits\)", text, float),
        "avg_target_nits": find(r"Average target nits:\s*([0-9.]+)", text, float),
    }


def cmv40_levels_from_frame0(path: str) -> set[str]:
    data = json.loads(read_text(path))
    blocks = (
        data.get("vdr_dm_data", {})
        .get("cmv40_metadata", {})
        .get("ext_metadata_blocks", [])
    )
    levels: set[str] = set()
    for block in blocks:
        if isinstance(block, dict):
            for k in block.keys():
                if isinstance(k, str) and k.startswith("Level"):
                    levels.add(k)
    return levels


def read_l8_flag(path: str) -> bool:
    return read_text(path).strip().lower() == "yes"


def pretty(v):
    return "n/a" if v is None else str(v)


orig_summary_text = read_text(orig_summary_path)
new_summary_text = read_text(new_summary_path)
verifier_text = read_text(verifier_path)

orig = parse_dv_summary(orig_summary_text)
new = parse_dv_summary(new_summary_text)
meas = parse_verifier(verifier_text)

orig_levels = cmv40_levels_from_frame0(orig_frame0_path)
new_levels = cmv40_levels_from_frame0(new_frame0_path)

if read_l8_flag(orig_l8_flag_path):
    orig_levels.add("Level8")
if read_l8_flag(new_l8_flag_path):
    new_levels.add("Level8")

tracked = ["Level8", "Level9", "Level11"]
tracked_label = {"Level8": "L8", "Level9": "L9", "Level11": "L11"}

orig_present = {lvl for lvl in tracked if lvl in orig_levels}
new_present = {lvl for lvl in tracked if lvl in new_levels}

missing_in_original = set(tracked) - orig_present
added_from_missing = sorted(missing_in_original & new_present)
still_missing_in_new = sorted(set(tracked) - new_present)

lines = []
lines.append("mkvdovi High-Fidelity Regeneration Report")
lines.append("=" * 42)
lines.append("")
lines.append(f"Input file:        {input_file}")
lines.append(f"Output file:       {output_file}")
lines.append(f"Measurements file: {measurements_file}")
lines.append("")
lines.append("[Original Dolby Vision metadata]")
lines.append(f"Frames:            {pretty(orig['frames'])}")
lines.append(f"Profile:           {pretty(orig['profile'])}")
lines.append(f"DM version:        {pretty(orig['dm_version'])}")
lines.append(f"Scene/shot count:  {pretty(orig['scene_count'])}")
lines.append(f"L1 MaxCLL (nits):  {pretty(orig['l1_maxcll'])}")
lines.append(f"L1 MaxFALL (nits): {pretty(orig['l1_maxfall'])}")
lines.append(f"L2 trims:          {pretty(orig['l2_trims'])}")
lines.append(f"L6 metadata:       {pretty(orig['l6'])}")
lines.append(f"L9 MDP:            {pretty(orig['l9_mdp'])}")
lines.append("")
lines.append("[New Dolby Vision metadata]")
lines.append(f"Frames:            {pretty(new['frames'])}")
lines.append(f"Profile:           {pretty(new['profile'])}")
lines.append(f"DM version:        {pretty(new['dm_version'])}")
lines.append(f"Scene/shot count:  {pretty(new['scene_count'])}")
lines.append(f"L1 MaxCLL (nits):  {pretty(new['l1_maxcll'])}")
lines.append(f"L1 MaxFALL (nits): {pretty(new['l1_maxfall'])}")
lines.append(f"L2 trims:          {pretty(new['l2_trims'])}")
lines.append(f"L6 metadata:       {pretty(new['l6'])}")
lines.append(f"L9 MDP:            {pretty(new['l9_mdp'])}")
lines.append("")
lines.append("[New high-fidelity measurements (verifier)]")
lines.append(f"Scenes:            {pretty(meas['scenes'])}")
lines.append(f"Frames:            {pretty(meas['frames'])}")
lines.append(f"Max Peak PQ:       {pretty(meas['max_peak_pq'])} ({pretty(meas['max_peak_nits'])} nits)")
lines.append(f"Avg Peak PQ:       {pretty(meas['avg_peak_pq'])} ({pretty(meas['avg_peak_nits'])} nits)")
lines.append(f"Max Avg PQ:        {pretty(meas['max_avg_pq'])} ({pretty(meas['max_avg_nits'])} nits)")
lines.append(f"Avg Avg PQ:        {pretty(meas['avg_avg_pq'])} ({pretty(meas['avg_avg_nits'])} nits)")
lines.append(f"Average target nits: {pretty(meas['avg_target_nits'])}")
lines.append("")
lines.append("[Comparison summary: original DV vs new high-fidelity measurements]")
if orig.get("frames") is not None and meas.get("frames") is not None:
    lines.append(
        f"Frame count delta (measurements - original DV): {meas['frames'] - orig['frames']:+d}"
    )
if orig.get("scene_count") is not None and meas.get("scenes") is not None:
    lines.append(
        f"Scene count delta (measurements - original DV): {meas['scenes'] - orig['scene_count']:+d}"
    )
if orig.get("l1_maxcll") is not None and meas.get("max_peak_nits") is not None:
    lines.append(
        f"Brightness reference (new max peak nits - original L1 MaxCLL): {meas['max_peak_nits'] - orig['l1_maxcll']:+.2f}"
    )
if orig.get("l1_maxfall") is not None and meas.get("max_avg_nits") is not None:
    lines.append(
        f"Brightness reference (new max avg nits - original L1 MaxFALL): {meas['max_avg_nits'] - orig['l1_maxfall']:+.2f}"
    )
lines.append("")
lines.append("[CM v4.0 levels check (L8/L9/L11)]")
lines.append(
    "Original present: "
    + (", ".join(tracked_label[l] for l in sorted(orig_present)) if orig_present else "none")
)
lines.append(
    "New present:      "
    + (", ".join(tracked_label[l] for l in sorted(new_present)) if new_present else "none")
)

if added_from_missing:
    lines.append(
        "Added in new (previously missing): "
        + ", ".join(tracked_label[l] for l in added_from_missing)
    )
else:
    lines.append("Added in new (previously missing): none")

if still_missing_in_new:
    lines.append(
        "Still missing in new: "
        + ", ".join(tracked_label[l] for l in still_missing_in_new)
    )
else:
    lines.append("Still missing in new: none")

report = "\n".join(lines) + "\n"
Path(report_path).write_text(report, encoding="utf-8")
print(report, end="")
PY

log "Workflow complete"
log "Comparison report written to: $REPORT_FILE"

if [[ "$KEEP_TEMP" -eq 1 ]]; then
    log "Temporary files kept at: $WORK_DIR"
fi
