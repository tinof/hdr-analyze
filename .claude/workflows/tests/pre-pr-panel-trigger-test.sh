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
  ['PQ histogram dump writer + display_sim reader (two contract sides)', ['hdr_analyzer_mvp/src/pq_hist_dump.rs', 'tools/display_sim/src/hist.rs'], false, true],
  ['display_sim alone (high-risk, one contract side)', ['tools/display_sim/src/sim.rs'], false, false],
  ['display_sim alone, hard: true', ['tools/display_sim/src/sim.rs'], true, true],
  ['libplacebo build script, hard: true (high-risk)', ['scripts/build-libplacebo.sh'], true, true],
  ['another script, hard: true (not high-risk)', ['scripts/rpu-baseline.sh'], true, false],
  ['mkvdovi injector, hard: true (high-risk)', ['mkvdovi/src/inject/interleave.rs'], true, true],
  ['mkvdovi injector as one file, hard: true (high-risk)', ['mkvdovi/src/inject.rs'], true, true],
  ['another mkvdovi module, hard: true (not high-risk)', ['mkvdovi/src/injector_notes.rs'], true, false],
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
  for (const f of ['tools/display_sim/src/tm_shim.c', 'scripts/build-libplacebo.sh', 'tools/display_sim/build.rs', 'tools/display_sim/Cargo.lock']) {
    const t = await run({ base: 'origin/main', files: [f], gatesOnly: true })
    check(`gates: ${f} requires tool:display_sim`, t.requiredGates.includes('tool:display_sim'))
  }
  for (const f of ['hdr_analyzer_mvp/src/pq_hist_dump.rs', 'hdr_analyzer_mvp/src/pq_hist_dump/mod.rs', 'hdr_analyzer_mvp/tests/pq_hist_dump.rs']) {
    const c = await run({ base: 'origin/main', files: [f], gatesOnly: true })
    check(`gates: ${f} requires cuda-parity and clippy-cuda`, c.requiredGates.includes('cuda-parity') && c.requiredGates.includes('clippy-cuda'))
  }
  const o = await run({ base: 'origin/main', files: ['tools/other/src/x.c'], gatesOnly: true })
  check('gates: a C file of another tool does not require tool:display_sim', !o.requiredGates.includes('tool:display_sim'))
  const d = await run({ base: 'origin/main', files: ['docs/CI.md'], gatesOnly: true })
  check('gates: a docs-only diff requires none', d.requiredGates.length === 0)
  if (fails) { console.log(`${fails} pre-pr-panel trigger test(s) failed`); process.exit(1) }
  console.log('all pre-pr-panel trigger tests passed')
})().catch(e => { console.log('FAIL ' + e.message); process.exit(1) })
EOF
