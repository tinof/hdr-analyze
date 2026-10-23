#!/usr/bin/env bash
# Decide which ci.yml jobs a run needs: the `changes` job.
#
# Usage: scripts/ci/ci-changes.sh <event>
#   pull_request also needs GH_TOKEN, REPO (owner/name), PR (number) and PR_FILES (the PR's
#   changed_files count). FILES_JSON=<file> reads the files API response from that file instead
#   of calling the API (the tests: scripts/ci/tests/ci-changes-test.sh).
#
# Prints and, when GITHUB_OUTPUT is set, appends:
#   code=    true when any file lies outside the docs-only set (*.md, docs/, LICENSE): the Rust,
#            L1 and build jobs. The same set as gate-skip in .claude/workflow.md and the
#            pre-push hook's skip pattern.
#   gemini=  true when a path of the gemini-harness gate in .claude/workflow.md changed.
#   deps=    true when a Cargo manifest, a Cargo.lock or deny.toml changed: the advisory jobs.
# push and workflow_dispatch run everything; schedule runs only the advisory jobs.
#
# Fail safe: when the file list cannot be read, does not parse, is empty or has a different
# length from PR_FILES (an incomplete pagination), every output is true. The script exits 0
# then, so a detection failure never skips a job that should have run.
set -uo pipefail

event=${1:?usage: $0 <event>}
code=true gemini=true deps=true

emit() {
    printf 'code=%s\ngemini=%s\ndeps=%s\n' "$code" "$gemini" "$deps" | tee -a "${GITHUB_OUTPUT:-/dev/null}"
    exit 0
}
run_all() {
    echo "::warning::$1; running every job"
    code=true gemini=true deps=true
    emit
}

case $event in
    pull_request) ;;
    schedule) code=false gemini=false deps=true; emit ;;
    *) emit ;;
esac

# One JSON object per changed file.
if [[ -n ${FILES_JSON:-} ]]; then
    files=$(jq -c '.[]' "$FILES_JSON" 2>&1) || run_all "the file list does not parse: $files"
else
    files=$(gh api --paginate "repos/${REPO:?}/pulls/${PR:?}/files?per_page=100" --jq '.[]' 2>&1) \
        || run_all "the PR file list could not be read: $files"
fi
count=$(jq -s 'length' <<< "$files" 2>/dev/null) || run_all "the file list does not parse"
if ! [[ $count =~ ^[1-9][0-9]*$ ]]; then
    run_all "the PR file list is empty"
fi
[[ $count == "${PR_FILES:-}" ]] || run_all "the PR lists ${PR_FILES:-?} changed files, the API returned $count"
valid=$(jq -s 'all(.[]; (.filename | type) == "string")' <<< "$files" 2>/dev/null)
[[ $valid == true ]] || run_all "a file entry has no file name"
names=$(jq -r '.filename, (.previous_filename // empty)' <<< "$files")

code=false gemini=false deps=false
while IFS= read -r f; do
    [[ $f =~ \.md$|^docs/|^LICENSE$ ]] || code=true
    [[ $f =~ ^\.claude/workflows/pre-pr-gemini\.|^\.claude/workflows/tests/|^\.agents/agents/pre-pr-reviewer\.md$ ]] && gemini=true
    [[ $f =~ (^|/)Cargo\.(toml|lock)$|^deny\.toml$ ]] && deps=true
done <<< "$names"
echo "$count changed files"
emit
