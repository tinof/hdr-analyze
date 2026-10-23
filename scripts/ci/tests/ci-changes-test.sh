#!/usr/bin/env bash
# Tests for ../ci-changes.sh: which ci.yml jobs a pull request runs, and that every failure to
# read the file list runs every job. No network: each case hands the script a files API
# response through FILES_JSON. ci.yml's `changes` job runs this before the script decides.
#
# Usage: scripts/ci/tests/ci-changes-test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../ci-changes.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/ci-changes-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fails=0

# check <name> <event> <PR_FILES> <files JSON> <expected outputs>
check() {
    local name=$1 event=$2 count=$3 json=$4 want=$5 got rc
    printf '%s' "$json" > "$work/files.json"
    FILES_JSON="$work/files.json" PR_FILES=$count GITHUB_OUTPUT='' "$script" "$event" > "$work/log" 2>&1
    rc=$?
    got=$(outputs "$work/log")
    if [[ $rc -eq 0 && $got == "$want" ]]; then
        echo "ok   $name"
    else
        echo "FAIL $name: exit $rc, got '$got', want '$want'"
        fails=$((fails + 1))
    fi
}
outputs() { grep -E '^(code|gemini|deps)=' "$1" | tr '\n' ' ' | sed 's/ $//'; }
f() { printf '{"filename":"%s"}' "$1"; }
ren() { printf '{"filename":"%s","previous_filename":"%s"}' "$1" "$2"; }

ALL='code=true gemini=true deps=true'

check 'docs only' pull_request 3 "[$(f ROADMAP.md),$(f docs/ROADMAP_LOG.md),$(f LICENSE)]" 'code=false gemini=false deps=false'
check 'nested markdown is docs' pull_request 1 "[$(f mkvdovi/README.md)]" 'code=false gemini=false deps=false'
check 'rust code' pull_request 2 "[$(f mkvdovi/src/main.rs),$(f CHANGELOG.md)]" 'code=true gemini=false deps=false'
check 'ci script is code' pull_request 1 "[$(f scripts/ci/test-gate.sh)]" 'code=true gemini=false deps=false'
check 'workflow is code' pull_request 1 "[$(f .github/workflows/ci.yml)]" 'code=true gemini=false deps=false'
check 'l1 reference csv is code' pull_request 1 "[$(f tools/l1_diff/corpus/pq.reference.csv)]" 'code=true gemini=false deps=false'
check 'docs-like name outside docs/ is code' pull_request 1 "[$(f scripts/docs.sh)]" 'code=true gemini=false deps=false'
check 'gemini prompt only' pull_request 1 "[$(f .agents/agents/pre-pr-reviewer.md)]" 'code=false gemini=true deps=false'
check 'gemini script' pull_request 1 "[$(f .claude/workflows/pre-pr-gemini.sh)]" 'code=true gemini=true deps=false'
check 'gemini harness test' pull_request 1 "[$(f .claude/workflows/tests/fake-agy)]" 'code=true gemini=true deps=false'
check 'other agent prompt is docs' pull_request 1 "[$(f .claude/agents/docs-keeper.md)]" 'code=false gemini=false deps=false'
check 'Cargo.lock' pull_request 1 "[$(f Cargo.lock)]" 'code=true gemini=false deps=true'
check 'crate manifest' pull_request 1 "[$(f dovi_rpu_builder/Cargo.toml)]" 'code=true gemini=false deps=true'
check 'deny.toml' pull_request 1 "[$(f deny.toml)]" 'code=true gemini=false deps=true'
check 'rename docs to code' pull_request 1 "[$(ren scripts/notes.sh docs/notes.md)]" 'code=true gemini=false deps=false'
check 'rename code to docs' pull_request 1 "[$(ren docs/old.md scripts/old.sh)]" 'code=true gemini=false deps=false'
check 'rename within docs' pull_request 1 "[$(ren docs/b.md docs/a.md)]" 'code=false gemini=false deps=false'

check 'push runs all' push 0 '[]' "$ALL"
check 'dispatch runs all' workflow_dispatch 0 '[]' "$ALL"
check 'schedule runs advisories only' schedule 0 '[]' 'code=false gemini=false deps=true'
check 'unknown event runs all' merge_group 0 '[]' "$ALL"

check 'empty list' pull_request 0 '[]' "$ALL"
check 'malformed json' pull_request 1 '[{"filename":' "$ALL"
check 'api error object' pull_request 2 '{"message":"Not Found","status":"404"}' "$ALL"
check 'entry without file name' pull_request 2 "[$(f docs/a.md),{\"sha\":\"x\"}]" "$ALL"
check 'count mismatch (incomplete pagination)' pull_request 5 "[$(f docs/a.md),$(f docs/b.md)]" "$ALL"
check 'missing PR_FILES' pull_request '' "[$(f docs/a.md)]" "$ALL"

# A failing API call: no FILES_JSON, and a gh on PATH that fails.
mkdir -p "$work/bin"
printf '#!/bin/sh\necho "HTTP 502" >&2\nexit 1\n' > "$work/bin/gh"
chmod +x "$work/bin/gh"
PATH="$work/bin:$PATH" REPO=o/r PR=1 PR_FILES=1 GITHUB_OUTPUT='' "$script" pull_request > "$work/log" 2>&1
rc=$?
got=$(outputs "$work/log")
if [[ $rc -eq 0 && $got == "$ALL" ]]; then
    echo "ok   api call fails"
else
    echo "FAIL api call fails: exit $rc, got '$got'"
    fails=$((fails + 1))
fi

# GITHUB_OUTPUT gets the same lines.
printf '[%s]' "$(f docs/a.md)" > "$work/files.json"
FILES_JSON="$work/files.json" PR_FILES=1 GITHUB_OUTPUT="$work/out" "$script" pull_request > /dev/null 2>&1
if [[ $(tr '\n' ' ' < "$work/out") == 'code=false gemini=false deps=false ' ]]; then
    echo "ok   writes GITHUB_OUTPUT"
else
    echo "FAIL writes GITHUB_OUTPUT: $(cat "$work/out")"
    fails=$((fails + 1))
fi

((fails == 0)) || { echo "$fails case(s) failed"; exit 1; }
echo "all cases passed"
