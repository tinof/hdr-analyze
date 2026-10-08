#!/usr/bin/env bash
# check_sh evaluates its test later, so single quotes and variables used only there are intended.
# shellcheck disable=SC2016,SC2034
# Tests for ../pre-pr-gemini.sh against a fake agy (fake-agy, same directory). No network, no
# model: each case runs the real script in a throwaway git repo with small time limits.
#
# Usage: .claude/workflows/tests/pre-pr-gemini-test.sh [case-name-substring]
# Exits 0 when every selected case passes.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../pre-pr-gemini.sh"
only=${1:-}
work=$(mktemp -d "${TMPDIR:-/tmp}/pre-pr-gemini-test.XXXXXX")
trap 'rm -rf "$work"' EXIT

# Two commits in a scratch repo: HEAD~1...HEAD has a diff, HEAD...HEAD has none.
repo="$work/repo"
git init -q "$repo"
git -C "$repo" -c user.email=t@t -c user.name=t commit -q --allow-empty -m base
echo 'fn main() {}' > "$repo/a.rs"
git -C "$repo" add a.rs
git -C "$repo" -c user.email=t@t -c user.name=t commit -q -m change
echo "scope" > "$work/scope.md"

bin="$work/bin" nobin="$work/nobin"
mkdir -p "$bin" "$nobin"
ln -s "$here/fake-agy" "$bin/agy"
for t in jq git; do
    p=$(type -P "$t") || { echo "missing $t" >&2; exit 2; }
    ln -s "$p" "$bin/$t"
    ln -s "$p" "$nobin/$t"
done

pass=0 failed=0
# run_case <name> <plan> [VAR=value ...]: runs the script; leaves $out, $res and $fake set.
run_case() {
    name=$1 plan=$2
    shift 2
    out="$work/$name/out" fake="$work/$name/fake"
    mkdir -p "$out" "$fake"
    res="$out/prepr-gemini.result.json"
    (cd "$repo" && env PATH="$bin:/usr/bin:/bin" FAKE_DIR="$fake" FAKE_AGY_PLAN="$plan" \
        FAKE_AGY_HOME="$work/$name/agyhome" PRE_PR_GEMINI_AGY_HOME="$work/$name/agyhome" \
        PRE_PR_GEMINI_IDLE=3 PRE_PR_GEMINI_POLL=1 PRE_PR_GEMINI_KILL_GRACE=1 \
        PRE_PR_GEMINI_RETRY_FLOOR=1 PRE_PR_GEMINI_TIMEOUT=60 "$@" \
        "$script" "${BASE:-HEAD~1}" "$out" "$work/scope.md") > "$out/stdout" 2> "$out/stderr"
    rc=$?
}

# check <description> <jq expression on the result>
check() {
    if [ "$rc" -eq 0 ] && jq -e "$2" "$res" > /dev/null 2>&1; then
        pass=$((pass + 1))
    else
        failed=$((failed + 1))
        echo "FAIL [$name] $1 (exit $rc)"
        jq -c . "$res" 2> /dev/null | cut -c 1-600 || echo "  no result file"
    fi
}

# check_sh <description> <shell test>
check_sh() {
    if eval "$2"; then pass=$((pass + 1)); else failed=$((failed + 1)); echo "FAIL [$name] $1"; fi
}

# No process may still carry any marker this case's attempts ran with.
no_leftovers() {
    local m left=""
    for m in "$fake"/a*.marker; do
        [ -e "$m" ] || continue
        left+=$(grep -l -s -z -x -F "PRE_PR_GEMINI_ATTEMPT=$(command cat "$m")" /proc/[0-9]*/environ 2> /dev/null)
    done
    check_sh "no process left with an attempt marker" '[ -z "$left" ]'
}

want() { [ -z "$only" ] || [[ $1 == *"$only"* ]]; }

if want ok; then
    run_case ok ok
    check "one completed attempt with findings" \
        '.ok == true and (.findings | length) == 1 and (.attempts | length) == 1 and .attempts[0].outcome == "completed"'
    check "a finish step is not a denial" '.attempts[0].denied_tool == null and .attempts[0].denied_path == null'
    check "usage from the result, complete" '.usage.thinking_tokens == 45 and .usage.partial == false and .attempts[0].steps == 5'
    check "no agent file in this repo" '.agent == null'
    check_sh "the stdout is the result" 'cmp -s "$out/stdout" "$res"'
    check_sh "print mode stays read-only" 'grep -qx -- --sandbox "$fake/a1.args" && grep -qx plan "$fake/a1.args"'
    check_sh "agy runs under the timeout backstop" '[ "$(command cat "$fake/a1.parent")" = timeout ]'
fi

if want agent; then
    mkdir -p "$repo/.agents/agents" && echo x > "$repo/.agents/agents/pre-pr-reviewer.md"
    run_case agent ok
    rm -rf "$repo/.agents"
    check "agent recorded" '.ok == true and .agent == "pre-pr-reviewer"'
    check_sh "--agent passed" 'grep -qx -- --agent "$fake/a1.args" && grep -qx pre-pr-reviewer "$fake/a1.args"'
fi

if want thinkwal; then
    run_case thinkwal thinkwal
    check "conversation store writes count as activity" '.ok == true and (.attempts | length) == 1'
fi

if want stale; then
    mkdir -p "$work/stale/out"
    echo junk > "$work/stale/out/prepr-gemini.a3.stream.ndjson"
    echo keep > "$work/stale/out/prepr-scope.md"
    run_case stale ok
    check_sh "stale attempt files removed" '[ ! -e "$out/prepr-gemini.a3.stream.ndjson" ]'
    check_sh "other files kept" '[ -e "$out/prepr-scope.md" ]'
fi

if want hang; then
    run_case hang hang,ok
    check "stalled, then a retry succeeds" \
        '.ok == true and .attempts[0].outcome == "stalled" and .attempts[0].usage_partial == true and .attempts[1].outcome == "completed"'
    check "partial usage flagged" '.usage.partial == true'
    no_leftovers

    run_case hangterm hangterm,ok
    check "TERM ignored: KILL ends it" '.ok == true and .attempts[0].outcome == "stalled"'
    no_leftovers

    run_case hangescape hangescape,ok
    check "escaped child: still retried" '.ok == true and .attempts[0].outcome == "stalled"'
    no_leftovers
fi

if want escape; then
    run_case escape escape
    check "completed" '.ok == true'
    no_leftovers

    run_case forkterm forkterm
    check "completed" '.ok == true'
    no_leftovers
fi

if want deny; then
    run_case deny deny,ok
    check "denied, then a retry succeeds" \
        '.ok == true and .attempts[0].outcome == "denied" and .attempts[0].denied_path == "/home/someone/.claude/skills/x/watch.sh" and .attempts[0].denied_tool == "read_file"'
    check "usage of the denied attempt" '.attempts[0].thinking_tokens == 45'
    n1=$(grep -o 'REVIEW-NONCE: [0-9a-f]\{24\}' "$fake/a1.stdin" | cut -d' ' -f2)
    n2=$(grep -o 'REVIEW-NONCE: [0-9a-f]\{24\}' "$fake/a2.stdin" | cut -d' ' -f2)
    note=$(jq -r '.message.content' "$fake/a2.stdin" | grep 'Note from an earlier attempt')
    check_sh "the retry note names the path" '[[ $note == *"/home/someone/.claude/skills/x/watch.sh"* ]]'
    check_sh "the note carries no nonce" '[ -n "$n1" ] && [ -n "$n2" ] && [[ $note != *"$n1"* ]] && [[ $note != *"$n2"* ]]'
    check_sh "a new nonce per attempt" '[ "$n1" != "$n2" ]'
    check_sh "attempt 1 nonce absent from attempt 2" '! grep -q "$n1" "$fake/a2.stdin"'
    check_sh "attempt 2 nonce only on its diff line" \
        '[ "$(jq -r .message.content "$fake/a2.stdin" | grep -c "$n2")" = 1 ] && jq -r .message.content "$fake/a2.stdin" | grep -qx "REVIEW-NONCE: $n2"'

    run_case denycmd denycmd,ok
    check "command denial" '.ok == true and .attempts[0].outcome == "denied" and .attempts[0].denied_path == null and .attempts[0].denied_tool == "run_command"'
    check_sh "the note names the tool" 'jq -r .message.content "$fake/a2.stdin" | grep -q "the tool \`run_command\` was denied"'

    run_case denydeny deny,deny
    check "retries used up" '.ok == false and (.attempts | length) == 2 and (.reason | test("denied.*not retried: 1 retry used"))'
fi

if want noresult; then
    run_case noresult noresult,ok
    check "retryable AGY_ERROR: retry" '.ok == true and .attempts[0].outcome == "agy-error" and .attempts[0].rc == 3'
    run_case noresult0 noresult0,ok
    check "no result, cut-off stream: retry" '.ok == true and .attempts[0].outcome == "no-result" and .attempts[0].usage_partial == true'
fi

if want retry; then
    for p in malformed badnonce cancelled weirdrc; do
        run_case "retry-$p" "$p,ok"
        check "$p is retried" '.ok == true and (.attempts | length) == 2'
    done
    check "exit 7 is an agy error" '.attempts[0].outcome == "agy-error" and .attempts[0].rc == 7'
    run_case retry-badnonce-only badnonce
    check "nonce mismatch reported" '.ok == false and (.reason | test("nonce mismatch"))'
fi

if want fatal; then
    run_case fatal fatal,ok
    check "non-retryable AGY_ERROR: one attempt" '.ok == false and (.attempts | length) == 1 and (.reason | test("not retried: a retry cannot fix it"))'
    run_case fatal3 fatal3,ok
    check "exit 3 without result, non-retryable: agy-error, one attempt" \
        '.ok == false and (.attempts | length) == 1 and .attempts[0].outcome == "agy-error"'
    run_case startup startup,ok
    check "startup error: one attempt" '.ok == false and (.attempts | length) == 1 and (.reason | test("startup"))'
fi

if want budget; then
    run_case floor hang,ok PRE_PR_GEMINI_RETRY_FLOOR=100
    check "below the retry floor: no retry" '.ok == false and (.attempts | length) == 1 and (.reason | test("s left, a retry needs 100 s"))'
    # agy finishes during the poll sleep that crosses the deadline: its result must count.
    run_case lastpoll ok PRE_PR_GEMINI_TIMEOUT=4 PRE_PR_GEMINI_POLL=5
    check "a run that ended before the wakeup is not a timeout" '.ok == true and .attempts[0].outcome == "completed"'
    run_case budget hang,hang PRE_PR_GEMINI_TIMEOUT=9 PRE_PR_GEMINI_IDLE=5
    check "second attempt gets only the remaining time" \
        '.ok == false and (.attempts | length) == 2 and .attempts[1].outcome == "timeout" and .seconds <= 12'
    no_leftovers
fi

if want signal; then
    name=signal out="$work/signal/out" fake="$work/signal/fake"
    mkdir -p "$out" "$fake"
    res="$out/prepr-gemini.result.json"
    (cd "$repo" && exec env PATH="$bin:/usr/bin:/bin" FAKE_DIR="$fake" FAKE_AGY_PLAN=hangescape,ok \
        PRE_PR_GEMINI_AGY_HOME="$work/signal/agyhome" PRE_PR_GEMINI_IDLE=100 PRE_PR_GEMINI_POLL=1 \
        PRE_PR_GEMINI_KILL_GRACE=1 "$script" HEAD~1 "$out" "$work/scope.md") > "$out/stdout" 2>&1 &
    spid=$!
    sleep 3
    kill -TERM "$spid"
    wait "$spid"
    rc=$?
    check "interrupted: exit 0, one attempt, valid result" \
        '.ok == false and (.attempts | length) == 1 and .attempts[0].outcome == "interrupted" and (.reason | test("SIGTERM"))'
    check_sh "no second attempt started" '[ "$(command cat "$fake/count")" = 1 ]'
    no_leftovers
fi

if want signal; then
    # A second TERM during cleanup (grace 3 s keeps it inside the window) must not stop the result.
    name=signal2 out="$work/signal2/out" fake="$work/signal2/fake"
    mkdir -p "$out" "$fake"
    res="$out/prepr-gemini.result.json"
    (cd "$repo" && exec env PATH="$bin:/usr/bin:/bin" FAKE_DIR="$fake" FAKE_AGY_PLAN=hangterm,ok \
        PRE_PR_GEMINI_AGY_HOME="$work/signal2/agyhome" PRE_PR_GEMINI_IDLE=100 PRE_PR_GEMINI_POLL=1 \
        PRE_PR_GEMINI_KILL_GRACE=3 "$script" HEAD~1 "$out" "$work/scope.md") > "$out/stdout" 2>&1 &
    spid=$!
    sleep 3
    kill -TERM "$spid"
    sleep 1
    kill -TERM "$spid" 2> /dev/null
    wait "$spid"
    rc=$?
    check "second signal during cleanup: still exit 0 and a result" \
        '.ok == false and (.attempts | length) == 1 and .attempts[0].outcome == "interrupted"'
    check_sh "no second attempt started" '[ "$(command cat "$fake/count")" = 1 ]'
    no_leftovers
fi

if want missing; then
    name=missing out="$work/missing/out" fake="$work/missing/fake"
    mkdir -p "$out" "$fake"
    res="$out/prepr-gemini.result.json"
    (cd "$repo" && env PATH="$nobin:/usr/bin:/bin" "$script" HEAD~1 "$out" "$work/scope.md") > /dev/null 2>&1
    rc=$?
    check "agy missing: no attempt" '.ok == false and .attempts == [] and (.reason | test("not found"))'

    BASE=HEAD run_case empty ok
    check "empty diff: no attempt" '.ok == false and .attempts == [] and (.reason | test("empty diff"))'
    check_sh "the fake was never called" '[ ! -e "$fake/count" ]'
fi

echo "pre-pr-gemini tests: $pass passed, $failed failed"
[ "$failed" -eq 0 ]
