#!/usr/bin/env bash
# Tests for the reviewer decision and gate list of ../pre-pr-panel.js, through its gatesOnly mode
# (no agents start). The script runs under node with stub workflow globals. Expected values come
# from the rule in .claude/workflow.md "## Notes" and the pre-pr-review skill: Fable reviews a diff
# that is high-risk (a `## Tier 2 paths` file) and hard (CPU/CUDA numeric code, two or more sides
# of a cross-binary contract, or `hard: true` from the skill).
#
# Usage: .claude/workflows/tests/pre-pr-panel-trigger-test.sh   (needs node)
set -u
here=$(cd "$(dirname "$0")" && pwd)
command -v node >/dev/null || { echo "FAIL node not found"; exit 1; }
node - "$here/../pre-pr-panel.js" <<'EOF'
const fs = require('fs')
const src = fs.readFileSync(process.argv[2], 'utf8').replace(/^export const meta\b/m, 'const meta')
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
const run = args => {
  const stub = () => { throw new Error('gatesOnly must start no agent') }
  return new AsyncFunction('args', 'log', 'agent', 'parallel', 'pipeline', 'phase', src)(
    args, () => {}, stub, stub, stub, stub)
}
let fails = 0
const check = (name, ok) => { console.log((ok ? 'ok   ' : 'FAIL ') + name); if (!ok) fails++ }
const cases = [
  // [name, files, hard, expected fable]
  ['docs only, hard: true', ['docs/CI.md', 'CLAUDE.md'], true, false],
  ['review tooling only, hard: true', ['.claude/workflows/pre-pr-panel.js'], true, false],
  ['analysis code (numeric)', ['hdr_analyzer_mvp/src/analysis/histogram.rs'], false, true],
  ['composer code (numeric)', ['dovi84_composer/src/bt2100_v1.rs'], false, true],
  ['analyzer cli + mkvdovi external (two contract sides)', ['hdr_analyzer_mvp/src/cli.rs', 'mkvdovi/src/external.rs'], false, true],
  ['mkvdovi measure + l1_diff (two contract sides)', ['mkvdovi/src/pipeline/measure.rs', 'tools/l1_diff/src/main.rs'], false, true],
  ['analyzer cli alone (one contract side)', ['hdr_analyzer_mvp/src/cli.rs'], false, false],
  ['mkvdovi pipeline alone (high-risk, not hard)', ['mkvdovi/src/pipeline/convert.rs'], false, false],
  ['mkvdovi pipeline alone, hard: true', ['mkvdovi/src/pipeline/convert.rs'], true, true],
  ['RPU builder alone, hard: true', ['dovi_rpu_builder/src/lib.rs'], true, true],
  ['hard as a string is not hard', ['mkvdovi/src/pipeline/convert.rs'], 'true', false],
]
;(async () => {
  for (const [name, files, hard, want] of cases) {
    const args = { base: 'origin/main', files, gatesOnly: true }
    if (hard !== false) args.hard = hard
    const r = await run(args)
    check(`${name}: fable ${want}`, r && r.gatesOnly === true && r.fable === want)
  }
  const g = await run({ base: 'origin/main', files: ['.claude/hooks/rust-check.sh', '.claude/workflows/pre-pr-panel.js'], gatesOnly: true })
  check('gates: the hook path requires rust-check-hook', g.requiredGates.includes('rust-check-hook'))
  check('gates: the panel requires panel-trigger', g.requiredGates.includes('panel-trigger'))
  const d = await run({ base: 'origin/main', files: ['docs/CI.md'], gatesOnly: true })
  check('gates: a docs-only diff requires none', d.requiredGates.length === 0)
  if (fails) { console.log(`${fails} pre-pr-panel trigger test(s) failed`); process.exit(1) }
  console.log('all pre-pr-panel trigger tests passed')
})().catch(e => { console.log('FAIL ' + e.message); process.exit(1) })
EOF
