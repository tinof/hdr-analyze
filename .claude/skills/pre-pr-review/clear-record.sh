#!/usr/bin/env bash
# Removes the pre-pr-review record of one commit (SKILL.md step 1.2), so a review that stops early
# cannot leave an older, reusable record behind. A record is three files in the git common dir,
# which every worktree shares: pre-pr-review/<full sha>.json, .md and .scope.md.
#
# Usage: .claude/skills/pre-pr-review/clear-record.sh [<commit>]   (default HEAD)
# Prints each file it removed. Exit 0 also when there was nothing to remove; 2 when <commit> does
# not name a commit or the directory is not a git repository.
#
# Why a script: the inline `rm -f "$D/$S.json" …` with command substitutions was refused by
# Claude Code's command safety check in six review runs; a fixed script path with resolved paths
# is one command that can be allowed.
set -u

rev=${1:-HEAD}
sha=$(git rev-parse --verify --quiet "$rev^{commit}") || {
    echo "clear-record: not a commit: $rev" >&2
    exit 2
}
common=$(git rev-parse --path-format=absolute --git-common-dir) || exit 2
dir="$common/pre-pr-review"

for f in "$dir/$sha.json" "$dir/$sha.md" "$dir/$sha.scope.md"; do
    if [ -e "$f" ]; then
        rm -f -- "$f" && echo "removed $f"
    fi
done
exit 0
