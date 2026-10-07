#!/usr/bin/env bash
# Gemini reviewer for the pre-PR panel, through the Antigravity CLI (agy) in read-only print mode.
#
# Usage: pre-pr-gemini.sh <base> <out-dir> <scope-file>
#
# Writes <out-dir>/prepr-gemini.result.json and prints it:
#   {"reviewer":"gemini","model":..,"seconds":..,"ok":true,"findings":[..]}
#   {"reviewer":"gemini","model":..,"seconds":..,"ok":false,"reason":".."}
# Always exits 0: a failed Gemini pass is reported, never a blocker.
#
# Print mode denies every action that needs a confirmation (any terminal command, any file write)
# and then ends the run, so a denied action means an incomplete review. The result is checked here,
# not by an agent: status, schema, no denied actions, and the nonce from the diff's first line,
# which Gemini can only echo if it read the diff.
#
# The diff goes inside the prompt, on stdin (stream-json): letting Gemini fetch it costs one model
# round trip per 800-line read and resends the whole conversation each time (2026-10-07, P8 diff:
# 88 calls and 610 s fetching, against 17 calls and 248 s inline at medium). Medium, not high:
# high spent 45k thinking tokens (about 5 min) on that diff before answering.
set -u

base=${1:?base}
out=${2:?out-dir}
scope=${3:?scope-file}
model=${PRE_PR_GEMINI_MODEL:-gemini-3.8-flash-medium}
limit=${PRE_PR_GEMINI_TIMEOUT:-900}

here=$(cd "$(dirname "$0")" && pwd)
root=$(git rev-parse --show-toplevel) || exit 0
mkdir -p "$out"
result="$out/prepr-gemini.result.json"
rm -f -- "$result"

fail() {
    jq -n --arg model "$model" --argjson seconds "${2:-0}" --arg reason "$1" \
        '{reviewer: "gemini", model: $model, seconds: $seconds, ok: false, reason: $reason}' > "$result"
    command cat "$result"
    exit 0
}

command -v agy > /dev/null || fail "agy (Antigravity CLI) not found on PATH"
[ -r "$scope" ] || fail "scope file not readable: $scope"

nonce=$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')
diff="$out/prepr-gemini.diff"
{ echo "REVIEW-NONCE: $nonce"; git -C "$root" diff "$base...HEAD"; } > "$diff" || fail "git diff $base...HEAD failed"
[ "$(wc -l < "$diff")" -gt 1 ] || fail "empty diff for $base...HEAD"

prompt="$out/prepr-gemini.prompt.md"
sed -e "s|{{SCOPE}}|$scope|g" "$here/pre-pr-gemini.prompt.md" > "$prompt"
message="$out/prepr-gemini.message.ndjson"
jq -nc --rawfile p "$prompt" --rawfile d "$diff" \
    '{event: "user", message: {content: ($p + "\n=== DIFF START ===\n" + $d + "\n=== DIFF END ===\n")}}' \
    > "$message" || fail "could not build the prompt"

stream="$out/prepr-gemini.stream.ndjson"
raw="$out/prepr-gemini.raw.json"
start=$(date +%s)
(cd "$root" && timeout --kill-after=30s "$limit" agy --input-format stream-json --output-format stream-json \
    --model "$model" --mode plan --sandbox --add-dir "$(dirname "$scope")" \
    --json-schema "$here/pre-pr-gemini.schema.json") < "$message" > "$stream" 2> "$out/prepr-gemini.err"
rc=$?
seconds=$(( $(date +%s) - start ))

[ "$rc" -eq 124 ] || [ "$rc" -eq 137 ] && fail "timed out after ${limit}s" "$seconds"
jq -c 'select(.event == "result") | .result' "$stream" 2> /dev/null | tail -n 1 > "$raw"
jq -e . "$raw" > /dev/null 2>&1 || fail "no result event (exit $rc): $(head -c 300 "$out/prepr-gemini.err")" "$seconds"

jq --arg nonce "$nonce" --arg model "$model" --argjson seconds "$seconds" '
  def valid_item:
    type == "object"
    and (.priority | IN("P0", "P1", "P2", "P3"))
    and (.file | type == "string")
    and (.line | type == "number" and . == floor)
    and (.summary | type == "string")
    and (.scenario | type == "string");
  (if .status != "SUCCESS" then {ok: false, reason: "agy status \(.status)"}
   elif ((.denied_actions // []) | length) > 0 then
     {ok: false, reason: "agy denied \(.denied_actions | map(.action) | join(", ")); the run ended early"}
   elif .structured_output == null then {ok: false, reason: "no structured_output"}
   elif .structured_output.nonce != $nonce then {ok: false, reason: "nonce mismatch: the diff was not read"}
   elif (.structured_output.findings | type) != "array" then {ok: false, reason: "findings is not an array"}
   elif (.structured_output.findings | all(valid_item) | not) then {ok: false, reason: "malformed finding"}
   else {ok: true, findings: .structured_output.findings}
   end) + {reviewer: "gemini", model: $model, seconds: $seconds}
' "$raw" > "$result" || fail "result check failed" "$seconds"
command cat "$result"
