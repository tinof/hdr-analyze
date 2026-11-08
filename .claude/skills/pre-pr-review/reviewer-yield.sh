#!/usr/bin/env bash
# reviewer-yield.sh: the observer for the non-Opus pre-PR reviewers. Reads the step 8 records in
# <git common dir>/pre-pr-review/*.json and answers, per reviewer, whether it catches defects that
# no Opus lens reports. No model runs: it counts what the records already hold.
#
# Usage: reviewer-yield.sh [--since YYYY-MM-DD] [--min-runs N] [--dir <records dir>]
#   --since     count records written on or after this date (file mtime); default 2026-11-08,
#               the day the Sonnet reviewer joined the panel and Fable's trial restarted.
#   --min-runs  runs a reviewer needs before it gets a verdict; default 10.
#   --dir       records directory; default <git common dir>/pre-pr-review.
#
# Per reviewer: runs (records whose `reviewers` list it with ran=true), findings it is a finder of,
# confirmed (verdict fixed or deferred), refuted or rejected, and beyond-Opus: confirmed findings
# that no Opus lens (correctness, contracts, tests-and-gates, spec, move-integrity) also reported,
# split by priority. Then each beyond-Opus finding (record, where, priority, all finders), so the
# main session can judge whether it mattered.
#
# Verdict, for the observed reviewers (sonnet, fable), once runs >= --min-runs:
#   keep                   at least one beyond-Opus finding at P0-P2
#   candidate for removal  none at P0-P2 (P3-only catches do not pay for a reviewer)
# Below --min-runs: "observing (k of N runs)". The owner decides; this script never edits the panel.
set -euo pipefail

since=2026-11-08
min_runs=10
dir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --since) since=${2:?--since needs a date}; shift 2 ;;
        --min-runs) min_runs=${2:?--min-runs needs a number}; shift 2 ;;
        --dir) dir=${2:?--dir needs a path}; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) echo "reviewer-yield.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$since" in
    [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
    *) echo "reviewer-yield.sh: --since must be YYYY-MM-DD, got: $since" >&2; exit 2 ;;
esac
case "$min_runs" in ''|*[!0-9]*) echo "reviewer-yield.sh: --min-runs must be a number" >&2; exit 2 ;; esac
if [ -z "$dir" ]; then
    dir="$(git rev-parse --path-format=absolute --git-common-dir)/pre-pr-review"
fi
command -v jq >/dev/null || { echo "reviewer-yield.sh: jq not found" >&2; exit 2; }

files=()
if [ -d "$dir" ]; then
    # -newermt takes the date as local midnight: records written on --since count.
    while IFS= read -r f; do files+=("$f"); done < <(find "$dir" -maxdepth 1 -name '*.json' -newermt "$since" | sort)
fi
echo "reviewer yield since $since: ${#files[@]} record(s) in $dir"
if [ "${#files[@]}" -eq 0 ]; then
    echo "no records yet; observing sonnet and fable."
    exit 0
fi

# Unreadable records are skipped and named, never silently dropped.
good=()
for f in "${files[@]}"; do
    if jq -e 'type == "object"' "$f" >/dev/null 2>&1; then good+=("$f"); else echo "skipped (not a JSON record): $f"; fi
done
[ "${#good[@]}" -gt 0 ] || exit 0

jq -rn --argjson min "$min_runs" '
  def opus: ["correctness", "contracts", "tests-and-gates", "spec", "move-integrity"];
  def confirmed: .verdict == "fixed" or .verdict == "deferred";
  [inputs | {head: ((.head // input_filename) | tostring | .[0:8]), reviewers: (.reviewers // []), findings: (.findings // [])}] as $recs
  | ([$recs[].findings[].finders[]?] + [$recs[].reviewers[] | select(.ran == true) | .key] | unique) as $names
  | ($names | map(. as $n | {
      name: $n,
      runs: ([$recs[] | select(any(.reviewers[]; .key == $n and .ran == true))] | length),
      found: ([$recs[].findings[] | select((.finders // []) | index($n))]),
    } | . + {
      confirmed: ([.found[] | select(confirmed)] | length),
      refuted: ([.found[] | select(.verdict == "refuted" or .verdict == "rejected")] | length),
      beyond: ([.found[] | select(confirmed and ((.finders // []) - opus | length) == ((.finders // []) | length))]),
    })) as $rows
  | "reviewer        runs  findings  confirmed  refuted  beyond-Opus (P0/P1/P2/P3)",
    ($rows[] | "\(.name | . + (" " * (16 - length)))\(.runs | tostring | (" " * (4 - length)) + .)  \(.found | length | tostring | (" " * (8 - length)) + .)  \(.confirmed | tostring | (" " * (9 - length)) + .)  \(.refuted | tostring | (" " * (7 - length)) + .)  \(.beyond | length) (\([.beyond[] | select(.priority == "P0")] | length)/\([.beyond[] | select(.priority == "P1")] | length)/\([.beyond[] | select(.priority == "P2")] | length)/\([.beyond[] | select(.priority == "P3")] | length))"),
    "",
    "beyond-Opus findings:",
    ([$recs[] | .head as $h | .findings[] | select(confirmed and ((.finders // []) - opus | length) == ((.finders // []) | length)) | "  \($h) \(.priority // "?") \(.where // "?") [\((.finders // []) | join(", "))]"] | if length == 0 then ["  none"] else . end | .[]),
    "",
    "verdicts:",
    (["sonnet", "fable"][] as $n
      | ($rows | map(select(.name == $n)) | .[0] // {runs: 0, beyond: []}) as $r
      | if $r.runs < $min then "  \($n): observing (\($r.runs) of \($min) runs)"
        elif ([$r.beyond[] | select(.priority == "P0" or .priority == "P1" or .priority == "P2")] | length) > 0 then "  \($n): keep (\($r.beyond | length) beyond-Opus finding(s) in \($r.runs) runs)"
        else "  \($n): candidate for removal (no beyond-Opus finding at P0-P2 in \($r.runs) runs)" end)
' "${good[@]}"
