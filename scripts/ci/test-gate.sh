#!/usr/bin/env bash
# The workspace test gate: the `test` part of the `gate` in .claude/workflow.md, also run by
# /pre-pr-review step 5. It builds the current analyzer next to the debug mkvdovi that the
# integration tests run, runs the workspace tests and fails on a failed build, a failed test or
# a `Skipping` line outside the allow-list of scripts/ci/check-test-skips.sh, plus the libx265
# HDR10+ skips that CI also allows (Ubuntu's libx265 has no HDR10_PLUS). The CUDA parity test
# is skipped by name: its own gate (scripts/cuda-parity.sh) runs it.
#
# Usage: scripts/ci/test-gate.sh [log file] [test filter]   (default log: target/test-gate.log)
# With a filter it runs only the matching tests, with the same build and skip check: the
# `gate-focused` of .claude/workflow.md.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
cd "$(git -C "$here" rev-parse --show-toplevel)" || exit 1
log=${1:-target/test-gate.log}
filter=${2:-}
mkdir -p "$(dirname "$log")"
rm -f "$log"

# The log goes to a file, not through a pipe, so the exit status is cargo's.
cargo build -p hdr_analyzer_mvp && cargo test --workspace -- --nocapture --skip cuda_output_matches_cpu ${filter:+"$filter"} > "$log" 2>&1
gate_rc=$?
if [ ! -s "$log" ]; then
    echo 'the build failed before the tests ran'
elif [ "$gate_rc" -ne 0 ]; then
    grep -n -E 'test result: FAILED|error: test failed' "$log"
    echo "--- last 40 lines of $log ---"
    tail -n 40 "$log"
elif [ -n "$filter" ] && ! grep -qE 'test result: ok\. [1-9]' "$log"; then
    echo "no test matched the filter '$filter'"
    gate_rc=1
fi
if [ -s "$log" ] && ! "$here/check-test-skips.sh" "$log" 'built without HDR10_PLUS|does not accept dhdr10-info'; then
    echo 'unexpected skip: required coverage did not run'
    gate_rc=1
fi
echo "test gate exit $gate_rc (log: $log)"
[ "$gate_rc" -eq 0 ]
