#!/usr/bin/env bash
# Tests for ../reviewer-yield.sh on hand-made records: counts runs from `reviewers` (gemini and
# codex-focused from geminiPass and codexFocusedPass), counts a finding as beyond-Opus only when
# confirmed and no Opus lens shares it, gives verdicts (fable, gemini) at the run threshold and
# none to a reviewer that left the panel (sonnet), skips records older than --since and unreadable files with a note, and refuses bad
# arguments.
#
# Usage: .claude/skills/pre-pr-review/tests/reviewer-yield-test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../reviewer-yield.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/reviewer-yield-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
has() { if printf '%s\n' "$out" | grep -qF -- "$2"; then ok "$1"; else bad "$1 (missing: $2)"; printf '%s\n' "$out"; fi; }
hasnt() { if printf '%s\n' "$out" | grep -qF -- "$2"; then bad "$1 (unexpected: $2)"; else ok "$1"; fi; }

rec="$work/rec"
mkdir -p "$rec"
reviewers='[{"key":"correctness","ran":true},{"key":"fable","ran":true},{"key":"sonnet","ran":false,"reason":"left the panel 2026-11-10"}]'
# r1: fable alone, fixed P2 (beyond-Opus); fable with correctness, fixed (not beyond); sonnet refuted.
# Gemini and the Codex focused pass ran here.
cat > "$rec/aaaaaaaa1.json" <<EOF
{"head":"aaaaaaaa1111","reviewers":$reviewers,"geminiPass":"done","codexFocusedPass":"done","findings":[
 {"where":"a.rs:1","priority":"P2","verdict":"fixed","finders":["fable"]},
 {"where":"a.rs:2","priority":"P1","verdict":"fixed","finders":["correctness","fable"]},
 {"where":"a.rs:3","priority":"P3","verdict":"refuted","finders":["sonnet"]}]}
EOF
# r2: fable alone but refuted (not beyond); fable + codex-focused deferred P3 (beyond: codex is not Opus);
# fable + spec fixed P1 (not beyond: spec is an Opus lens). Gemini ran, found nothing, pass skipped is not a run.
cat > "$rec/bbbbbbbb2.json" <<EOF
{"head":"bbbbbbbb2222","reviewers":$reviewers,"geminiPass":"done","codexFocusedPass":"skipped","findings":[
 {"where":"b.rs:1","priority":"P1","verdict":"refuted","finders":["fable"]},
 {"where":"b.rs:3","priority":"P1","verdict":"fixed","finders":["spec","fable"]},
 {"where":"b.rs:2","priority":"P3","verdict":"deferred","finders":["fable","codex-focused"]}]}
EOF
# old: before --since, must not count.
cat > "$rec/cccccccc3.json" <<EOF
{"head":"cccccccc3333","reviewers":$reviewers,"findings":[{"where":"c.rs:1","priority":"P0","verdict":"fixed","finders":["sonnet"]}]}
EOF
touch -t 202601010000 "$rec/cccccccc3.json"
echo 'not json' > "$rec/dddddddd4.json"

# 1. counts and beyond-Opus with the default threshold (observing)
out=$("$script" --dir "$rec" --since 2026-06-01 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "exits 0" || bad "exits 0 (got $rc)"
has "reads two records plus the bad one" "3 record(s)"
has "names the unreadable record" "skipped (not a JSON record): $rec/dddddddd4.json"
has "fable row: 2 runs, 5 findings, 4 confirmed, 1 refuted, 2 beyond (0/0/1/1)" "fable              2         5          4        1  2 (0/0/1/1)"
has "gemini row: 2 runs from geminiPass, no findings" "gemini             2         0          0        0  0 (0/0/0/0)"
has "codex-focused row: 1 run (skipped is not a run), 1 finding" "codex-focused      1         1          1        0  1 (0/0/0/1)"
has "sonnet row: 0 runs, 1 finding refuted, 0 beyond" "sonnet             0         1          0        1  0 (0/0/0/0)"
has "lists the beyond-Opus P2" "aaaaaaaa P2 a.rs:1 [fable]"
has "lists the beyond-Opus P3 shared with codex" "bbbbbbbb P3 b.rs:2 [fable, codex-focused]"
hasnt "a finding shared with an Opus lens is not beyond-Opus" "a.rs:2 [correctness, fable]"
hasnt "a finding shared with the spec lens is not beyond-Opus" "b.rs:3 [spec, fable]"
hasnt "an old record does not count" "c.rs:1"
has "fable observing below the threshold" "fable: observing (2 of 10 runs)"
hasnt "a reviewer that left the panel gets no verdict" "sonnet:"
has "gemini observing below the threshold" "gemini: observing (2 of 10 runs)"
hasnt "codex-focused gets no verdict" "codex-focused:"

# 2. verdicts at the threshold
out=$("$script" --dir "$rec" --since 2026-06-01 --min-runs 2 2>&1)
has "fable keeps with a P2 beyond-Opus finding" "fable: keep (2 beyond-Opus finding(s) in 2 runs)"
has "gemini with no beyond-Opus finding is a removal candidate" "gemini: candidate for removal (no beyond-Opus finding at P0-P2 in 2 runs)"
rm "$rec/aaaaaaaa1.json"
out=$("$script" --dir "$rec" --since 2026-06-01 --min-runs 1 2>&1)
has "P3-only beyond-Opus makes a removal candidate" "fable: candidate for removal (no beyond-Opus finding at P0-P2 in 1 runs)"

# 3. no records, bad arguments
out=$("$script" --dir "$work/none" 2>&1); rc=$?
[ "$rc" -eq 0 ] && ok "missing dir exits 0" || bad "missing dir exits 0 (got $rc)"
has "missing dir says it is observing" "no records yet"
"$script" --dir "$rec" --since 08.11.2026 >/dev/null 2>&1; rc=$?
[ "$rc" -eq 2 ] && ok "refuses a bad date" || bad "refuses a bad date (got $rc)"
"$script" --dir "$rec" --min-runs x >/dev/null 2>&1; rc=$?
[ "$rc" -eq 2 ] && ok "refuses a bad run count" || bad "refuses a bad run count (got $rc)"
"$script" --bogus >/dev/null 2>&1; rc=$?
[ "$rc" -eq 2 ] && ok "refuses an unknown argument" || bad "refuses an unknown argument (got $rc)"

if [ "$fails" -eq 0 ]; then echo "all reviewer-yield tests passed"; else echo "$fails reviewer-yield test(s) failed"; exit 1; fi
