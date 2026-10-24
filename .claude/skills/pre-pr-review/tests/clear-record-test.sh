#!/usr/bin/env bash
# Tests for ../clear-record.sh in a throwaway repository with a linked worktree: it removes the
# three record files of the named commit from the common git dir, leaves every other commit's
# record alone, works from a worktree, and refuses a name that is not a commit.
#
# Usage: .claude/skills/pre-pr-review/tests/clear-record-test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../clear-record.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/clear-record-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }

repo="$work/repo"
git init -q "$repo"
git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m one
one=$(git -C "$repo" rev-parse HEAD)
git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m two
two=$(git -C "$repo" rev-parse HEAD)
git -C "$repo" worktree add -q --detach "$work/wt" "$one"
rec="$repo/.git/pre-pr-review"
mkdir -p "$rec"
for s in "$one" "$two"; do
    for ext in json md scope.md; do : > "$rec/$s.$ext"; done
done

# 1. default HEAD from the main checkout: removes HEAD's three files, keeps the other commit's
out=$(cd "$repo" && "$script")
rc=$?
if [ "$rc" -eq 0 ] && [ ! -e "$rec/$two.json" ] && [ ! -e "$rec/$two.md" ] && [ ! -e "$rec/$two.scope.md" ]; then
    ok "removes HEAD's record"
else
    bad "removes HEAD's record (exit $rc)"
fi
if [ -e "$rec/$one.json" ] && [ -e "$rec/$one.md" ] && [ -e "$rec/$one.scope.md" ]; then
    ok "keeps another commit's record"
else
    bad "keeps another commit's record"
fi
[ "$(printf '%s\n' "$out" | grep -c '^removed ')" -eq 3 ] && ok "prints each removed file" || bad "prints each removed file: $out"

# 2. from a linked worktree, a short revision: the common dir is the main repo's .git
(cd "$work/wt" && "$script" "${one:0:9}") > /dev/null
if [ ! -e "$rec/$one.json" ] && [ ! -e "$rec/$one.md" ] && [ ! -e "$rec/$one.scope.md" ]; then
    ok "works from a worktree with a short sha"
else
    bad "works from a worktree with a short sha"
fi

# 3. nothing left to remove: exit 0, no output
out=$(cd "$repo" && "$script")
rc=$?
[ "$rc" -eq 0 ] && [ -z "$out" ] && ok "nothing to remove is not an error" || bad "nothing to remove (exit $rc, '$out')"

# 4. not a commit: exit 2, nothing removed
: > "$rec/$two.json"
(cd "$repo" && "$script" no-such-rev) > /dev/null 2>&1
rc=$?
[ "$rc" -eq 2 ] && [ -e "$rec/$two.json" ] && ok "refuses a name that is not a commit" || bad "refuses a non-commit (exit $rc)"

[ "$fails" -eq 0 ] && echo "clear-record-test: all passed" || echo "clear-record-test: $fails failed"
[ "$fails" -eq 0 ]
