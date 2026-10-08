#!/usr/bin/env bash
# Gemini reviewer for the pre-PR panel, through the Antigravity CLI (agy) in read-only print mode,
# under an idle watchdog with one retry.
#
# Usage: pre-pr-gemini.sh <base> <out-dir> <scope-file>
#
# Writes <out-dir>/prepr-gemini.result.json and prints it:
#   {"reviewer":"gemini","model":..,"agent":..,"seconds":..,"ok":true,"findings":[..],"attempts":[..],"usage":{..}}
#   {"reviewer":"gemini","model":..,"agent":..,"seconds":..,"ok":false,"reason":"..","attempts":[..],"usage":{..}}
# Per attempt N: prepr-gemini.a<N>.{diff,message.ndjson,stream.ndjson,err,agy.log}.
# Always exits 0: a failed Gemini pass is reported, never a blocker.
#
# Print mode denies every action that needs a confirmation (a terminal command, a file write, a read
# outside the workspace) and then ends the turn with status SUCCESS and no answer (2026-10-07: a
# view_file of a ~/.claude path the diff named). So the result is checked here, not by an agent:
# status, schema, no denied action, and the nonce from the diff's first line, which Gemini can only
# echo if it read the diff. A denied attempt is retried once with the denied path or tool named;
# every attempt is a new conversation with a new nonce, and no prompt line ever carries a nonce.
#
# The diff goes inside the prompt, on stdin (stream-json): letting Gemini fetch it costs one model
# round trip per 800-line read and resends the whole conversation each time (2026-10-07, P8 diff:
# 88 calls and 610 s fetching, against 17 calls and 248 s inline at medium). Medium, not high:
# high spent 45k thinking tokens (about 5 min) on that diff before answering.
#
# Watchdog: activity is the newest write to the attempt's stream, its stderr, or the conversation's
# SQLite store (<agy home>/conversations/<id>.db and its -wal). agy emits no stream event while the
# model thinks (2026-10-08: 222 s in a review, 341 s in a probe); in that probe the WAL was written at
# least every 19 s. If a hang kept writing the WAL, it would end at the total limit instead, as
# before this watchdog. The idle limit is 480 s. The repo agent .agents/agents/pre-pr-reviewer.md limits the tools to
# view_file, grep_search and finish (no terminal, no writes, no web); it is used when present.
# Every attempt runs with PRE_PR_GEMINI_ATTEMPT=<token>.<N> in its environment, and cleanup kills
# each process that carries it, so a child that left agy's process group dies too.
#
# Environment: PRE_PR_GEMINI_MODEL (gemini-3.8-flash-medium), PRE_PR_GEMINI_TIMEOUT (s for all
# attempts, 1200), PRE_PR_GEMINI_IDLE (s, 480), PRE_PR_GEMINI_RETRIES (1), PRE_PR_GEMINI_RETRY_FLOOR
# (s that must remain for a retry, 300), PRE_PR_GEMINI_POLL (s, 10), PRE_PR_GEMINI_KILL_GRACE
# (s between TERM and KILL, 10), PRE_PR_GEMINI_AGY_HOME (~/.gemini/antigravity-cli).
set -u

base=${1:?base}
out=${2:?out-dir}
scope=${3:?scope-file}
model=${PRE_PR_GEMINI_MODEL:-gemini-3.8-flash-medium}
limit=${PRE_PR_GEMINI_TIMEOUT:-1200}
idle=${PRE_PR_GEMINI_IDLE:-480}
retries=${PRE_PR_GEMINI_RETRIES:-1}
floor=${PRE_PR_GEMINI_RETRY_FLOOR:-300}
poll=${PRE_PR_GEMINI_POLL:-10}
grace=${PRE_PR_GEMINI_KILL_GRACE:-10}
agent_name=pre-pr-reviewer
agy_home=${PRE_PR_GEMINI_AGY_HOME:-$HOME/.gemini/antigravity-cli}

here=$(cd "$(dirname "$0")" && pwd)
root=$(git rev-parse --show-toplevel) || exit 0
mkdir -p "$out"
result="$out/prepr-gemini.result.json"
rm -f -- "$out"/prepr-gemini.*

started=$(date +%s)
deadline=$((started + limit))
token=$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')
attempts='[]'
agent=null
[ -f "$root/.agents/agents/$agent_name.md" ] && agent="\"$agent_name\""
marker="" pid=""

# Writes the result from attempts plus $1, a JSON object with ok and findings or reason, and exits 0.
finish() {
    jq -n --arg model "$model" --argjson agent "$agent" --argjson seconds "$(( $(date +%s) - started ))" \
        --argjson attempts "$attempts" --argjson tail "$1" '
      {reviewer: "gemini", model: $model, agent: $agent, seconds: $seconds} + $tail
      + {attempts: $attempts,
         usage: {thinking_tokens: ($attempts | map(.thinking_tokens) | add // 0),
                 output_tokens: ($attempts | map(.output_tokens) | add // 0),
                 input_tokens: ($attempts | map(.input_tokens) | add // 0),
                 partial: ($attempts | any(.usage_partial))}}' > "$result"
    trap - EXIT INT TERM HUP
    command cat "$result"
    exit 0
}

fail() {
    finish "$(jq -nc --arg reason "$1" '{ok: false, reason: $reason}')"
}

# PIDs of live processes that carry attempt marker $1 in their environment.
marked() {
    grep -l -s -z -x -F "PRE_PR_GEMINI_ATTEMPT=$1" /proc/[0-9]*/environ 2> /dev/null | cut -d/ -f3
}

# Stops attempt $2: TERM to its process group and every marked process, KILL after the grace
# period, then KILL whatever still carries the marker (a child may fork on TERM) until none is left.
stop_attempt() {
    local p=$1 m=$2 i pids
    [ -n "$m" ] || return 0
    pids=$(marked "$m")
    [ -n "$p" ] && kill -TERM -- "-$p" 2> /dev/null
    # shellcheck disable=SC2086 # a list of PIDs
    [ -n "$pids" ] && kill -TERM $pids 2> /dev/null
    for ((i = 0; i < grace * 4; i++)); do
        [ -n "$(marked "$m")" ] || { [ -n "$p" ] && kill -0 "$p" 2> /dev/null; } || break
        sleep 0.25
    done
    for i in 1 2 3 4 5 6 7 8; do
        [ -n "$p" ] && kill -KILL -- "-$p" 2> /dev/null
        pids=$(marked "$m")
        [ -n "$pids" ] || break
        # shellcheck disable=SC2086 # a list of PIDs
        kill -KILL $pids 2> /dev/null
        sleep 0.25
    done
}

on_signal() {
    trap - INT TERM HUP
    local s=$(( $(date +%s) - ${attempt_start:-$started} ))
    stop_attempt "$pid" "$marker"
    if [ -n "$marker" ]; then
        attempts=$(jq -c --argjson n "$n" --argjson s "$s" --arg sig "$1" \
            '. + [{attempt: $n, outcome: "interrupted", reason: "wrapper got SIG\($sig)", seconds: $s, rc: null,
                   conversation_id: "", steps: 0, thinking_tokens: 0, output_tokens: 0, input_tokens: 0,
                   usage_partial: true, denied_path: null, denied_tool: null}]' <<< "$attempts")
    fi
    marker=""
    fail "interrupted by SIG$1"
}
trap 'stop_attempt "$pid" "$marker"' EXIT
trap 'on_signal INT' INT
trap 'on_signal TERM' TERM
trap 'on_signal HUP' HUP

newest_mtime() {
    local m=0 t f
    for f in "$@"; do
        [ -e "$f" ] || continue
        t=$(stat -c %Y "$f")
        [ "$t" -gt "$m" ] && m=$t
    done
    echo "$m"
}

command -v agy > /dev/null || fail "agy (Antigravity CLI) not found on PATH"
[ -r "$scope" ] || fail "scope file not readable: $scope"

body="$out/prepr-gemini.diff-body"
git -C "$root" diff "$base...HEAD" > "$body" || fail "git diff $base...HEAD failed"
[ -s "$body" ] || fail "empty diff for $base...HEAD"
prompt="$out/prepr-gemini.prompt.md"
sed -e "s|{{SCOPE}}|$scope|g" "$here/pre-pr-gemini.prompt.md" > "$prompt" || fail "could not build the prompt"

agent_args=()
[ "$agent" = null ] || agent_args=(--agent "$agent_name")
note=""
n=0
while :; do
    n=$((n + 1))
    a="$out/prepr-gemini.a$n"
    nonce=$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')
    { echo "REVIEW-NONCE: $nonce"; command cat "$body"; } > "$a.diff"
    jq -nc --rawfile p "$prompt" --arg note "$note" --rawfile d "$a.diff" \
        '{event: "user", message: {content: ($p + (if $note == "" then "" else "\n" + $note + "\n" end)
                                             + "\n=== DIFF START ===\n" + $d + "\n=== DIFF END ===\n")}}' \
        > "$a.message.ndjson" || fail "could not build the message"

    marker="$token.$n"
    attempt_start=$(date +%s)
    # setsid: its own process group, so the group kill reaches agy's children; the marker reaches
    # the ones that start their own session.
    (cd "$root" && exec env PRE_PR_GEMINI_ATTEMPT="$marker" setsid agy --input-format stream-json \
        --output-format stream-json --model "$model" --mode plan --sandbox "${agent_args[@]}" \
        --add-dir "$(dirname "$scope")" --json-schema "$here/pre-pr-gemini.schema.json" \
        --log-file "$a.agy.log") < "$a.message.ndjson" > "$a.stream.ndjson" 2> "$a.err" &
    pid=$!
    killed="" conv=""
    while kill -0 "$pid" 2> /dev/null; do
        sleep "$poll" &
        wait $!
        [ -n "$conv" ] || conv=$(jq -R -r 'fromjson? | select(.event == "init") | .conversation_id // empty' \
            "$a.stream.ndjson" 2> /dev/null | head -n 1)
        now=$(date +%s)
        last=$(newest_mtime "$a.stream.ndjson" "$a.err" ${conv:+"$agy_home/conversations/$conv.db"} \
            ${conv:+"$agy_home/conversations/$conv.db-wal"})
        [ "$last" -ge "$attempt_start" ] || last=$attempt_start
        if [ $((now - last)) -ge "$idle" ]; then
            killed="stalled:no output for $((now - last)) s (idle limit ${idle} s)"
        elif [ "$now" -ge "$deadline" ]; then
            killed="timeout:total time limit ${limit} s reached"
        fi
        if [ -n "$killed" ]; then
            stop_attempt "$pid" "$marker"
            break
        fi
    done
    wait "$pid" 2> /dev/null
    rc=$?
    stop_attempt "" "$marker"
    marker="" pid=""
    seconds=$(( $(date +%s) - attempt_start ))

    agy_error=$(grep -a -o 'AGY_ERROR: .*' "$a.err" 2> /dev/null | tail -n 1 | cut -c 12-)
    error_line=$(grep -a -m 1 '^error:' "$a.err" 2> /dev/null | cut -c 1-300)
    soft_deny=$(grep -a -o 'soft-denying tool confirmation "[^"]*" at step [0-9]*' "$a.agy.log" 2> /dev/null | tail -n 1)
    verdict=$(jq -R -n -c --arg nonce "$nonce" --argjson rc "$rc" --arg killed "$killed" \
        --arg agy_error "$agy_error" --arg error_line "$error_line" --arg soft_deny "$soft_deny" '
      def valid_item:
        type == "object"
        and (.priority | IN("P0", "P1", "P2", "P3"))
        and (.file | type == "string")
        and (.line | type == "number" and . == floor)
        and (.summary | type == "string")
        and (.scenario | type == "string");
      # Quota, rate-limit and auth failures: a retry cannot fix them.
      def permanent: test("RESOURCE_EXHAUSTED|quota|rate.?limit|UNAUTHENTICATED|PERMISSION_DENIED|unauthori[sz]ed|not logged in|credits"; "i");
      [inputs | fromjson? | objects] as $ev
      | ($ev | map(select(.event == "result") | .result | objects) | last) as $r
      | ($ev | map(select(.event == "step_update") | .step_update | objects)) as $su
      | (($ev | map(select(.event == "init") | .conversation_id) | first) // $r.conversation_id // "") as $conv
      | ($su | map(select(.state == "DONE" or .state == "ERROR") | .step_index) | unique) as $ended
      | ($su | map(select(.step_type == "tool" and .state == "ACTIVE"))) as $opened
      | ([$soft_deny | capture("at step (?<i>[0-9]+)$") | .i | tonumber] | first) as $deny_step
      | ([$soft_deny | capture("confirmation \"(?<t>[^\"]*)\"") | .t] | first) as $deny_tool_log
      | (if $deny_step then ($opened | map(select(.step_index == $deny_step)) | last)
         else ($opened | map(select(.step_index as $i | $ended | index($i) | not)) | last) end) as $deny_step_ev
      | (($r.denied_actions // []) | if type == "array" then . else [] end) as $da
      | ($deny_step_ev.tool_info.parameters // {} | .AbsolutePath // .SearchPath // .DirectoryPath // null) as $deny_path
      | ($da[0].action // $deny_step_ev.tool_name // $deny_tool_log // null) as $deny_tool
      | ($agy_error | fromjson? // null) as $agy_json
      | ([$agy_json | .. | objects | to_entries[] | select((.key | test("retry"; "i")) and (.value | type == "boolean")) | .value] | first) as $agy_retryable
      | (($agy_error + " " + ($r.error // "" | tostring))) as $error_text
      | (if $r and ($r.usage | type == "object") then {u: $r.usage, partial: false}
         else {u: ($su | map(select(.state == "DONE") | .usage // empty) | {thinking_tokens: (map(.thinking_tokens // 0) | add // 0),
                    output_tokens: (map(.output_tokens // 0) | add // 0), input_tokens: (map(.input_tokens // 0) | add // 0)}),
               partial: true} end) as $usage
      | (if $killed != "" then {outcome: ($killed | split(":")[0]), reason: ($killed | split(":")[1:] | join(":")),
                                retryable: (($killed | startswith("stalled")))}
         elif ($da | length) > 0 or $deny_step != null then
           {outcome: "denied", reason: "agy denied \($deny_tool // "an action")\(if $deny_path then " (\($deny_path))" else "" end); the run ended early",
            retryable: true}
         elif $agy_error != "" then
           {outcome: "agy-error", reason: ("AGY_ERROR " + ($agy_error | .[0:300])),
            retryable: (if $agy_retryable != null then $agy_retryable and ($error_text | permanent | not)
                        else ($error_text | permanent | not) end)}
         elif $r and $r.status == "ERROR" and ($r.num_turns // 0) == 0 and $conv == "" then
           {outcome: "agy-error", reason: ("agy failed at startup: " + ($r.error // $error_line | tostring | .[0:300])), retryable: false}
         elif ($rc != 0 and $rc != 3) then
           {outcome: "agy-error", reason: ("agy exited \($rc)" + (if $error_line != "" then ": " + $error_line else "" end)),
            retryable: ($error_text + " " + $error_line | permanent | not)}
         elif $r == null then {outcome: "no-result", reason: "no result event (exit \($rc))", retryable: true}
         elif $r.status != "SUCCESS" then
           {outcome: "status", reason: "agy status \($r.status)\(if $r.error then ": " + ($r.error | tostring | .[0:300]) else "" end)",
            retryable: ($error_text | permanent | not)}
         elif $r.structured_output == null then {outcome: "malformed", reason: "no structured_output", retryable: true}
         elif $r.structured_output.nonce != $nonce then {outcome: "nonce", reason: "nonce mismatch: the diff was not read", retryable: true}
         elif ($r.structured_output.findings | type) != "array" then {outcome: "malformed", reason: "findings is not an array", retryable: true}
         elif ($r.structured_output.findings | all(valid_item) | not) then {outcome: "malformed", reason: "malformed finding", retryable: true}
         else {outcome: "completed", reason: "", retryable: false, findings: $r.structured_output.findings}
         end)
      + {conversation_id: $conv, steps: ($ended | length),
         thinking_tokens: ($usage.u.thinking_tokens // 0), output_tokens: ($usage.u.output_tokens // 0),
         input_tokens: ($usage.u.input_tokens // 0), usage_partial: $usage.partial,
         denied_path: (if ($da | length) > 0 or $deny_step != null then $deny_path else null end),
         denied_tool: (if ($da | length) > 0 or $deny_step != null then $deny_tool else null end)}
    ' < "$a.stream.ndjson") || verdict='{"outcome":"malformed","reason":"result check failed","retryable":true,"conversation_id":"","steps":0,"thinking_tokens":0,"output_tokens":0,"input_tokens":0,"usage_partial":true,"denied_path":null,"denied_tool":null}'

    attempts=$(jq -c --argjson n "$n" --argjson s "$seconds" --argjson rc "$rc" --argjson v "$verdict" \
        '. + [{attempt: $n, seconds: $s, rc: $rc} + ($v | del(.findings, .retryable))]' <<< "$attempts")
    outcome=$(jq -r .outcome <<< "$verdict")
    reason=$(jq -r .reason <<< "$verdict")
    [ "$outcome" = completed ] && finish "$(jq -c '{ok: true, findings: .findings}' <<< "$verdict")"

    left=$((deadline - $(date +%s)))
    if [ "$(jq -r .retryable <<< "$verdict")" != true ]; then
        fail "$reason (not retried: $( [ "$outcome" = timeout ] && echo "no time left" || echo "a retry cannot fix it"))"
    elif [ "$n" -gt "$retries" ]; then
        fail "$reason (not retried: $retries retr$( [ "$retries" = 1 ] && echo y || echo ies) used)"
    elif [ "$left" -lt "$floor" ]; then
        fail "$reason (not retried: ${left} s left, a retry needs ${floor} s)"
    fi
    if [ "$outcome" = denied ]; then
        path=$(jq -r '.denied_path // empty' <<< "$verdict")
        tool=$(jq -r '.denied_tool // empty' <<< "$verdict")
        if [ -n "$path" ]; then
            note="Note from an earlier attempt: reading \`$path\` was denied because it lies outside the workspace, and the denial ended that review. Do not read it again; judge it from the diff."
        else
            note="Note from an earlier attempt: the tool \`${tool:-unknown}\` was denied, and the denial ended that review. Do not call it."
        fi
    fi
done
