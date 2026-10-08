#!/usr/bin/env bash
# Fail when a `cargo test -- --nocapture` log shows a test that skipped for a reason the
# release must not hide: a missing tool (ffmpeg, libx265, dovi_tool, mkvmerge, mediainfo) or a
# missing sibling analyzer. Skips for media that never lives in the repository are allowed.
#
# Usage: scripts/ci/check-test-skips.sh <test log> [extra allowed regex]
# The same allow-list as the pre-PR test gate (.claude/skills/pre-pr-review/SKILL.md).
set -euo pipefail

log=${1:?usage: $0 <test log> [extra allowed regex]}
allowed='HDR_ANALYZE_REAL_SAMPLE|MKVDOVI_FEL_SAMPLE|MKVDOVI_CORPUS_DIR|sample not found at'
if [[ -n ${2:-} ]]; then
    allowed="$allowed|$2"
fi

if [[ ! -s $log ]]; then
    echo "error: test log '$log' is missing or empty" >&2
    exit 1
fi

unexpected=$(grep 'Skipping' "$log" | grep -v -E "$allowed" || true)
if [[ -n $unexpected ]]; then
    echo "error: tests skipped for a reason the release must not hide:" >&2
    echo "$unexpected" >&2
    exit 1
fi
echo "check-test-skips: only allowed skips ($(grep -c 'Skipping' "$log" || true) lines)"
