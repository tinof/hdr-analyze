#!/usr/bin/env bash
set -euo pipefail

INPUT="p7_sample_10s.mkv"
BINARY="target/release/mkvdolby"

echo "=== Profile 7 FEL Pipeline Profiler ==="
echo "Input: $INPUT ($(du -h "$INPUT" | cut -f1))"
echo ""

OVERALL_START=$(date +%s%3N)

# Run the tool, capturing all output and adding timestamps per line
"$BINARY" "$INPUT" -v 2>&1 | while IFS= read -r line; do
    printf "[%s] %s\n" "$(date +%H:%M:%S.%3N)" "$line"
done

OVERALL_END=$(date +%s%3N)
OVERALL_MS=$((OVERALL_END - OVERALL_START))

echo ""
echo "=== TOTAL WALL TIME: ${OVERALL_MS}ms ($(echo "scale=1; $OVERALL_MS / 1000" | bc)s) ==="
